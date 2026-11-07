//! The render graph shared by preview and export, layouts, and incremental export.
//!
//! - [`layout`]: where each source sits in a layout, as plain geometry.
//! - [`graph`]: draws a frame's scene from decoded IOSurface planes into an IOSurface target.
//! - [`plan`]: an evaluated output snapped to the frame grid.
//! - [`cursor`]: the frame of a video track on screen at a time, decoding forward.
//! - [`audio`]: the mic audio of an output, cut and faded like its video.
//! - [`voice`]: that audio finished: voice chain, music bed, -14 LUFS.
//! - [`captions`]: word-timed captions with the caption fixes, and the WebVTT sidecar.
//! - [`export`]: renders changed segments in parallel and joins them into one MP4.

pub mod captions;
pub mod layout;
pub mod plan;

#[cfg(target_os = "macos")]
pub mod audio;
#[cfg(target_os = "macos")]
pub mod cursor;
#[cfg(target_os = "macos")]
pub mod export;
#[cfg(target_os = "macos")]
pub mod graph;
#[cfg(target_os = "macos")]
pub mod surface;
#[cfg(target_os = "macos")]
pub mod voice;

/// Why a frame could not be drawn.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no GPU for rendering: {0}")]
    Gpu(String),
    #[error("pixel format {0:#010x} is not biplanar 4:2:0")]
    PixelFormat(u32),
    #[error("a frame is not backed by an IOSurface")]
    NotIoSurface,
    #[error("creating a pixel buffer failed with CVReturn {0}")]
    Pool(i32),
    #[error("the scene uses the {0:?} source but no frame was given for it")]
    MissingSource(layout::Source),
    #[error("a frame draws at most 8 layers, this one has {0}")]
    TooManyLayers(usize),
}
