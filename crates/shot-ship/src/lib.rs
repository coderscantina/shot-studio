//! Before a video ships: `shot check` rules, script checks, thumbnail candidates and the upload
//! description. The YouTube upload and release tag of `shot ship` come with milestone 07's
//! derived cuts.
//!
//! - [`check`]: the rules of `shot check`, pure functions over measurements.
//! - [`script_check`]: hook length, spoken length, sources and visuals of `script.md`.
//! - [`spoken`]: the transcript in an output's time and its chapter marks.
//! - [`describe`]: title and description with word-accurate chapters.
//! - [`media`] (macOS): the measurements `shot check` reads from an export.
//! - [`thumbnails`] (macOS): candidate frames by expression strength.

pub mod check;
pub mod describe;
pub mod script_check;
pub mod spoken;

#[cfg(target_os = "macos")]
pub mod media;
#[cfg(target_os = "macos")]
pub mod thumbnails;

pub use spoken::clock;
