//! VideoToolbox decode and encode, fMP4 mux, FFmpeg fallback.

pub mod mux;
pub mod timing;

#[cfg(target_os = "macos")]
pub mod audio;
#[cfg(target_os = "macos")]
pub mod encode;
#[cfg(target_os = "macos")]
pub mod player;
#[cfg(target_os = "macos")]
pub mod videotoolbox;
