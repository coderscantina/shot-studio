//! Transcription, VAD, auto-cut rules, faces, scene and idle detection.
//!
//! - [`models`]: pinned model downloads.
//! - [`Transcriber`]: streaming mic audio into a word-timed transcript.
//! - [`cache`]: the transcript cache file, its format is documented there.
//! - [`decode_mono`]: a recorded audio track as mono PCM.

mod audio;
pub mod cache;
pub mod models;
mod resample;
mod transcribe;

pub use audio::decode_mono;
pub use transcribe::{Transcriber, normalize};

/// Errors of model downloads, decoding, transcription and the transcript cache.
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
}
