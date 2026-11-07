//! The Edit screen's sound: the edit's mic audio, cut and faded like the export, played on the
//! default output from a feeder thread. While it plays, it is the preview's clock: the screen
//! asks which output frame is heard and shows that one.
//!
//! A seek or a new plan stops the device and drops what is queued, so the next sound heard is
//! the new position's. Starting after a seek fades in over 5 ms instead of clicking.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use shot_media::audio::SAMPLE_RATE;
use shot_media::speaker::{Speaker, SpeakerClock};
use shot_render::audio::{AudioSegment, FADE_SAMPLES, TimelineAudio};

use crate::model::{EditMap, FPS};

const SAMPLES_PER_FRAME: u64 = (SAMPLE_RATE / FPS) as u64;
/// Samples decoded per read, about 21 ms.
const BLOCK: usize = 1024;
/// Queued ahead of the device while playing, 200 ms: enough to ride out a slow read.
const QUEUED: usize = SAMPLE_RATE as usize / 5;
/// The device starts once this much is queued, 50 ms.
const PREFILL: usize = SAMPLE_RATE as usize / 20;
/// How often the feeder tops up the queue while playing.
const FEED_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Default)]
struct State {
    /// The edit's segments and where its takes live; `None` before the first plan.
    plan: Option<(Vec<AudioSegment>, PathBuf)>,
    /// Counts plan changes, so the feeder rebuilds its reader.
    plan_generation: u64,
    /// Counts seeks and plan changes; the clock is only read for the one the feeder applied.
    generation: u64,
    applied: u64,
    /// The output sample the last seek asked for, and the one the device starts from after it.
    position: u64,
    origin: u64,
    playing: bool,
    clock: Option<SpeakerClock>,
    /// Why there is no sound. The screen then paces by wall clock.
    error: Option<String>,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("sound state poisoned")
    }
}

pub struct Sound {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Sound {
    pub fn spawn() -> Self {
        let shared = Arc::new(Shared::default());
        let thread = std::thread::Builder::new()
            .name("shot-sound".into())
            .spawn({
                let shared = shared.clone();
                move || feed(&shared)
            })
            .expect("spawning the sound thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Plays `map`'s audio, with takes in `media`, from output frame `frame`.
    pub fn set_plan(&self, map: &EditMap, media: PathBuf, frame: u64) {
        let segments = map
            .spans
            .iter()
            .map(|span| AudioSegment {
                take: span.take.clone(),
                source_frames: span.source_frames.clone(),
            })
            .collect();
        let mut state = self.shared.lock();
        state.plan = Some((segments, media));
        state.plan_generation += 1;
        state.error = None;
        Self::restart(&mut state, frame);
        self.shared.changed.notify_all();
    }

    pub fn seek(&self, frame: u64) {
        Self::restart(&mut self.shared.lock(), frame);
        self.shared.changed.notify_all();
    }

    fn restart(state: &mut State, frame: u64) {
        state.generation += 1;
        state.position = frame * SAMPLES_PER_FRAME;
    }

    pub fn set_playing(&self, playing: bool) {
        self.shared.lock().playing = playing;
        self.shared.changed.notify_all();
    }

    /// Whether sound plays and paces the preview. False without an output device or when a
    /// take's mic could not be read.
    pub fn is_clock(&self) -> bool {
        let state = self.shared.lock();
        state.clock.is_some() && state.error.is_none()
    }

    /// The output frame heard `ahead` from now, as a fraction of frames. `None` until the
    /// device plays from the last seek.
    pub fn heard(&self, ahead: Duration) -> Option<f64> {
        let state = self.shared.lock();
        if state.applied != state.generation || state.error.is_some() {
            return None;
        }
        let sample = state.origin + state.clock.as_ref()?.audible(ahead)?;
        Some(sample as f64 / SAMPLES_PER_FRAME as f64)
    }

    pub fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The feeder thread: owns the device and the mic readers, keeps [`QUEUED`] samples ahead.
fn feed(shared: &Shared) {
    let mut speaker = match Speaker::open() {
        Ok(speaker) => speaker,
        Err(error) => {
            shared.lock().error = Some(error.to_string());
            return;
        }
    };
    shared.lock().clock = Some(speaker.clock());
    let mut audio: Option<TimelineAudio> = None;
    let mut plan_generation = 0;
    let mut generation = 0;
    // The next output sample to read, and how far into the fade-in after a restart it is.
    let mut next = 0;
    let mut faded = FADE_SAMPLES;
    let mut block = Vec::with_capacity(BLOCK);
    loop {
        {
            let mut state = shared.lock();
            if state.quit {
                return;
            }
            if state.plan_generation != plan_generation {
                plan_generation = state.plan_generation;
                audio = state.plan.clone().map(|(segments, media)| {
                    TimelineAudio::new(segments, FPS, move |take| {
                        media.join(take.as_str()).join("mic.mp4")
                    })
                });
            }
            if state.generation != generation {
                speaker.clear();
                generation = state.generation;
                next = state.position;
                faded = 0;
                state.origin = next;
                state.applied = generation;
            }
            if !state.playing || state.error.is_some() || audio.is_none() {
                speaker.stop();
                let _unused = shared.changed.wait(state).expect("sound state poisoned");
                continue;
            }
        }
        let timeline = audio.as_mut().expect("checked above");
        while speaker.queued() < QUEUED {
            block.clear();
            if let Err(error) = timeline.read(next..next + BLOCK as u64, &mut block) {
                shared.lock().error = Some(error.to_string());
                break;
            }
            for sample in block.iter_mut() {
                if faded >= FADE_SAMPLES {
                    break;
                }
                *sample *= (faded as f32 + 0.5) / FADE_SAMPLES as f32;
                faded += 1;
            }
            speaker.push(&block);
            next += BLOCK as u64;
        }
        if speaker.queued() >= PREFILL
            && let Err(error) = speaker.play()
        {
            shared.lock().error = Some(error.to_string());
        }
        let state = shared.lock();
        if state.generation == generation && state.playing && !state.quit {
            let _unused = shared
                .changed
                .wait_timeout(state, FEED_INTERVAL)
                .expect("sound state poisoned");
        }
    }
}
