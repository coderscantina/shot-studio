//! The render graph: a [`Scene`] of source layers drawn into an IOSurface-backed 4:2:0 target.
//!
//! Each layer is one draw into the luma plane and one into the chroma plane, sampling the decoded
//! frame's planes in place. The target is whatever the consumer takes directly: the encoder's
//! `420v` BT.709 buffers for export, `420f` BT.601 buffers for gpui's preview surface. Range and
//! matrix are read from each buffer, so both sides show the same RGB.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::CVPixelBuffer;
use objc2_metal::MTLDevice;
use wgpu::hal::api::Metal;

use std::sync::Mutex;

use crate::RenderError;
use crate::drawing::Drawing;
use crate::frame::Frame;
use crate::layout::{Canvas, Scene, Sources};
use crate::material_pass::MaterialPass;
use crate::materials;
use crate::surface::{Matrix, Plane, Range, plane_texture, size};
use crate::vector::VectorPass;

/// Layers one frame can draw. 04 to 07 add overlays on top of the two sources.
const MAX_LAYERS: usize = 8;

/// Bytes per layer in the uniform buffer, the minimum dynamic offset alignment.
const LAYER_STRIDE: u64 = 256;
/// Size of the shader's `Layer` struct.
const LAYER_SIZE: u64 = 144;

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    metal: Retained<ProtocolObject<dyn MTLDevice>>,
    layout: wgpu::BindGroupLayout,
    /// Indexed by [`Plane`]: luma, then chroma.
    pipelines: [wgpu::RenderPipeline; 2],
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    /// The drawing under and over the layers, see [`crate::vector`].
    vector: Mutex<VectorPass>,
    /// Shadows and sheets between them, see [`crate::material_pass`].
    material: Mutex<MaterialPass>,
}

impl Renderer {
    /// A renderer on its own wgpu device on the system GPU.
    pub fn new() -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(|e| RenderError::Gpu(e.to_string()))?;
        // vello's compute stages need more than the WebGPU default limits.
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| RenderError::Gpu(e.to_string()))?;
        // SAFETY: The backend is Metal, so the hal device is the Metal one.
        let metal = unsafe { device.as_hal::<Metal>() }
            .ok_or_else(|| RenderError::Gpu("wgpu is not on Metal".into()))?
            .raw_device()
            .clone();

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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
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
                        min_binding_size: wgpu::BufferSize::new(LAYER_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compose"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compose"),
            source: wgpu::ShaderSource::Wgsl(include_str!("compose.wgsl").into()),
        });
        let pipelines = Plane::BOTH.map(|plane| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("compose"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layers"),
            size: LAYER_STRIDE * MAX_LAYERS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vector = Mutex::new(VectorPass::new(&device)?);
        let material = Mutex::new(MaterialPass::new(&device));
        Ok(Self {
            device,
            queue,
            metal,
            layout,
            pipelines,
            sampler,
            uniforms,
            vector,
            material,
        })
    }

    /// Draws `frame`, its layers and its drawing, into `target`. See [`Self::render`].
    pub fn draw(
        &self,
        frame: &Frame,
        sources: Sources<Option<&CVPixelBuffer>>,
        target: &CVPixelBuffer,
    ) -> Result<(), RenderError> {
        self.render_with(&frame.scene, &frame.drawing, sources, target, true)
    }

    /// Draws `frame` on top of what `target` holds, without its background: for a frame
    /// drawn over another, such as one side of a transition over the other.
    pub fn draw_over(
        &self,
        frame: &Frame,
        sources: Sources<Option<&CVPixelBuffer>>,
        target: &CVPixelBuffer,
    ) -> Result<(), RenderError> {
        self.render_with(&frame.scene, &frame.drawing, sources, target, false)
    }

    /// Draws `scene` into `target` and waits for the GPU, so the target can go straight to the
    /// encoder or the display. `sources` holds the decoded frame of each source a layer uses.
    pub fn render(
        &self,
        scene: &Scene,
        sources: Sources<Option<&CVPixelBuffer>>,
        target: &CVPixelBuffer,
    ) -> Result<(), RenderError> {
        self.render_with(scene, &Drawing::default(), sources, target, true)
    }

    fn render_with(
        &self,
        scene: &Scene,
        drawing: &Drawing,
        sources: Sources<Option<&CVPixelBuffer>>,
        target: &CVPixelBuffer,
        clear: bool,
    ) -> Result<(), RenderError> {
        if scene.layers.len() > MAX_LAYERS {
            return Err(RenderError::TooManyLayers(scene.layers.len()));
        }
        let target_range = Range::of(target)?;
        let target_matrix = Matrix::of(target);
        let (width, height) = size(target);
        let canvas = Canvas { width, height };

        struct Bound {
            group: wgpu::BindGroup,
            offset: u32,
        }
        let mut layers = Vec::with_capacity(scene.layers.len());
        for (index, layer) in scene.layers.iter().enumerate() {
            let source = sources
                .get(layer.source)
                .ok_or(RenderError::MissingSource(layer.source))?;
            let source_range = Range::of(source)?;
            let [source_kr, source_kb] = Matrix::of(source).weights();
            let [target_kr, target_kb] = target_matrix.weights();
            let views = Plane::BOTH.map(|plane| {
                plane_texture(
                    &self.device,
                    &self.metal,
                    source,
                    plane,
                    wgpu::TextureUsages::TEXTURE_BINDING,
                )
                .map(|texture| texture.create_view(&Default::default()))
            });
            let [luma, chroma] = views;
            let (luma, chroma) = (luma?, chroma?);
            let offset = index as u64 * LAYER_STRIDE;
            let quad = layer.extent();
            let border = layer.border.map_or([0.0; 4], |b| {
                let [r, g, b2] = b.color;
                [r, g, b2, b.width]
            });
            let shadow = layer
                .shadow
                .map_or([0.0; 4], |s| [s.offset[0], s.offset[1], s.blur, s.alpha]);
            let values = [
                layer.frame.left,
                layer.frame.top,
                layer.frame.right,
                layer.frame.bottom,
                layer.crop.left,
                layer.crop.top,
                layer.crop.right,
                layer.crop.bottom,
                canvas.width as f32,
                canvas.height as f32,
                layer.radius,
                video_flag(source_range),
                video_flag(target_range),
                layer.blur,
                materials::samples(layer.travel(), canvas) as f32,
                layer.opacity,
                source_kr,
                source_kb,
                target_kr,
                target_kb,
                layer.shutter.left,
                layer.shutter.top,
                layer.shutter.right,
                layer.shutter.bottom,
                border[0],
                border[1],
                border[2],
                border[3],
                shadow[0],
                shadow[1],
                shadow[2],
                shadow[3],
                quad.left,
                quad.top,
                quad.right,
                quad.bottom,
            ];
            let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
            self.queue.write_buffer(&self.uniforms, offset, &bytes);
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&luma),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&chroma),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.uniforms,
                            offset: 0,
                            size: wgpu::BufferSize::new(LAYER_SIZE),
                        }),
                    },
                ],
            });
            layers.push(Bound {
                group,
                offset: offset as u32,
            });
        }

        let mut vector = self.vector.lock().expect("vector pass poisoned");
        let [under, over] = vector.prepare(
            &self.device,
            &self.queue,
            drawing,
            canvas,
            target_range,
            target_matrix,
        )?;
        let material = &mut *self.material.lock().expect("material pass poisoned");
        let materials = material.prepare(
            &self.device,
            &self.queue,
            &mut vector,
            drawing,
            canvas,
            target_range,
            target_matrix,
        )?;
        let glass = materials.as_ref().is_some_and(|m| m.has_glass());
        let usage = if glass {
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        };
        let [luma, chroma] = Plane::BOTH.map(|plane| {
            plane_texture(&self.device, &self.metal, target, plane, usage)
                .map(|texture| texture.create_view(&Default::default()))
        });
        let views = [luma?, chroma?];
        let background = ycbcr(scene.background, target_range, target_matrix);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if let Some(prepared) = &materials {
            material.encode_shadow_blurs(&mut encoder, prepared);
        }
        // Up to three passes over both planes: the layers, the shadows, then what sits on the
        // backdrop glass reads back.
        let load = |plane: Plane, clear: bool| {
            if !clear {
                return wgpu::LoadOp::Load;
            }
            let color = match plane {
                Plane::Luma => [background[0], 0.0],
                Plane::Chroma => [background[1], background[2]],
            };
            wgpu::LoadOp::Clear(wgpu::Color {
                r: f64::from(color[0]),
                g: f64::from(color[1]),
                b: 0.0,
                a: 1.0,
            })
        };
        for plane in Plane::BOTH {
            let mut pass = begin_pass(&mut encoder, &views[plane as usize], load(plane, clear));
            if let Some(under) = &under {
                vector.draw(&mut pass, plane, under);
            }
            pass.set_pipeline(&self.pipelines[plane as usize]);
            for layer in &layers {
                pass.set_bind_group(0, &layer.group, &[layer.offset]);
                pass.draw(0..4, 0..1);
            }
            if let Some(prepared) = &materials {
                material.draw_shadows(&mut pass, plane, prepared);
                if glass {
                    continue;
                }
                material.draw_sheets(&mut pass, plane, prepared);
            }
            if let Some(over) = &over {
                vector.draw(&mut pass, plane, over);
            }
        }
        if let (Some(prepared), true) = (&materials, glass) {
            material.encode_backdrops(&self.device, &mut encoder, prepared, [&views[0], &views[1]]);
            for plane in Plane::BOTH {
                let mut pass = begin_pass(&mut encoder, &views[plane as usize], load(plane, false));
                material.draw_sheets(&mut pass, plane, prepared);
                if let Some(over) = &over {
                    vector.draw(&mut pass, plane, over);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Gpu(e.to_string()))?;
        Ok(())
    }
}

fn begin_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("compose"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

fn video_flag(range: Range) -> f32 {
    match range {
        Range::Video => 1.0,
        Range::Full => 0.0,
    }
}

/// YCbCr of a gamma-encoded RGB color in `range` and `matrix`, matching the shader.
fn ycbcr([r, g, b]: [f32; 3], range: Range, matrix: Matrix) -> [f32; 3] {
    let [kr, kb] = matrix.weights();
    let y = kr * r + (1.0 - kr - kb) * g + kb * b;
    let (cb, cr) = ((b - y) / (2.0 * (1.0 - kb)), (r - y) / (2.0 * (1.0 - kr)));
    match range {
        Range::Video => [
            (16.0 + 219.0 * y) / 255.0,
            (128.0 + 224.0 * cb) / 255.0,
            (128.0 + 224.0 * cr) / 255.0,
        ],
        Range::Full => [y, cb + 128.0 / 255.0, cr + 128.0 / 255.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_black_land_on_the_bt709_code_values() {
        let to_codes = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as u8);
        let bt709 = Matrix::Bt709;
        assert_eq!(
            to_codes(ycbcr([1.0; 3], Range::Video, bt709)),
            [235, 128, 128]
        );
        assert_eq!(
            to_codes(ycbcr([0.0; 3], Range::Video, bt709)),
            [16, 128, 128]
        );
        assert_eq!(
            to_codes(ycbcr([1.0; 3], Range::Full, bt709)),
            [255, 128, 128]
        );
        // Pure red: Cr at its maximum.
        assert_eq!(
            to_codes(ycbcr([1.0, 0.0, 0.0], Range::Video, bt709)),
            [63, 102, 240]
        );
    }

    #[test]
    fn bt601_full_range_decodes_back_with_gpuis_matrix() {
        // gpui's surface shader: R = Y + 1.402 Cr, G = Y - 0.3441 Cb - 0.7141 Cr, B = Y + 1.772 Cb.
        let rgb = [0.75, 0.25, 0.5];
        let [y, cb, cr] = ycbcr(rgb, Range::Full, Matrix::Bt601);
        let (cb, cr) = (cb - 128.0 / 255.0, cr - 128.0 / 255.0);
        let back = [
            y + 1.402 * cr,
            y - 0.3441 * cb - 0.7141 * cr,
            y + 1.772 * cb,
        ];
        for (a, b) in rgb.iter().zip(back) {
            assert!((a - b).abs() < 0.5 / 255.0, "{rgb:?} came back as {back:?}");
        }
    }
}
