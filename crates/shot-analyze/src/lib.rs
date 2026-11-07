//! Transcription, VAD, auto-cut rules, faces, scene and idle detection.
//!
//! - [`models`]: pinned model downloads.
//! - [`Transcriber`]: streaming mic audio into a word-timed transcript.
//! - [`cache`]: the transcript cache file, its format is documented there.
//! - [`decode_mono`]: a recorded audio track as mono PCM.
//! - [`SpeechDetector`]: streaming mic audio into speech ranges and loudness levels.
//! - [`idle`]: stretches where the screen track does not change.
//! - [`again_markers`]: the `again` hotkey presses of a take's event log.
//! - [`autocut`]: the rough cut, rules and pacing written into `cut.kdl`.
//! - [`vision`]: faces and on-screen text with Apple Vision, for reframing and highlights.
//! - [`elements`]: the facts of a take in `elements/<take>.jsonl`, from all of the above.

mod audio;
pub mod autocut;
pub mod cache;
pub mod elements;
mod events;
pub mod idle;
pub mod models;
mod resample;
mod speech;
mod transcribe;
pub mod vision;

pub use audio::decode_mono;
pub use events::again_markers;
pub use speech::{LEVEL_STEP, Speech, SpeechDetector};
pub use transcribe::{Transcriber, normalize};

/// Errors of model downloads, decoding, transcription, the caches and the elements file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("download failed: {0}")]
    Http(String),
    #[error(
        "{file} failed verification: expected {expected}, got {actual}; the download was deleted"
    )]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("cannot decode audio: {0}")]
    Decode(String),
    #[error("transcription failed: {0}")]
    Whisper(String),
    #[error("bad transcript cache: {0}")]
    Cache(String),
    #[error(transparent)]
    Vision(#[from] vision::VisionError),
    #[error(transparent)]
    Parse(#[from] shot_core::ParseError),
}
