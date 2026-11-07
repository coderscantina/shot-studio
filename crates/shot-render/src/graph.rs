//! The render graph: a [`Scene`] of source layers drawn into an IOSurface-backed 4:2:0 target.
//!
//! Each layer is one draw into the luma plane and one into the chroma plane, sampling the decoded
//! frame's planes in place. The target is whatever the consumer takes directly: the encoder's
//! `420v` buffers for export, `420f` buffers for gpui's preview surface.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::CVPixelBuffer;
use objc2_metal::MTLDevice;
use wgpu::hal::api::Metal;

use crate::RenderError;
use crate::layout::{Canvas, Scene, Sources};
use crate::surface::{Plane, Range, plane_texture, size};

/// Layers one frame can draw. 04 to 07 add overlays on top of the two sources.
const MAX_LAYERS: usize = 8;

/// Bytes per layer in the uniform buffer, the minimum dynamic offset alignment.
const LAYER_STRIDE: u64 = 256;

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    metal: Retained<ProtocolObject<dyn MTLDevice>>,
    layout: wgpu::BindGroupLayout,
    /// Indexed by [`Plane`]: luma, then chroma.
    pipelines: [wgpu::RenderPipeline; 2],
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
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
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
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
                        min_binding_size: wgpu::BufferSize::new(64),
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
        Ok(Self {
            device,
            queue,
            metal,
            layout,
            pipelines,
            sampler,
            uniforms,
        })
    }

    /// Draws `scene` into `target` and waits for the GPU, so the target can go straight to the
    /// encoder or the display. `sources` holds the decoded frame of each source a layer uses.
    pub fn render(
        &self,
        scene: &Scene,
        sources: Sources<Option<&CVPixelBuffer>>,
        target: &CVPixelBuffer,
    ) -> Result<(), RenderError> {
        if scene.layers.len() > MAX_LAYERS {
            return Err(RenderError::TooManyLayers(scene.layers.len()));
        }
        let target_range = Range::of(target)?;
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
                0.0,
                0.0,
                0.0,
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
                            size: wgpu::BufferSize::new(64),
                        }),
                    },
                ],
            });
            layers.push(Bound {
                group,
                offset: offset as u32,
            });
        }

        let background = ycbcr(scene.background, target_range);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        for plane in Plane::BOTH {
            let texture = plane_texture(
                &self.device,
                &self.metal,
                target,
                plane,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )?;
            let view = texture.create_view(&Default::default());
            let clear = match plane {
                Plane::Luma => [background[0], 0.0],
                Plane::Chroma => [background[1], background[2]],
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("compose"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear[0]),
                            g: f64::from(clear[1]),
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipelines[plane as usize]);
            for layer in &layers {
                pass.set_bind_group(0, &layer.group, &[layer.offset]);
                pass.draw(0..4, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Gpu(e.to_string()))?;
        Ok(())
    }
}

fn video_flag(range: Range) -> f32 {
    match range {
        Range::Video => 1.0,
        Range::Full => 0.0,
    }
}

/// BT.709 YCbCr of a gamma-encoded RGB color in `range`, matching the shader.
fn ycbcr([r, g, b]: [f32; 3], range: Range) -> [f32; 3] {
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let (cb, cr) = ((b - y) / 1.8556, (r - y) / 1.5748);
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
        assert_eq!(to_codes(ycbcr([1.0; 3], Range::Video)), [235, 128, 128]);
        assert_eq!(to_codes(ycbcr([0.0; 3], Range::Video)), [16, 128, 128]);
        assert_eq!(to_codes(ycbcr([1.0; 3], Range::Full)), [255, 128, 128]);
        // Pure red: Cr at its maximum.
        assert_eq!(
            to_codes(ycbcr([1.0, 0.0, 0.0], Range::Video)),
            [63, 102, 240]
        );
    }
}
