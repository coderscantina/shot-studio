//! ScreenCaptureKit, AVFoundation camera and mic, and cursor events, recorded into one take
//! folder per session.
//!
//! [`Session::start`] creates `media/<take>/` in a video folder and writes each source to its
//! own fragmented MP4 as it arrives, plus `events.jsonl`:
//!
//! - `screen.mp4`: the display without the cursor, HEVC at display resolution, constant 60 fps
//! - `camera.mp4`: HEVC at the camera's format (the briefing's ProRes camera track is out for now)
//! - `mic.mp4`, `system.mp4`: AAC 48 kHz
//!
//! Video has a 0.5 s GOP and no B-frames. Each file is written as an init segment plus `moof`
//! and `mdat` fragments of about 0.5 s, appended as the encoder emits them, so a killed process
//! loses at most the last fragment. All tracks and the event log share one media clock that
//! starts at 0 when the session starts (see [`clock`]). Recording needs Screen Recording and
//! Input Monitoring permission, plus Camera and Microphone for those sources; a missing one is a
//! [`CaptureError::Permission`].
//!
//! [`Session::preview`] gives the Record screen the newest screen and camera frames as
//! IOSurface-backed `420f` buffers, converted and scaled in hardware on a thread of their own, so
//! the recording never waits for the preview.
//!
//! The live mic tap hands the mic samples to the caller as they arrive, for streaming
//! transcription:
//!
//! ```no_run
//! use shot_capture::{Session, SessionConfig};
//!
//! let mut config = SessionConfig::new("videos/friday-deploys");
//! config.mic_tap = Some(Box::new(|at, samples, rate| {
//!     // `at` is the media time of samples[0], `samples` is mono f32 at `rate` Hz.
//!     let _ = (at, samples, rate);
//! }));
//! let session = Session::start(config)?;
//! let take = session.stop()?;
//! println!("{}", take.id);
//! # Ok::<(), shot_capture::CaptureError>(())
//! ```

pub mod clock;
pub mod events;
pub mod take;

#[cfg(target_os = "macos")]
mod macos;

use std::path::PathBuf;

use shot_core::{Id, Micros};

#[cfg(target_os = "macos")]
pub use macos::{Preview, PreviewFrame, Session, list_devices};

/// Receives mic samples during a recording: the media time of the first sample, mono samples
/// and their sample rate. It runs on the mic's capture queue, so it must return quickly; hand
/// the samples to another thread for heavy work.
pub type MicTap = Box<dyn FnMut(Micros, &[f32], u32) + Send>;

/// What a session records.
pub struct SessionConfig {
    /// The video folder. The take goes into `media/<take>/` inside it.
    pub video_dir: PathBuf,
    /// CoreGraphics display id. `None` records the main display.
    pub display: Option<u32>,
    pub camera: Source,
    pub mic: Source,
    pub system_audio: bool,
    pub mic_tap: Option<MicTap>,
}

impl SessionConfig {
    /// The main display, default camera and mic, and system audio.
    pub fn new(video_dir: impl Into<PathBuf>) -> Self {
        Self {
            video_dir: video_dir.into(),
            display: None,
            camera: Source::Default,
            mic: Source::Default,
            system_audio: true,
            mic_tap: None,
        }
    }
}

/// Which camera or mic to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The system default device, skipped when there is none.
    Default,
    /// A device by its name or unique id. Missing is an error.
    Named(String),
    Off,
}

/// A finished take.
#[derive(Debug, Clone)]
pub struct Take {
    pub id: Id,
    pub dir: PathBuf,
    pub tracks: Vec<Track>,
    pub events: u64,
    pub duration: Micros,
}

/// One recorded file of a take.
#[derive(Debug, Clone)]
pub struct Track {
    pub file: &'static str,
    /// Sample buffers written.
    pub samples: u64,
    /// Sample buffers the encoder was not ready for.
    pub dropped: u64,
}

/// Devices a session can record.
#[derive(Debug, Clone, Default)]
pub struct Devices {
    pub displays: Vec<Display>,
    pub cameras: Vec<Device>,
    pub mics: Vec<Device>,
}

#[derive(Debug, Clone)]
pub struct Display {
    pub id: u32,
    pub width: usize,
    pub height: usize,
    pub refresh_hz: f64,
    pub main: bool,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub unique_id: String,
    pub default: bool,
}

/// The macOS privacy permissions capture needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    ScreenRecording,
    Camera,
    Microphone,
    InputMonitoring,
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ScreenRecording => "Screen & System Audio Recording",
            Self::Camera => "Camera",
            Self::Microphone => "Microphone",
            Self::InputMonitoring => "Input Monitoring",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error(
        "{0} permission is missing. Allow this app in System Settings > Privacy & Security > {0}, then start it again"
    )]
    Permission(Permission),
    #[error("no {kind} named {name:?}, see `shot record --list-devices`")]
    NoDevice { kind: &'static str, name: String },
    #[error("display {0} is not connected, see `shot record --list-devices`")]
    NoDisplay(u32),
    #[error("creating the take folder failed: {0}")]
    TakeDir(#[from] std::io::Error),
    #[error("{what} failed: {message}")]
    Source { what: &'static str, message: String },
    #[error("writing {file} failed: {message}")]
    Write { file: &'static str, message: String },
}
