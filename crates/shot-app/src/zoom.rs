//! Punch-in zoom as one wgpu pass over the decoded frame, proving the render graph can share it.
//!
//! The decoded IOSurface's two planes become wgpu textures without a copy, the pass samples them
//! with a zoom transform into another IOSurface-backed buffer, and gpui shows that buffer.
//! Both planes are resampled separately, so the frame stays YCbCr and needs no color math.

use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetIOSurface,
    CVPixelBufferGetWidthOfPlane, CVPixelBufferPool, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor, MTLTextureUsage};
use shot_media::videotoolbox::{Frame, PIXEL_FORMAT};
use wgpu::hal::api::Metal;

/// The zoom factor shared with the UI, as `f32` bits. 1.0 skips the pass.
#[derive(Clone)]
pub struct ZoomControl(Arc<AtomicU32>);

impl ZoomControl {
    pub fn new(factor: f32) -> Self {
        Self(Arc::new(AtomicU32::new(factor.to_bits())))
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, factor: f32) {
        self.0.store(factor.to_bits(), Ordering::Relaxed);
    }
}

const SHADER: &str = r#"
struct Zoom { center: vec2<f32>, scale: f32, pad: f32 }
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(0) @binding(2) var<uniform> zoom: Zoom;

struct Varyings { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Varyings {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Varyings(vec4<f32>(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0), uv);
}

@fragment
fn fragment(input: Varyings) -> @location(0) vec4<f32> {
    return textureSample(source, linear, zoom.center + (input.uv - vec2(0.5)) / zoom.scale);
}
"#;

/// One plane of a biplanar 4:2:0 buffer as wgpu sees it.
struct Plane {
    index: usize,
    wgpu_format: wgpu::TextureFormat,
    metal_format: MTLPixelFormat,
}

const PLANES: [Plane; 2] = [
    Plane {
        index: 0,
        wgpu_format: wgpu::TextureFormat::R8Unorm,
        metal_format: MTLPixelFormat::R8Unorm,
    },
    Plane {
        index: 1,
        wgpu_format: wgpu::TextureFormat::Rg8Unorm,
        metal_format: MTLPixelFormat::RG8Unorm,
    },
];

pub struct ZoomStage {
    control: ZoomControl,
    device: wgpu::Device,
    queue: wgpu::Queue,
    metal: Retained<ProtocolObject<dyn MTLDevice>>,
    pipelines: [wgpu::RenderPipeline; 2],
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    pool: Option<(usize, usize, CFRetained<CVPixelBufferPool>)>,
}

impl ZoomStage {
    pub fn new(control: ZoomControl) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("a Metal adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("a wgpu device");
        // SAFETY: The backend is Metal, so the hal device is the Metal one.
        let metal = unsafe { device.as_hal::<Metal>() }
            .expect("wgpu runs on Metal")
            .raw_device()
            .clone();

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("punch-in zoom"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipelines = PLANES.map(|plane| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("punch-in zoom"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(plane.wgpu_format.into())],
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
            label: Some("zoom"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            control,
            device,
            queue,
            metal,
            pipelines,
            sampler,
            uniforms,
            pool: None,
        }
    }

    /// Returns the frame zoomed into a new buffer, or unchanged when the zoom is 1.
    pub fn process(&mut self, frame: Frame) -> Frame {
        let scale = self.control.get();
        if scale <= 1.0 {
            return frame;
        }
        let output = self.output_buffer(frame.width(), frame.height());
        let uniforms: Vec<u8> = [0.5f32, 0.5, scale, 0.0]
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        self.queue.write_buffer(&self.uniforms, 0, &uniforms);

        let mut encoder = self.device.create_command_encoder(&Default::default());
        for (plane, pipeline) in PLANES.iter().zip(&self.pipelines) {
            let source =
                self.plane_texture(&frame.buffer, plane, wgpu::TextureUsages::TEXTURE_BINDING);
            let target = self.plane_texture(&output, plane, wgpu::TextureUsages::RENDER_ATTACHMENT);
            let source_view = source.create_view(&Default::default());
            let target_view = target.create_view(&Default::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniforms.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("punch-in zoom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        // gpui samples the output on its own queue, so the pass must finish before handing it over.
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting for the zoom pass");
        Frame {
            pts: frame.pts,
            buffer: output,
        }
    }

    /// A wgpu texture over one plane of an IOSurface-backed buffer, sharing its memory.
    fn plane_texture(
        &self,
        buffer: &CVPixelBuffer,
        plane: &Plane,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        let width = CVPixelBufferGetWidthOfPlane(buffer, plane.index);
        let height = CVPixelBufferGetHeightOfPlane(buffer, plane.index);
        let surface =
            CVPixelBufferGetIOSurface(Some(buffer)).expect("decoded frames are IOSurfaces");
        let metal_usage = if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
            MTLTextureUsage::RenderTarget
        } else {
            MTLTextureUsage::ShaderRead
        };
        let texture = unsafe {
            let descriptor =
                MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                    plane.metal_format,
                    width,
                    height,
                    false,
                );
            descriptor.setUsage(metal_usage);
            self.metal
                .newTextureWithDescriptor_iosurface_plane(&descriptor, &surface, plane.index)
        }
        .expect("a Metal texture over the IOSurface plane");
        let size = wgpu::Extent3d {
            width: texture.width() as u32,
            height: texture.height() as u32,
            depth_or_array_layers: 1,
        };
        // SAFETY: The raw texture is a live 2D, single mip, single layer texture of this format and
        // size on wgpu's own Metal device, used only as the usage flags declare.
        unsafe {
            let hal = wgpu::hal::metal::Device::texture_from_raw(
                texture,
                plane.wgpu_format,
                objc2_metal::MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: size.width,
                    height: size.height,
                    depth: 1,
                },
                None,
            );
            self.device.create_texture_from_hal::<Metal>(
                hal,
                &wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: plane.wgpu_format,
                    usage,
                    view_formats: &[],
                },
                wgpu::wgt::TextureUses::UNINITIALIZED,
            )
        }
    }

    /// A buffer from a pool of IOSurface-backed frames in the decoder's format.
    fn output_buffer(&mut self, width: usize, height: usize) -> CFRetained<CVPixelBuffer> {
        if !matches!(&self.pool, Some((w, h, _)) if (*w, *h) == (width, height)) {
            self.pool = Some((width, height, create_pool(width, height)));
        }
        let (_, _, pool) = self.pool.as_ref().expect("pool created above");
        let mut buffer = ptr::null_mut();
        // SAFETY: The pool is live and the out pointer is writable.
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut buffer))
        };
        assert_eq!(status, 0, "allocating a zoom output buffer");
        // SAFETY: Create returns an owned buffer on success.
        unsafe { CFRetained::from_raw(NonNull::new(buffer).expect("pool returns a buffer")) }
    }
}

fn create_pool(width: usize, height: usize) -> CFRetained<CVPixelBufferPool> {
    let format = CFNumber::new_i32(PIXEL_FORMAT as i32);
    let width = CFNumber::new_isize(width as isize);
    let height = CFNumber::new_isize(height as isize);
    let no_surface_options = CFDictionary::<CFString, CFType>::empty();
    let metal: &CFType = CFBoolean::new(true);
    // SAFETY: The keys are constant CF strings and each value has the type CoreVideo documents.
    let attributes = unsafe {
        CFDictionary::<CFString, CFType>::from_slices(
            &[
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferWidthKey,
                kCVPixelBufferHeightKey,
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVPixelBufferMetalCompatibilityKey,
            ],
            &[&format, &width, &height, &no_surface_options, metal],
        )
    };
    let mut pool = ptr::null_mut();
    // SAFETY: The attributes dictionary is valid and the out pointer is writable.
    let status = unsafe {
        CVPixelBufferPool::create(
            None,
            None,
            Some(attributes.as_opaque()),
            NonNull::from(&mut pool),
        )
    };
    assert_eq!(status, 0, "creating the zoom buffer pool");
    // SAFETY: Create returns an owned pool on success.
    unsafe { CFRetained::from_raw(NonNull::new(pool).expect("CoreVideo returns a pool")) }
}
