//! The material pass: soft shadows and sheets of a frame's [`Drawing`], between the source
//! layers and the drawing over them.
//!
//! A shadow's shapes render with vello at a reduced resolution, blur with a separable gaussian
//! and composite in their color. A sheet's shapes render flat into their own texture, which is
//! drawn again only when they change, so a card moving in renders once. The sheet composites
//! through a perspective transform per motion blur sample. On glass, the frame drawn so far is
//! read back from the target's planes, blurred the same way and tinted behind it.

use vello::kurbo::Affine;

use crate::RenderError;
use crate::drawing::{Drawing, Placement, Shape, Sheet, SoftShadow, intersect};
use crate::layout::{Canvas, Rect};
use crate::materials::{self, LIGHT};
use crate::surface::{Matrix, Plane, Range};
use crate::vector::VectorPass;

/// Bytes per material in the uniform buffer, a multiple of the dynamic offset alignment.
const MATERIAL_STRIDE: u64 = 768;
/// Size of the shader's `Material` struct.
const MATERIAL_SIZE: u64 = 528;
/// Bytes per blur pass in the uniform buffer, and the shader's `Blur` struct.
const BLUR_STRIDE: u64 = 256;
const BLUR_SIZE: u64 = 48;
/// Format of the blurred textures: enough precision for smooth gradients.
const BLURRED: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What vello renders into.
const DRAWN: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Canvas pixels per texel when blurring by `sigma`: the blur runs at a quarter of its own
/// width, at most 4x down.
fn downscale(sigma: f32) -> f32 {
    (sigma / 4.0).clamp(1.0, 4.0)
}

/// A texture slot reused across frames, replaced when a frame needs another size.
#[derive(Default)]
struct Slot(Option<(wgpu::Texture, wgpu::TextureView)>);

impl Slot {
    fn get(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> wgpu::TextureView {
        let fits = self
            .0
            .as_ref()
            .is_some_and(|(t, _)| (t.width(), t.height()) == size && t.format() == format);
        if !fits {
            let usage = wgpu::TextureUsages::TEXTURE_BINDING
                | if format == DRAWN {
                    wgpu::TextureUsages::STORAGE_BINDING
                } else {
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("material"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.0 = Some((texture, view));
        }
        self.0.as_ref().expect("set above").1.clone()
    }
}

/// The textures of one blur: what it reads, then the horizontal and the vertical result.
#[derive(Default)]
struct BlurSlots {
    source: Slot,
    across: Slot,
    blurred: Slot,
}

#[derive(Default)]
struct SheetSlots {
    content: Slot,
    /// What the content texture holds.
    drawn: Option<(Vec<Shape>, [f32; 2])>,
    backdrop: BlurSlots,
}

/// A draw with its bind group and uniform offset.
struct Bound {
    group: wgpu::BindGroup,
    offset: u32,
}

/// One gaussian blur to encode.
struct Blur {
    /// For a backdrop: the extract pass's uniform offset and the texture it writes.
    extract: Option<(u32, wgpu::TextureView)>,
    across: (Bound, wgpu::TextureView),
    down: (Bound, wgpu::TextureView),
}

/// What a frame's materials need encoded.
pub(crate) struct Prepared {
    shadow_blurs: Vec<Blur>,
    shadows: Vec<Bound>,
    backdrops: Vec<Blur>,
    sheets: Vec<Bound>,
}

impl Prepared {
    pub(crate) fn has_glass(&self) -> bool {
        !self.backdrops.is_empty()
    }
}

pub(crate) struct MaterialPass {
    composite: [wgpu::RenderPipeline; 2],
    extract: wgpu::RenderPipeline,
    gaussian: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    materials: wgpu::Buffer,
    blurs: wgpu::Buffer,
    shadows: Vec<BlurSlots>,
    sheets: Vec<SheetSlots>,
    /// Bound where a draw has no second texture.
    blank: wgpu::TextureView,
}

impl MaterialPass {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout_of = |label, size| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[
                    texture(0),
                    texture(1),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: wgpu::BufferSize::new(size),
                        },
                        count: None,
                    },
                ],
            })
        };
        let layout = layout_of("material", MATERIAL_SIZE);
        let blur_layout = layout_of("blur", BLUR_SIZE);
        let pipeline = |layout: &wgpu::BindGroupLayout,
                        module: &wgpu::ShaderModule,
                        entry: &str,
                        format: wgpu::TextureFormat,
                        composite: bool| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: if composite {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: composite.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let material = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("material"),
            source: wgpu::ShaderSource::Wgsl(include_str!("material.wgsl").into()),
        });
        let blur = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let composite = Plane::BOTH.map(|plane| {
            let entry = match plane {
                Plane::Luma => "luma_plane",
                Plane::Chroma => "chroma_plane",
            };
            pipeline(&layout, &material, entry, plane.format(), true)
        });
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let blank = Slot::default().get(device, (1, 1), BLURRED);
        Self {
            extract: pipeline(&blur_layout, &blur, "extract", BLURRED, false),
            gaussian: pipeline(&blur_layout, &blur, "gaussian", BLURRED, false),
            composite,
            layout,
            blur_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            materials: buffer("materials", MATERIAL_STRIDE),
            blurs: buffer("blurs", BLUR_STRIDE),
            shadows: Vec::new(),
            sheets: Vec::new(),
            blank,
        }
    }

    /// Renders the shapes of `drawing`'s shadows and sheets and binds what the frame needs
    /// encoded. `None` when it has no materials.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vector: &mut VectorPass,
        drawing: &Drawing,
        canvas: Canvas,
        range: Range,
        matrix: Matrix,
    ) -> Result<Option<Prepared>, RenderError> {
        if drawing.shadows.is_empty() && drawing.sheets.is_empty() {
            return Ok(None);
        }
        let count = drawing.shadows.len() + drawing.sheets.len();
        self.reserve(device, count);
        if self.shadows.len() < drawing.shadows.len() {
            self.shadows
                .resize_with(drawing.shadows.len(), BlurSlots::default);
        }
        if self.sheets.len() < drawing.sheets.len() {
            self.sheets
                .resize_with(drawing.sheets.len(), SheetSlots::default);
        }
        let mut uniforms = Uniforms::default();
        let mut prepared = Prepared {
            shadow_blurs: Vec::new(),
            shadows: Vec::new(),
            backdrops: Vec::new(),
            sheets: Vec::new(),
        };
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let target = Target {
            canvas,
            range,
            matrix,
        };

        for (index, shadow) in drawing.shadows.iter().enumerate() {
            let Some(area) = shadow.bounds().and_then(|b| intersect(whole(b), full)) else {
                continue;
            };
            let scale = downscale(shadow.blur);
            let size = texels(area, scale);
            let mask = self.shadows[index].source.get(device, size, DRAWN);
            let at = Affine::scale(1.0 / f64::from(scale))
                * Affine::translate((
                    f64::from(shadow.offset[0] - area.left),
                    f64::from(shadow.offset[1] - area.top),
                ));
            vector.render(device, queue, &shadow.shapes, at, size, &mask)?;
            let blur = self.blur(
                device,
                index,
                None,
                size,
                shadow.blur / scale,
                &mut uniforms,
            );
            let blurred = blur.down.1.clone();
            prepared.shadow_blurs.push(blur);
            let offset = uniforms.material(shadow_values(shadow, area, &target));
            prepared.shadows.push(Bound {
                group: self.group(
                    &self.layout,
                    &self.materials,
                    MATERIAL_SIZE,
                    device,
                    [&self.blank, &blurred],
                ),
                offset,
            });
        }

        for (index, sheet) in drawing.sheets.iter().enumerate() {
            let plan = SheetPlan::new(sheet, canvas);
            let Some(bounds) = intersect(whole(plan.bounds), full) else {
                continue;
            };
            let size = (
                sheet.size[0].ceil() as u32 + 2,
                sheet.size[1].ceil() as u32 + 2,
            );
            let slots = &mut self.sheets[index];
            let fresh = slots
                .content
                .0
                .as_ref()
                .is_some_and(|(t, _)| (t.width(), t.height()) == size)
                && slots
                    .drawn
                    .as_ref()
                    .is_some_and(|(shapes, at)| *shapes == sheet.shapes && *at == sheet.size);
            let content = slots.content.get(device, size, DRAWN);
            if !fresh {
                let at = Affine::translate((1.0, 1.0));
                vector.render(device, queue, &sheet.shapes, at, size, &content)?;
                slots.drawn = Some((sheet.shapes.clone(), sheet.size));
            }
            let mut backdrop = None;
            if let Some(glass) = sheet.glass {
                let reach = glass.blur * 3.0;
                let covered = Rect::new(
                    (bounds.left - reach).max(0.0),
                    (bounds.top - reach).max(0.0),
                    (bounds.right + reach).min(full.right),
                    (bounds.bottom + reach).min(full.bottom),
                );
                let scale = downscale(glass.blur);
                let texels = texels(covered, scale);
                let covered = Rect::new(
                    covered.left,
                    covered.top,
                    covered.left + texels.0 as f32 * scale,
                    covered.top + texels.1 as f32 * scale,
                );
                let origin = [covered.left, covered.top];
                let blur = self.blur(
                    device,
                    index,
                    Some((origin, scale, &target)),
                    texels,
                    glass.blur / scale,
                    &mut uniforms,
                );
                backdrop = Some((covered, blur.down.1.clone()));
                prepared.backdrops.push(blur);
            }
            let values = sheet_values(
                sheet,
                &plan,
                bounds,
                size,
                backdrop.as_ref().map(|b| b.0),
                &target,
            );
            let offset = uniforms.material(values);
            let second = backdrop.map_or(self.blank.clone(), |b| b.1);
            prepared.sheets.push(Bound {
                group: self.group(
                    &self.layout,
                    &self.materials,
                    MATERIAL_SIZE,
                    device,
                    [&content, &second],
                ),
                offset,
            });
        }
        queue.write_buffer(&self.materials, 0, &uniforms.materials);
        if !uniforms.blurs.is_empty() {
            queue.write_buffer(&self.blurs, 0, &uniforms.blurs);
        }
        Ok(Some(prepared))
    }

    /// Makes sure the uniform buffers hold `count` materials and their blurs.
    fn reserve(&mut self, device: &wgpu::Device, count: usize) {
        let grow = |buffer: &mut wgpu::Buffer, size: u64| {
            if buffer.size() < size {
                *buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("material uniforms"),
                    size: size.next_power_of_two(),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
        };
        grow(&mut self.materials, MATERIAL_STRIDE * count as u64);
        grow(&mut self.blurs, BLUR_STRIDE * 3 * count as u64);
    }

    fn group(
        &self,
        layout: &wgpu::BindGroupLayout,
        buffer: &wgpu::Buffer,
        size: u64,
        device: &wgpu::Device,
        [first, second]: [&wgpu::TextureView; 2],
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(first),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(second),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(size),
                    }),
                },
            ],
        })
    }

    /// Lays out a blur of `size` texels by `sigma` texels: of shadow `index`'s mask, or with
    /// `extract` of sheet `index`'s backdrop read from the target.
    fn blur(
        &mut self,
        device: &wgpu::Device,
        index: usize,
        extract: Option<([f32; 2], f32, &Target)>,
        size: (u32, u32),
        sigma: f32,
        uniforms: &mut Uniforms,
    ) -> Blur {
        let slots = match extract {
            Some(_) => &mut self.sheets[index].backdrop,
            None => &mut self.shadows[index],
        };
        let format = if extract.is_some() { BLURRED } else { DRAWN };
        let source = slots.source.get(device, size, format);
        let across = slots.across.get(device, size, BLURRED);
        let blurred = slots.blurred.get(device, size, BLURRED);
        let extract = extract.map(|(origin, scale, target)| {
            let [kr, kb] = target.matrix.weights();
            let values = [
                origin[0],
                origin[1],
                scale,
                video_flag(target.range),
                kr,
                kb,
                0.0,
                0.0,
                0.0,
                target.canvas.width as f32,
                target.canvas.height as f32,
                0.0,
            ];
            (uniforms.blur(values), source.clone())
        });
        let pass = |step: [f32; 2]| {
            [
                0.0, 0.0, 1.0, 0.0, 0.0, 0.0, step[0], step[1], sigma, 0.0, 0.0, 0.0,
            ]
        };
        let (w, h) = (size.0 as f32, size.1 as f32);
        let across_offset = uniforms.blur(pass([1.0 / w, 0.0]));
        let down_offset = uniforms.blur(pass([0.0, 1.0 / h]));
        let bind = |view: &wgpu::TextureView, offset| Bound {
            group: self.group(
                &self.blur_layout,
                &self.blurs,
                BLUR_SIZE,
                device,
                [view, &self.blank],
            ),
            offset,
        };
        Blur {
            across: (bind(&source, across_offset), across.clone()),
            down: (bind(&across, down_offset), blurred),
            extract,
        }
    }

    /// Encodes the blurs of the shadows, which read nothing of the frame.
    pub(crate) fn encode_shadow_blurs(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        prepared: &Prepared,
    ) {
        for blur in &prepared.shadow_blurs {
            self.encode_blur(encoder, blur);
        }
    }

    /// Encodes the backdrops of glass sheets, read from the target's `planes` as drawn so far.
    pub(crate) fn encode_backdrops(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        prepared: &Prepared,
        [luma, chroma]: [&wgpu::TextureView; 2],
    ) {
        for blur in &prepared.backdrops {
            if let Some((offset, source)) = &blur.extract {
                let group = self.group(
                    &self.blur_layout,
                    &self.blurs,
                    BLUR_SIZE,
                    device,
                    [luma, chroma],
                );
                run(encoder, source, &self.extract, &group, *offset);
            }
            self.encode_blur(encoder, blur);
        }
    }

    fn encode_blur(&self, encoder: &mut wgpu::CommandEncoder, blur: &Blur) {
        for (bound, target) in [&blur.across, &blur.down] {
            run(encoder, target, &self.gaussian, &bound.group, bound.offset);
        }
    }

    /// Composites the shadows into the plane `pass` draws.
    pub(crate) fn draw_shadows(
        &self,
        pass: &mut wgpu::RenderPass,
        plane: Plane,
        prepared: &Prepared,
    ) {
        self.draw_all(pass, plane, &prepared.shadows);
    }

    /// Composites the sheets, in order, into the plane `pass` draws.
    pub(crate) fn draw_sheets(
        &self,
        pass: &mut wgpu::RenderPass,
        plane: Plane,
        prepared: &Prepared,
    ) {
        self.draw_all(pass, plane, &prepared.sheets);
    }

    fn draw_all(&self, pass: &mut wgpu::RenderPass, plane: Plane, bound: &[Bound]) {
        pass.set_pipeline(&self.composite[plane as usize]);
        for b in bound {
            pass.set_bind_group(0, &b.group, &[b.offset]);
            pass.draw(0..4, 0..1);
        }
    }
}

/// One full-target draw of `pipeline` into `target`.
fn run(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    offset: u32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("blur"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[offset]);
    pass.draw(0..3, 0..1);
}

/// The target a frame renders into.
struct Target {
    canvas: Canvas,
    range: Range,
    matrix: Matrix,
}

/// Uniform data of a frame, laid out at the dynamic offset strides.
#[derive(Default)]
struct Uniforms {
    materials: Vec<u8>,
    blurs: Vec<u8>,
}

impl Uniforms {
    fn material(&mut self, values: Vec<f32>) -> u32 {
        push(&mut self.materials, &values, MATERIAL_STRIDE)
    }

    fn blur(&mut self, values: [f32; 12]) -> u32 {
        push(&mut self.blurs, &values, BLUR_STRIDE)
    }
}

fn push(bytes: &mut Vec<u8>, values: &[f32], stride: u64) -> u32 {
    let offset = bytes.len();
    bytes.extend(values.iter().flat_map(|v| v.to_ne_bytes()));
    bytes.resize(offset + stride as usize, 0);
    offset as u32
}

fn video_flag(range: Range) -> f32 {
    match range {
        Range::Video => 1.0,
        Range::Full => 0.0,
    }
}

/// The smallest whole-pixel rectangle around `r`.
fn whole(r: Rect) -> Rect {
    Rect::new(
        r.left.floor(),
        r.top.floor(),
        r.right.ceil(),
        r.bottom.ceil(),
    )
}

/// Texels covering `area` at `scale` canvas pixels per texel, rounded up to a multiple of 32 so
/// a moving material keeps its textures instead of allocating new ones every frame.
fn texels(area: Rect, scale: f32) -> (u32, u32) {
    let round = |pixels: f32| ((pixels / scale).ceil().max(1.0) as u32).next_multiple_of(32);
    (round(area.width()), round(area.height()))
}

/// Where a sheet's motion blur samples place it, and the canvas rectangle they cover.
struct SheetPlan {
    /// Per sample: the map from canvas pixels to sheet pixels, and its opacity.
    samples: Vec<([[f64; 3]; 3], f32)>,
    bounds: Rect,
    sheen: f32,
}

impl SheetPlan {
    fn new(sheet: &Sheet, canvas: Canvas) -> Self {
        let [open, close] = sheet.placement;
        let corners = |p: Placement| {
            let h = homography(sheet.size, p, canvas);
            [
                [0.0, 0.0],
                [sheet.size[0], 0.0],
                [0.0, sheet.size[1]],
                sheet.size,
            ]
            .map(|[u, v]| project(&h, f64::from(u), f64::from(v)))
        };
        let (a, b) = (corners(open), corners(close));
        let travel = a
            .iter()
            .zip(&b)
            .map(|(p, q)| ((p[0] - q[0]).abs()).max((p[1] - q[1]).abs()))
            .fold(0.0, f64::max);
        let count = materials::samples(travel as f32, canvas);
        let at = |i: usize| {
            if count == 1 {
                return open;
            }
            mix(open, close, i as f32 / (count - 1) as f32)
        };
        let mut bounds: Option<Rect> = None;
        let mut samples = Vec::with_capacity(count);
        for i in 0..count {
            let placement = at(i);
            let h = homography(sheet.size, placement, canvas);
            for [u, v] in [
                [0.0, 0.0],
                [sheet.size[0], 0.0],
                [0.0, sheet.size[1]],
                sheet.size,
            ] {
                let [x, y] = project(&h, f64::from(u), f64::from(v));
                let point = Rect::new(x as f32, y as f32, x as f32, y as f32);
                bounds = Some(bounds.map_or(point, |b| crate::drawing::union(b, point)));
            }
            samples.push((invert(&h), placement.opacity));
        }
        let b = bounds.expect("at least one sample");
        let middle = at(count / 2);
        Self {
            samples,
            bounds: Rect::new(b.left - 2.0, b.top - 2.0, b.right + 2.0, b.bottom + 2.0),
            sheen: LIGHT.sheen(middle.tilt),
        }
    }
}

fn mix(a: Placement, b: Placement, t: f32) -> Placement {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Placement {
        center: [
            lerp(a.center[0], b.center[0]),
            lerp(a.center[1], b.center[1]),
        ],
        scale: lerp(a.scale, b.scale),
        tilt: [lerp(a.tilt[0], b.tilt[0]), lerp(a.tilt[1], b.tilt[1])],
        opacity: lerp(a.opacity, b.opacity),
    }
}

/// How far the camera stands from the canvas, in canvas widths: the strength of perspective.
const CAMERA_DISTANCE: f64 = 0.8;

/// The map from sheet pixels to canvas pixels, homogeneous: scaled about the sheet's center,
/// tilted about the horizontal then the vertical axis, seen from a camera in front of the canvas.
fn homography(size: [f32; 2], p: Placement, canvas: Canvas) -> [[f64; 3]; 3] {
    let (sx, cx) = f64::from(p.tilt[0]).to_radians().sin_cos();
    let (sy, cy) = f64::from(p.tilt[1]).to_radians().sin_cos();
    let s = f64::from(p.scale);
    let (w, h) = (f64::from(size[0]), f64::from(size[1]));
    let d = CAMERA_DISTANCE * f64::from(canvas.width);
    let (px, py) = (f64::from(p.center[0]), f64::from(p.center[1]));
    // A point (a, b) of the sheet relative to its center lands at X = a cy + b sx sy, Y = b cx,
    // Z = a sy - b sx cy, away from the viewer when positive.
    let rows = [[cy, sx * sy], [0.0, cx], [sy, -sx * cy]];
    // In sheet pixels: a = s u - s w / 2, b = s v - s h / 2.
    let affine = |[ra, rb]: [f64; 2]| [ra * s, rb * s, -(ra * w + rb * h) * s / 2.0];
    let [x, y, z] = rows.map(affine);
    let depth = [z[0], z[1], z[2] + d];
    let row = |r: [f64; 3], center: f64| {
        [
            d * r[0] + center * depth[0],
            d * r[1] + center * depth[1],
            d * r[2] + center * depth[2],
        ]
    };
    [row(x, px), row(y, py), depth]
}

fn project(h: &[[f64; 3]; 3], u: f64, v: f64) -> [f64; 2] {
    let apply = |r: [f64; 3]| r[0] * u + r[1] * v + r[2];
    let w = apply(h[2]);
    [apply(h[0]) / w, apply(h[1]) / w]
}

/// The inverse of `m`, from its adjugate. Scale does not matter for a homography.
fn invert(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c =
        |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    // Keep the homogeneous coordinate positive in front of the camera.
    let sign = if det < 0.0 { -1.0 } else { 1.0 };
    let norm = adj[2][2].abs().max(1e-12) * sign;
    adj.map(|r| r.map(|v| v / norm))
}

fn sheet_values(
    sheet: &Sheet,
    plan: &SheetPlan,
    bounds: Rect,
    texels: (u32, u32),
    backdrop: Option<Rect>,
    target: &Target,
) -> Vec<f32> {
    let [kr, kb] = target.matrix.weights();
    let tint = sheet.glass.map_or([0.0; 4], |g| g.tint.0);
    let behind = backdrop.unwrap_or(Rect::UNIT);
    let mut values = vec![
        bounds.left,
        bounds.top,
        bounds.right,
        bounds.bottom,
        target.canvas.width as f32,
        target.canvas.height as f32,
        sheet.size[0],
        sheet.size[1],
        texels.0 as f32,
        texels.1 as f32,
        sheet.radius,
        plan.samples.len() as f32,
        tint[0],
        tint[1],
        tint[2],
        tint[3],
        behind.left,
        behind.top,
        behind.right,
        behind.bottom,
        kr,
        kb,
        video_flag(target.range),
        if backdrop.is_some() { 1.0 } else { 0.0 },
        0.0,
        plan.sheen,
        0.0,
        0.0,
    ];
    let mut opacity = [0.0; 8];
    for (o, (_, alpha)) in opacity.iter_mut().zip(&plan.samples) {
        *o = *alpha;
    }
    values.extend(opacity);
    for i in 0..materials::MAX_SAMPLES {
        let rows = plan.samples.get(i).map_or([[0.0; 3]; 3], |s| s.0);
        for row in rows {
            values.extend([row[0] as f32, row[1] as f32, row[2] as f32, 0.0]);
        }
    }
    values
}

fn shadow_values(shadow: &SoftShadow, area: Rect, target: &Target) -> Vec<f32> {
    let [kr, kb] = target.matrix.weights();
    let c = shadow.color.0;
    let mut values = vec![
        area.left,
        area.top,
        area.right,
        area.bottom,
        target.canvas.width as f32,
        target.canvas.height as f32,
        1.0,
        1.0,
        1.0,
        1.0,
        0.0,
        1.0,
        c[0],
        c[1],
        c[2],
        c[3],
        area.left,
        area.top,
        area.left + texels(area, downscale(shadow.blur)).0 as f32 * downscale(shadow.blur),
        area.top + texels(area, downscale(shadow.blur)).1 as f32 * downscale(shadow.blur),
        kr,
        kb,
        video_flag(target.range),
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
    ];
    values.resize((MATERIAL_SIZE / 4) as usize, 0.0);
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    const UHD: Canvas = Canvas::UHD;

    fn placement(tilt: [f32; 2]) -> Placement {
        Placement {
            center: [1920.0, 1080.0],
            scale: 1.0,
            tilt,
            opacity: 1.0,
        }
    }

    #[test]
    fn a_flat_sheet_maps_its_pixels_one_to_one_around_its_center() {
        let h = homography([400.0, 200.0], placement([0.0, 0.0]), UHD);
        let [x, y] = project(&h, 0.0, 0.0);
        assert!((x - 1720.0).abs() < 1e-6 && (y - 980.0).abs() < 1e-6);
        let back = invert(&h);
        let [u, v] = project(&back, 1920.0, 1080.0);
        assert!((u - 200.0).abs() < 1e-6 && (v - 100.0).abs() < 1e-6);
    }

    #[test]
    fn tilting_the_top_away_narrows_the_top_edge_in_perspective() {
        let size = [400.0, 200.0];
        let h = homography(size, placement([20.0, 0.0]), UHD);
        let top = project(&h, 400.0, 0.0)[0] - project(&h, 0.0, 0.0)[0];
        let bottom = project(&h, 400.0, 200.0)[0] - project(&h, 0.0, 200.0)[0];
        assert!(top < 400.0 && bottom > 400.0, "{top} {bottom}");
        // Every sample maps back to where it came from.
        let back = invert(&h);
        let [x, y] = project(&h, 123.0, 45.0);
        let [u, v] = project(&back, x, y);
        assert!((u - 123.0).abs() < 1e-6 && (v - 45.0).abs() < 1e-6);
    }
}
