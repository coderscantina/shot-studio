//! The render graph shared by preview and export, layouts, and incremental export.
//!
//! - [`layout`]: where each source sits in a layout, as plain geometry.
//! - [`frame`]: everything one output frame shows, with the product video effects of [`drawing`],
//!   [`zoom`], [`events`] and [`code_diff`], and the overlay templates of [`overlay`]. [`assets`]
//!   loads the files they draw from.
//! - [`autozoom`]: the auto-zoom rule, which writes zooms from the event log; [`rules`] runs it
//!   over a video folder.
//! - [`graph`]: draws a frame's scene from decoded IOSurface planes into an IOSurface target.
//! - [`lock`]: a migrated video's `cut.lock`, which styles its timeline.
//! - [`plan`]: an evaluated output snapped to the frame grid.
//! - [`cursor`]: the frame of a video track on screen at a time, decoding forward.
//! - [`audio`]: the mic audio of an output, cut and faded like its video.
//! - [`voice`]: that audio finished: voice chain, music bed, -14 LUFS.
//! - [`captions`]: word-timed captions with the caption fixes, and the WebVTT sidecar.
//! - [`export`]: renders changed chunks in parallel and joins them into one MP4.
//! - [`derived`]: Shorts and X clips drawn from the main cut's chunks, reframed and captioned.
//! - [`overlay_rules`]: link cards, highlights and b-roll slots written into `cut.kdl`.
//! - [`motion`]: springs, presets and choreography over media time; [`kinetic`] moves text on
//!   them, [`materials`] holds the light, parallax and motion blur rules that sheets, shadows and
//!   glass follow, and [`lottie`] checks, fills and draws Lottie templates.
//! - [`theme`]: what the brand gives the templates (preset, feel, colors, fonts, Lottie
//!   templates) and the frame (camera, stinger, logo), read from the lock; [`typeface`] holds the
//!   font files text draws in.
//! - [`join`]: the intro and outro around a migrated video's segments and the transitions at
//!   its chapter edges, placed in the plan; [`bookend`] draws a generated title, and [`paint`]
//!   draws any output frame for export and preview alike.

pub mod assets;
pub mod autozoom;
pub mod bookend;
pub mod captions;
pub mod code_diff;
pub mod drawing;
pub mod events;
pub mod frame;
pub mod join;
pub mod kinetic;
pub mod layout;
pub mod lock;
pub mod lottie;
pub mod materials;
pub mod motion;
pub mod overlay;
pub mod plan;
pub mod theme;
pub mod typeface;
pub mod zoom;

#[cfg(target_os = "macos")]
pub mod audio;
#[cfg(target_os = "macos")]
pub mod cursor;
#[cfg(target_os = "macos")]
pub mod derived;
#[cfg(target_os = "macos")]
pub mod export;
#[cfg(target_os = "macos")]
pub mod graph;
#[cfg(all(test, target_os = "macos"))]
mod join_reel;
#[cfg(target_os = "macos")]
mod material_pass;
#[cfg(target_os = "macos")]
pub mod overlay_rules;
#[cfg(target_os = "macos")]
pub mod paint;
#[cfg(all(test, target_os = "macos"))]
mod reel;
#[cfg(target_os = "macos")]
pub mod rules;
#[cfg(target_os = "macos")]
pub mod surface;
#[cfg(target_os = "macos")]
mod vector;
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
