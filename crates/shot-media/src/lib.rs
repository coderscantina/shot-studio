//! VideoToolbox decode and encode, fMP4 mux, FFmpeg fallback.

pub mod timing;

#[cfg(target_os = "macos")]
pub mod player;
#[cfg(target_os = "macos")]
pub mod videotoolbox;
