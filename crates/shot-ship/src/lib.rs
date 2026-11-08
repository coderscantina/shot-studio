//! Before and while a video ships: `shot check` rules, script checks, thumbnail candidates, the
//! upload description, and `shot ship` itself, the YouTube upload and the release tag.
//!
//! - [`check`]: the rules of `shot check`, pure functions over measurements.
//! - [`script_check`]: hook length, spoken length, sources and visuals of `script.md`.
//! - [`spoken`]: the transcript in an output's time and its chapter marks.
//! - [`describe`]: title and description with word-accurate chapters.
//! - [`lock`]: a migrated video's `cut.lock` and the timeline export renders from it.
//! - [`ship`]: what ships where, rendering and checking it, uploading and tagging.
//! - [`youtube`] and [`oauth`]: the Data API upload and the desktop sign-in.
//! - [`record`]: what already shipped, in `.shot/ship/record.json`.
//! - [`diff`]: the line diff the Ship screen shows for `cut.kdl`.
//! - [`media`] (macOS): the measurements `shot check` reads from an export.
//! - [`thumbnails`] (macOS): candidate frames by expression strength.

pub mod check;
pub mod describe;
pub mod diff;
pub mod lock;
pub mod oauth;
pub mod record;
pub mod script_check;
pub mod ship;
pub mod spoken;
pub mod youtube;

#[cfg(target_os = "macos")]
pub mod media;
#[cfg(target_os = "macos")]
pub mod thumbnails;

pub use spoken::clock;
