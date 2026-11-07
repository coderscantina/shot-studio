//! The recording session on macOS.

mod av;
mod event_tap;
mod screen;
mod sink;
mod tap;
mod writer;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2_core_graphics::{CGGetActiveDisplayList, CGMainDisplayID};
use objc2_core_media::{CMClock, CMSyncConvertTime};
use shot_core::{Id, Micros};

use self::av::{AvCapture, Kind};
use self::event_tap::EventTap;
use self::screen::Screen;
use self::tap::Sources;
pub use self::tap::{Preview, PreviewFrame};
use self::writer::{Spec, Track};
use crate::{CaptureError, Devices, Display, SessionConfig, Take};

/// HEVC bits per pixel per frame. Screen text needs more than camera footage.
const SCREEN_BPP: f64 = 0.1;
const CAMERA_BPP: f64 = 0.08;
const CAMERA_FPS: u32 = 30;

/// A recording in progress. Dropping it without [`Session::stop`] leaves the files as a crash
/// would: playable up to the last fragment.
pub struct Session {
    id: Id,
    dir: std::path::PathBuf,
    started: Instant,
    screen: Screen,
    av: Option<AvCapture>,
    events: EventTap,
    tracks: Vec<Arc<Track>>,
    frames: Sources,
}

impl Session {
    /// Checks permissions and devices, creates the take folder and starts recording.
    pub fn start(config: SessionConfig) -> Result<Self, CaptureError> {
        screen::check_permission()?;
        event_tap::check_permission()?;
        let camera = av::resolve(Kind::Camera, &config.camera)?;
        let mic = av::resolve(Kind::Mic, &config.mic)?;
        if camera.is_some() {
            av::authorize(Kind::Camera)?;
        }
        if mic.is_some() {
            av::authorize(Kind::Mic)?;
        }
        let display = screen::display_id(config.display);
        screen::pixel_mode(display).ok_or(CaptureError::NoDisplay(display))?;

        let (id, dir) = crate::take::create_take_dir(&config.video_dir)?;
        let track = |file, spec| Arc::new(Track::new(&dir, file, spec));
        let screen_track = track(
            "screen.mp4",
            Spec::Video {
                fps: screen::FPS,
                bits_per_pixel: SCREEN_BPP,
            },
        );
        let system_track = config.system_audio.then(|| {
            track(
                "system.mp4",
                Spec::Audio {
                    channels: 2,
                    bitrate: 192_000,
                },
            )
        });
        let frames = Sources::default();
        let camera = camera.map(|device| {
            let spec = Spec::Video {
                fps: CAMERA_FPS,
                bits_per_pixel: CAMERA_BPP,
            };
            (device, track("camera.mp4", spec), frames.camera.clone())
        });
        let mic = mic.map(|device| {
            let spec = Spec::Audio {
                channels: 1,
                bitrate: 128_000,
            };
            (device, track("mic.mp4", spec), config.mic_tap)
        });
        let screen_tracks: Vec<_> = std::iter::once(screen_track.clone())
            .chain(system_track.clone())
            .collect();
        let av_tracks: Vec<_> = camera
            .iter()
            .map(|(_, t, _)| t.clone())
            .chain(mic.iter().map(|(_, t, _)| t.clone()))
            .collect();

        let started = (|| {
            let mut screen =
                Screen::start(display, screen_track, system_track, frames.screen.clone())?;
            let av = match (camera.is_some(), mic.is_some()) {
                (false, false) => None,
                _ => match AvCapture::start(camera, mic) {
                    Ok(av) => Some(av),
                    Err(e) => {
                        screen.stop();
                        return Err(e);
                    }
                },
            };
            Ok((screen, av))
        })();
        let (mut screen, av) = match started {
            Ok(sources) => sources,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };

        // Media time zero: now on the host clock, expressed in each source's clock.
        let host = unsafe { CMClock::host_time_clock() };
        let origin = unsafe { host.time() };
        let origin_instant = Instant::now();
        let in_clock = |clock: Option<Retained<CMClock>>| match clock {
            Some(clock) => unsafe { CMSyncConvertTime(origin, &host, &clock) },
            None => origin,
        };
        let screen_origin = in_clock(screen.clock());
        screen_tracks
            .iter()
            .for_each(|t| t.set_origin(screen_origin));
        screen.pace(origin_instant);
        if let Some(av) = &av {
            let av_origin = in_clock(av.clock());
            av_tracks.iter().for_each(|t| t.set_origin(av_origin));
        }

        let events = match EventTap::start(
            &dir.join("events.jsonl"),
            crate::clock::nanos(origin.value, origin.timescale),
            screen.geometry,
        ) {
            Ok(events) => events,
            Err(e) => {
                screen.stop();
                if let Some(av) = &av {
                    av.stop();
                }
                return Err(e);
            }
        };

        Ok(Self {
            id,
            dir,
            started: origin_instant,
            screen,
            av,
            events,
            tracks: screen_tracks.into_iter().chain(av_tracks).collect(),
            frames,
        })
    }

    pub fn id(&self) -> &Id {
        &self.id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The track files this session records, without `events.jsonl`.
    pub fn files(&self) -> Vec<&'static str> {
        self.tracks.iter().map(|t| t.file).collect()
    }

    /// A live preview of the screen and the camera. Each call starts its own conversion thread,
    /// so take it once and clone it.
    pub fn preview(&self) -> Preview {
        Preview::start(self.frames.clone())
    }

    /// Media time recorded so far.
    pub fn elapsed(&self) -> Micros {
        Micros(self.started.elapsed().as_micros() as u64)
    }

    /// Stops every source and finishes the files.
    pub fn stop(mut self) -> Result<Take, CaptureError> {
        let duration = self.elapsed();
        self.screen.stop();
        if let Some(av) = &self.av {
            av.stop();
        }
        let events = self.events.stop()?;
        let tracks = self
            .tracks
            .iter()
            .map(|t| t.finish())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Take {
            id: self.id,
            dir: self.dir,
            tracks,
            events,
            duration,
        })
    }
}

/// Displays, cameras and mics a session can record. Needs no permission.
pub fn list_devices() -> Devices {
    let mut ids = [0u32; 16];
    let mut count = 0;
    unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    let main = CGMainDisplayID();
    let displays = ids[..count as usize]
        .iter()
        .filter_map(|&id| {
            let (width, height, refresh_hz) = screen::pixel_mode(id)?;
            Some(Display {
                id,
                width,
                height,
                refresh_hz,
                main: id == main,
            })
        })
        .collect();
    Devices {
        displays,
        cameras: av::describe(Kind::Camera),
        mics: av::describe(Kind::Mic),
    }
}
