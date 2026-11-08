//! The recording session on macOS.

mod av;
mod event_tap;
mod screen;
mod sink;
mod tap;
mod writer;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use objc2::rc::Retained;
use objc2_core_graphics::{CGGetActiveDisplayList, CGMainDisplayID};
use objc2_core_media::{CMClock, CMSampleBuffer, CMSyncConvertTime};
use shot_core::{Id, Micros};

use self::av::{AvCapture, Kind};
use self::event_tap::EventTap;
pub use self::event_tap::Marks;
use self::screen::Screen;
use self::sink::Handler;
use self::tap::Sources;
pub use self::tap::{Preview, PreviewFrame};
use self::writer::{Spec, Track};
use crate::clock::nanos;
use crate::{
    Access, CaptureError, Devices, Display, Permission, SessionConfig, Source, Take, peak_dbfs,
};

/// HEVC bits per pixel per frame. Screen text needs more than camera footage.
const SCREEN_BPP: f64 = 0.1;
const CAMERA_BPP: f64 = 0.08;
const CAMERA_FPS: u32 = 30;
/// The screen's frame rate before a take: enough to check it, at half the cost.
const STANDBY_FPS: u32 = 30;

/// A recording in progress. Dropping it without [`Session::stop`] leaves the files as a crash
/// would: playable up to the last fragment.
pub struct Session {
    id: Id,
    dir: std::path::PathBuf,
    started: Instant,
    /// The display stream, while the screen or system audio records.
    screen: Option<Screen>,
    av: Option<AvCapture>,
    events: EventTap,
    tracks: Vec<Arc<Track>>,
    frames: Sources,
}

impl Session {
    /// Checks permissions and devices, creates the take folder and starts recording. A session
    /// records at least one track.
    pub fn start(config: SessionConfig) -> Result<Self, CaptureError> {
        let display_stream = config.screen || config.system_audio;
        if display_stream {
            screen::check_permission()?;
        }
        event_tap::check_permission()?;
        let camera = av::resolve(Kind::Camera, &config.camera)?;
        let mic = av::resolve(Kind::Mic, &config.mic)?;
        if camera.is_some() {
            av::authorize(Kind::Camera)?;
        }
        if mic.is_some() {
            av::authorize(Kind::Mic)?;
        }
        if !display_stream && camera.is_none() && mic.is_none() {
            return Err(CaptureError::NothingToRecord);
        }
        let display = screen::display_id(config.display);
        let geometry = screen::geometry(display).ok_or(CaptureError::NoDisplay(display))?;

        let (id, dir) = crate::take::create_take_dir(&config.video_dir)?;
        let track = |file, spec| Arc::new(Track::new(&dir, file, spec));
        let screen_track = config.screen.then(|| {
            track(
                "screen.mp4",
                Spec::Video {
                    fps: screen::FPS,
                    bits_per_pixel: SCREEN_BPP,
                },
            )
        });
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
        let screen_tracks: Vec<_> = screen_track.iter().chain(&system_track).cloned().collect();
        let av_tracks: Vec<_> = camera
            .iter()
            .map(|(_, t, _)| t.clone())
            .chain(mic.iter().map(|(_, t, _)| t.clone()))
            .collect();
        let camera = camera.map(|(device, track, latest)| {
            let handler: Handler = Box::new(move |sample: &CMSampleBuffer| {
                if let Some(image) = unsafe { sample.image_buffer() } {
                    latest.store(image);
                }
                track.push(sample)
            });
            (device, handler)
        });
        let mic = mic.map(|(device, track, tap)| {
            let tap = tap.map(Mutex::new);
            let handler: Handler = Box::new(move |sample: &CMSampleBuffer| {
                track.push(sample);
                if let (Some(tap), Some(clock)) = (&tap, track.clock()) {
                    let pts = unsafe { sample.presentation_time_stamp() };
                    if let (Some(at), Some((samples, rate))) = (
                        clock.media_time(nanos(pts.value, pts.timescale)),
                        av::mono(sample),
                    ) {
                        (tap.lock().expect("mic tap poisoned"))(at, &samples, rate);
                    }
                }
            });
            (device, handler)
        });

        let started = (|| {
            let mut screen = display_stream
                .then(|| Screen::start(display, screen_track, system_track, frames.screen.clone()))
                .transpose()?;
            let av = match (camera.is_some(), mic.is_some()) {
                (false, false) => None,
                _ => match AvCapture::start(camera, mic) {
                    Ok(av) => Some(av),
                    Err(e) => {
                        if let Some(screen) = &mut screen {
                            screen.stop();
                        }
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
        if let Some(screen) = &mut screen {
            let screen_origin = in_clock(screen.clock());
            screen_tracks
                .iter()
                .for_each(|t| t.set_origin(screen_origin));
            screen.pace(origin_instant);
        }
        if let Some(av) = &av {
            let av_origin = in_clock(av.clock());
            av_tracks.iter().for_each(|t| t.set_origin(av_origin));
        }

        let events = match EventTap::start(
            &dir.join("events.jsonl"),
            crate::clock::nanos(origin.value, origin.timescale),
            geometry,
            config.hotkeys,
        ) {
            Ok(events) => events,
            Err(e) => {
                if let Some(screen) = &mut screen {
                    screen.stop();
                }
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

    /// Writes record hotkey markers into this take's `events.jsonl`.
    pub fn marks(&self) -> Marks {
        self.events.marks()
    }

    /// Media time recorded so far.
    pub fn elapsed(&self) -> Micros {
        Micros(self.started.elapsed().as_micros() as u64)
    }

    /// Stops every source and finishes the files.
    pub fn stop(mut self) -> Result<Take, CaptureError> {
        let duration = self.elapsed();
        if let Some(screen) = &mut self.screen {
            screen.stop();
        }
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

/// The selected sources running without recording, so the Record screen can show them before
/// a take: the screen at [`STANDBY_FPS`] and scaled to the preview's size, the camera, and the
/// mic's peak level. Nothing is written. A source that fails is left out and its error kept, so
/// a denied camera still shows the screen. Dropping it stops every source.
///
/// A [`Session`] can start while a standby runs on the same devices, so the preview can hand
/// over to the recording without a gap.
pub struct Standby {
    screen: Option<screen::Stream>,
    av: Option<AvCapture>,
    frames: Sources,
    errors: Vec<CaptureError>,
}

impl Standby {
    /// Starts `display` (the main one for `None`, none at all without `screen`), `camera` and
    /// `mic`. Asks for camera and mic access when macOS has not asked yet, which waits for the
    /// answer. `on_peak` gets the mic's peak of each buffer in dBFS, on the mic's capture queue.
    pub fn start(
        screen: bool,
        display: Option<u32>,
        camera: &Source,
        mic: &Source,
        on_peak: impl Fn(f32) + Send + Sync + 'static,
    ) -> Self {
        let frames = Sources::default();
        let mut errors = Vec::new();
        let screen = screen.then(|| {
            screen::check_permission()?;
            let display = screen::display_id(display);
            let (width, height, _) =
                screen::pixel_mode(display).ok_or(CaptureError::NoDisplay(display))?;
            let size = tap::fit(width, height, tap::SCREEN_MAX);
            screen::Stream::start(display, size, STANDBY_FPS, frames.screen.clone(), None)
        });
        let screen = screen.and_then(|screen| screen.map_err(|e| errors.push(e)).ok());
        let mut device = |kind, source| {
            av::resolve(kind, source)
                .and_then(|device| {
                    if device.is_some() {
                        av::authorize(kind)?;
                    }
                    Ok(device)
                })
                .map_err(|e| errors.push(e))
                .ok()
                .flatten()
        };
        let camera = device(Kind::Camera, camera).map(|device| {
            let latest = frames.camera.clone();
            let handler: Handler = Box::new(move |sample: &CMSampleBuffer| {
                if let Some(image) = unsafe { sample.image_buffer() } {
                    latest.store(image);
                }
            });
            (device, handler)
        });
        let mic = device(Kind::Mic, mic).map(|device| {
            let handler: Handler = Box::new(move |sample: &CMSampleBuffer| {
                if let Some((samples, _)) = av::mono(sample) {
                    on_peak(peak_dbfs(&samples));
                }
            });
            (device, handler)
        });
        let av = match (camera.is_some(), mic.is_some()) {
            (false, false) => None,
            _ => AvCapture::start(camera, mic)
                .map_err(|e| errors.push(e))
                .ok(),
        };
        Self {
            screen,
            av,
            frames,
            errors,
        }
    }

    /// A live preview of the screen and the camera, like [`Session::preview`].
    pub fn preview(&self) -> Preview {
        Preview::start(self.frames.clone())
    }

    /// Why a source did not start.
    pub fn errors(&self) -> &[CaptureError] {
        &self.errors
    }
}

impl Drop for Standby {
    fn drop(&mut self) {
        if let Some(screen) = &self.screen {
            screen.stop();
        }
        if let Some(av) = &self.av {
            av.stop();
        }
    }
}

/// Whether the app may use what `permission` guards, without asking.
pub fn access(permission: Permission) -> Access {
    let allowed = |yes| if yes { Access::Allowed } else { Access::Denied };
    match permission {
        Permission::ScreenRecording => screen::access(),
        Permission::InputMonitoring => allowed(event_tap::allowed()),
        Permission::Camera => av::access(Kind::Camera),
        Permission::Microphone => av::access(Kind::Mic),
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
