//! IOSurface-backed pixel buffers as wgpu textures, sharing their memory, from the m02 spike.
//!
//! A biplanar 4:2:0 buffer becomes two textures: luma as `R8Unorm` and the interleaved chroma at
//! half size as `Rg8Unorm`. Decoded frames are read this way and render targets written this way,
//! so no pixel crosses to the CPU.

use std::ptr::{self, NonNull};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetIOSurface, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferGetWidthOfPlane, CVPixelBufferPool, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor, MTLTextureUsage};
use wgpu::hal::api::Metal;

use crate::RenderError;

/// The value range of a biplanar 4:2:0 buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Luma 16 to 235, chroma 16 to 240: `420v`, what video files store.
    Video,
    /// Every code value: `420f`, what gpui's surface element shows.
    Full,
}

impl Range {
    pub fn of(buffer: &CVPixelBuffer) -> Result<Self, RenderError> {
        #[allow(non_upper_case_globals)]
        match CVPixelBufferGetPixelFormatType(buffer) {
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => Ok(Self::Video),
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => Ok(Self::Full),
            other => Err(RenderError::PixelFormat(other)),
        }
    }

    pub fn pixel_format(self) -> u32 {
        match self {
            Self::Video => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            Self::Full => kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        }
    }
}

/// Which plane of a biplanar buffer, as wgpu and Metal see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    Luma,
    Chroma,
}

impl Plane {
    pub const BOTH: [Self; 2] = [Self::Luma, Self::Chroma];

    fn index(self) -> usize {
        match self {
            Self::Luma => 0,
            Self::Chroma => 1,
        }
    }

    pub fn format(self) -> wgpu::TextureFormat {
        match self {
            Self::Luma => wgpu::TextureFormat::R8Unorm,
            Self::Chroma => wgpu::TextureFormat::Rg8Unorm,
        }
    }

    fn metal_format(self) -> MTLPixelFormat {
        match self {
            Self::Luma => MTLPixelFormat::R8Unorm,
            Self::Chroma => MTLPixelFormat::RG8Unorm,
        }
    }
}

/// Pixel size of a buffer.
pub fn size(buffer: &CVPixelBuffer) -> (u32, u32) {
    (
        CVPixelBufferGetWidth(buffer) as u32,
        CVPixelBufferGetHeight(buffer) as u32,
    )
}

/// A wgpu texture over one plane of an IOSurface-backed buffer, sharing its memory. The texture
/// is valid while `buffer` is alive.
pub fn plane_texture(
    device: &wgpu::Device,
    metal: &ProtocolObject<dyn MTLDevice>,
    buffer: &CVPixelBuffer,
    plane: Plane,
    usage: wgpu::TextureUsages,
) -> Result<wgpu::Texture, RenderError> {
    let width = CVPixelBufferGetWidthOfPlane(buffer, plane.index());
    let height = CVPixelBufferGetHeightOfPlane(buffer, plane.index());
    let surface = CVPixelBufferGetIOSurface(Some(buffer)).ok_or(RenderError::NotIoSurface)?;
    let metal_usage = if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        MTLTextureUsage::RenderTarget
    } else {
        MTLTextureUsage::ShaderRead
    };
    let texture: Retained<ProtocolObject<dyn MTLTexture>> = unsafe {
        let descriptor =
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                plane.metal_format(),
                width,
                height,
                false,
            );
        descriptor.setUsage(metal_usage);
        metal.newTextureWithDescriptor_iosurface_plane(&descriptor, &surface, plane.index())
    }
    .ok_or(RenderError::NotIoSurface)?;
    let size = wgpu::Extent3d {
        width: width as u32,
        height: height as u32,
        depth_or_array_layers: 1,
    };
    // SAFETY: The raw texture is a live 2D, single mip, single layer texture of this format and
    // size on wgpu's own Metal device, used only as the usage flags declare.
    Ok(unsafe {
        let hal = wgpu::hal::metal::Device::texture_from_raw(
            texture,
            plane.format(),
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
        device.create_texture_from_hal::<Metal>(
            hal,
            &wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: plane.format(),
                usage,
                view_formats: &[],
            },
            wgpu::wgt::TextureUses::UNINITIALIZED,
        )
    })
}

/// A pool of IOSurface-backed biplanar 4:2:0 buffers to render into, for example a preview's
/// `420f` frames. Export renders into the encoder's own pool instead.
pub struct BufferPool {
    pool: CFRetained<CVPixelBufferPool>,
}

impl BufferPool {
    pub fn new(width: u32, height: u32, range: Range) -> Result<Self, RenderError> {
        let format = CFNumber::new_i32(range.pixel_format() as i32);
        let width = CFNumber::new_i32(width as i32);
        let height = CFNumber::new_i32(height as i32);
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
        match NonNull::new(pool) {
            // SAFETY: Create returns an owned pool on success.
            Some(pool) if status == 0 => Ok(Self {
                pool: unsafe { CFRetained::from_raw(pool) },
            }),
            _ => Err(RenderError::Pool(status)),
        }
    }

    pub fn buffer(&self) -> Result<CFRetained<CVPixelBuffer>, RenderError> {
        let mut buffer = ptr::null_mut();
        // SAFETY: The pool is live and the out pointer is writable.
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(None, &self.pool, NonNull::from(&mut buffer))
        };
        match NonNull::new(buffer) {
            // SAFETY: Create returns an owned buffer on success.
            Some(buffer) if status == 0 => Ok(unsafe { CFRetained::from_raw(buffer) }),
            _ => Err(RenderError::Pool(status)),
        }
    }
}
