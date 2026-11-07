//! The vector pass: draws a frame's [`Drawing`] with vello and parley into RGBA textures on the
//! render graph's device, then composites them into the 4:2:0 target under and over the source
//! layers.
//!
//! Each half of the drawing renders only the rectangle its shapes cover. The under half (the
//! window's shadow and device frame) rarely changes within a segment, so it is drawn again only
//! when its shapes change.

use parley::{FontContext, GenericFamily, LayoutContext, PositionedLayoutItem, StyleProperty};
use vello::kurbo::{Affine, BezPath, Circle, Rect as KRect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use crate::RenderError;
use crate::drawing::{Drawing, Font, Rgba, Shape, union};
use crate::layout::{Canvas, Rect};
use crate::surface::{Matrix, Plane, Range};

/// Bytes per quad in the uniform buffer, the minimum dynamic offset alignment.
const QUAD_STRIDE: u64 = 256;
/// Size of the shader's `Quad` struct.
const QUAD_SIZE: u64 = 48;

/// One half of the drawing, rendered and ready to composite.
pub(crate) struct Bound {
    group: wgpu::BindGroup,
    offset: u32,
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// What the texture holds: the shapes and the rectangle they were drawn for.
    drawn: Option<(Vec<Shape>, Rect)>,
}

pub(crate) struct VectorPass {
    vello: Renderer,
    fonts: FontContext,
    layouts: LayoutContext<()>,
    scene: Scene,
    /// Under, then over.
    targets: [Option<Target>; 2],
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::RenderPipeline; 2],
    uniforms: wgpu::Buffer,
}

impl VectorPass {
    pub(crate) fn new(device: &wgpu::Device) -> Result<Self, RenderError> {
        let vello = Renderer::new(
            device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: std::num::NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .map_err(|e| RenderError::Gpu(e.to_string()))?;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vector"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(QUAD_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vector"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vector"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vector.wgsl").into()),
        });
        let pipelines = Plane::BOTH.map(|plane| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("vector"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(match plane {
                        Plane::Luma => "luma_plane",
                        Plane::Chroma => "chroma_plane",
                    }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: plane.format(),
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vector quads"),
            size: QUAD_STRIDE * 2,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            vello,
            fonts: FontContext::new(),
            layouts: LayoutContext::new(),
            scene: Scene::new(),
            targets: [None, None],
            layout,
            pipelines,
            uniforms,
        })
    }

    /// Renders both halves of `drawing` for a `canvas` target and returns them, under first.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        drawing: &Drawing,
        canvas: Canvas,
        range: Range,
        matrix: Matrix,
    ) -> Result<[Option<Bound>; 2], RenderError> {
        let halves = [&drawing.under, &drawing.over];
        let mut bound = [None, None];
        for (index, shapes) in halves.into_iter().enumerate() {
            let Some(area) = area(shapes, canvas) else {
                continue;
            };
            let target = self.targets[index].get_or_insert_with(|| new_target(device, canvas));
            let size = target.texture.size();
            if size.width < canvas.width || size.height < canvas.height {
                *target = new_target(device, canvas);
            }
            let fresh = target
                .drawn
                .as_ref()
                .is_some_and(|(drawn, at)| drawn == shapes && *at == area);
            if !fresh {
                self.scene.reset();
                let origin = Affine::translate((-f64::from(area.left), -f64::from(area.top)));
                for shape in shapes {
                    draw(
                        &mut self.scene,
                        &mut self.fonts,
                        &mut self.layouts,
                        origin,
                        shape,
                    );
                }
                self.vello
                    .render_to_texture(
                        device,
                        queue,
                        &self.scene,
                        &target.view,
                        &RenderParams {
                            base_color: Color::TRANSPARENT,
                            width: area.width() as u32,
                            height: area.height() as u32,
                            antialiasing_method: AaConfig::Area,
                        },
                    )
                    .map_err(|e| RenderError::Gpu(e.to_string()))?;
                target.drawn = Some((shapes.clone(), area));
            }
            let [kr, kb] = matrix.weights();
            let values: [f32; 12] = [
                area.left,
                area.top,
                area.right,
                area.bottom,
                canvas.width as f32,
                canvas.height as f32,
                match range {
                    Range::Video => 1.0,
                    Range::Full => 0.0,
                },
                0.0,
                kr,
                kb,
                0.0,
                0.0,
            ];
            let offset = index as u64 * QUAD_STRIDE;
            let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
            queue.write_buffer(&self.uniforms, offset, &bytes);
            let view = target
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.uniforms,
                            offset: 0,
                            size: wgpu::BufferSize::new(QUAD_SIZE),
                        }),
                    },
                ],
            });
            bound[index] = Some(Bound {
                group,
                offset: offset as u32,
            });
        }
        Ok(bound)
    }

    /// Composites one rendered half into the plane `pass` draws.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass, plane: Plane, bound: &Bound) {
        pass.set_pipeline(&self.pipelines[plane as usize]);
        pass.set_bind_group(0, &bound.group, &[bound.offset]);
        pass.draw(0..4, 0..1);
    }
}

fn new_target(device: &wgpu::Device, canvas: Canvas) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vector drawing"),
        size: wgpu::Extent3d {
            width: canvas.width,
            height: canvas.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Target {
        texture,
        view,
        drawn: None,
    }
}

/// The whole-pixel rectangle of the canvas `shapes` cover, `None` when they cover none of it.
fn area(shapes: &[Shape], canvas: Canvas) -> Option<Rect> {
    let bounds = shapes.iter().map(Shape::bounds).reduce(union)?;
    let rect = Rect::new(
        bounds.left.floor().max(0.0),
        bounds.top.floor().max(0.0),
        bounds.right.ceil().min(canvas.width as f32),
        bounds.bottom.ceil().min(canvas.height as f32),
    );
    (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
}

fn color(c: Rgba) -> Color {
    Color::new(c.0)
}

fn rect(r: Rect) -> KRect {
    KRect::new(
        f64::from(r.left),
        f64::from(r.top),
        f64::from(r.right),
        f64::from(r.bottom),
    )
}

fn draw(
    scene: &mut Scene,
    fonts: &mut FontContext,
    layouts: &mut LayoutContext<()>,
    transform: Affine,
    shape: &Shape,
) {
    match shape {
        Shape::Rect {
            rect: r,
            radius,
            color: c,
        } => scene.fill(
            Fill::NonZero,
            transform,
            color(*c),
            None,
            &RoundedRect::from_rect(rect(*r), f64::from(*radius)),
        ),
        Shape::Shadow {
            rect: r,
            radius,
            blur,
            color: c,
        } => scene.draw_blurred_rounded_rect(
            transform,
            rect(*r),
            color(*c),
            f64::from(*radius),
            f64::from(*blur) / 2.0,
        ),
        Shape::Ring {
            center,
            radius,
            width,
            color: c,
        } => scene.stroke(
            &Stroke::new(f64::from(*width)),
            transform,
            color(*c),
            None,
            &Circle::new(
                (f64::from(center[0]), f64::from(center[1])),
                f64::from(*radius),
            ),
        ),
        Shape::Pointer { tip, height } => {
            let path = pointer();
            let at = transform
                * Affine::translate((f64::from(tip[0]), f64::from(tip[1])))
                * Affine::scale(f64::from(*height));
            scene.fill(Fill::NonZero, at, Color::BLACK, None, &path);
            scene.stroke(&Stroke::new(0.055), at, Color::WHITE, None, &path);
        }
        Shape::Text {
            origin,
            text,
            size,
            font,
            color: c,
        } => {
            let at = transform * Affine::translate((f64::from(origin[0]), f64::from(origin[1])));
            glyphs(scene, fonts, layouts, at, text, *size, *font, *c);
        }
        Shape::Label {
            center,
            text,
            size,
            color: c,
            background,
        } => {
            let (width, height) = measure(fonts, layouts, text, *size, Font::Sans);
            let (pad_x, pad_y) = (size * 0.55, size * 0.3);
            let (w, h) = (width + 2.0 * pad_x, height + 2.0 * pad_y);
            let left = center[0] - w / 2.0;
            let top = center[1] - h / 2.0;
            scene.fill(
                Fill::NonZero,
                transform,
                color(*background),
                None,
                &RoundedRect::from_rect(
                    rect(Rect::new(left, top, left + w, top + h)),
                    f64::from(size * 0.3),
                ),
            );
            let at =
                transform * Affine::translate((f64::from(left + pad_x), f64::from(top + pad_y)));
            glyphs(scene, fonts, layouts, at, text, *size, Font::Sans, *c);
        }
        Shape::Clip {
            rect: r,
            radius,
            shapes,
        } => {
            scene.push_clip_layer(
                Fill::NonZero,
                transform,
                &RoundedRect::from_rect(rect(*r), f64::from(*radius)),
            );
            for inner in shapes {
                draw(scene, fonts, layouts, transform, inner);
            }
            scene.pop_layer();
        }
    }
}

/// The macOS arrow, one unit tall, tip at the origin.
fn pointer() -> BezPath {
    let points = [
        (0.0, 0.0),
        (0.0, 0.79),
        (0.19, 0.62),
        (0.31, 0.9),
        (0.43, 0.85),
        (0.31, 0.58),
        (0.56, 0.58),
    ];
    let mut path = BezPath::new();
    path.move_to(points[0]);
    for p in &points[1..] {
        path.line_to(*p);
    }
    path.close_path();
    path
}

fn text_layout(
    fonts: &mut FontContext,
    layouts: &mut LayoutContext<()>,
    text: &str,
    size: f32,
    font: Font,
) -> parley::Layout<()> {
    let mut builder = layouts.ranged_builder(fonts, text, 1.0, true);
    builder.push_default(StyleProperty::FontSize(size));
    builder.push_default(StyleProperty::from(match font {
        Font::Sans => GenericFamily::SystemUi,
        Font::Mono => GenericFamily::Monospace,
    }));
    if font == Font::Sans {
        builder.push_default(StyleProperty::FontWeight(parley::FontWeight::SEMI_BOLD));
    }
    let mut layout = builder.build(text);
    layout.break_all_lines(None);
    layout
}

fn measure(
    fonts: &mut FontContext,
    layouts: &mut LayoutContext<()>,
    text: &str,
    size: f32,
    font: Font,
) -> (f32, f32) {
    let layout = text_layout(fonts, layouts, text, size, font);
    (layout.width(), layout.height())
}

#[allow(clippy::too_many_arguments)]
fn glyphs(
    scene: &mut Scene,
    fonts: &mut FontContext,
    layouts: &mut LayoutContext<()>,
    transform: Affine,
    text: &str,
    size: f32,
    font: Font,
    c: Rgba,
) {
    let layout = text_layout(fonts, layouts, text, size, font);
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let glyph_run = run.run();
            let coords: Vec<i16> = glyph_run
                .normalized_coords()
                .iter()
                .map(|c| c.to_bits())
                .collect();
            scene
                .draw_glyphs(glyph_run.font())
                .font_size(glyph_run.font_size())
                .normalized_coords(&coords)
                .transform(transform)
                .brush(color(c))
                .draw(
                    Fill::NonZero,
                    run.positioned_glyphs().map(|g| vello::Glyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                    }),
                );
        }
    }
}
