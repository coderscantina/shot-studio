//! The Edit screen's sound: the edit's finished audio as export makes it ([`crate::score`]),
//! voice chain, music, sound effects and loudness, played on the default output from a feeder
//! thread. While it plays, it is the preview's clock: the screen asks which output frame is
//! heard and shows that one.
//!
//! A seek or a new plan stops the device and drops what is queued, so the next sound heard is
//! the new position's. Starting after a seek fades in over 5 ms instead of clicking. Each plan
//! measures its loudness gain on a thread of its own; until it lands the last plan's gain plays.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use shot_media::audio::SAMPLE_RATE;
use shot_media::speaker::{Speaker, SpeakerClock};
use shot_render::audio::{FADE_SAMPLES, TimelineAudio};

use crate::model::{EditMap, FPS};
use crate::score::{self, Cues, Finished, Score, Sounds};

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
    /// The edit, where its takes live and what plays with the voice; `None` before the first
    /// plan.
    plan: Option<(EditMap, PathBuf, Cues)>,
    /// The loudness gain of the last measured plan, in dB.
    gain: Option<f64>,
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
                move || feed(shared)
            })
            .expect("spawning the sound thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Plays `map`'s audio, with takes in `media` and `cues` with the voice, from output frame
    /// `frame`.
    pub fn set_plan(&self, map: &EditMap, media: PathBuf, cues: Cues, frame: u64) {
        let mut state = self.shared.lock();
        state.plan = Some((map.clone(), media, cues));
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
fn feed(shared: Arc<Shared>) {
    let mut speaker = match Speaker::open() {
        Ok(speaker) => speaker,
        Err(error) => {
            shared.lock().error = Some(error.to_string());
            return;
        }
    };
    shared.lock().clock = Some(speaker.clock());
    let mut sounds = Sounds::default();
    let mut audio: Option<(TimelineAudio, Score)> = None;
    let mut finished: Option<Finished> = None;
    let measuring = Arc::new(AtomicU64::new(0));
    let mut plan_generation = 0;
    let mut generation = 0;
    // How far into the fade-in after a restart the sound is.
    let mut faded = FADE_SAMPLES;
    let mut block = Vec::with_capacity(BLOCK);
    loop {
        {
            let mut state = shared.lock();
            if state.quit {
                measuring.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if state.plan_generation != plan_generation {
                plan_generation = state.plan_generation;
                let plan = state.plan.clone();
                drop(state);
                let placed = plan.map(|(map, media, cues)| {
                    let score = Score::place(&map, &cues, &mut sounds)?;
                    measure(&shared, &measuring, plan_generation, &map, &media, &score);
                    Ok::<_, String>((score::timeline_audio(&map, media, &score), score))
                });
                state = shared.lock();
                audio = match placed.transpose() {
                    Ok(audio) => audio,
                    Err(error) => {
                        state.error = Some(error);
                        None
                    }
                };
                // The new plan plays from the position the next seek sets.
                finished = None;
            }
            if state.generation != generation || finished.is_none() {
                speaker.clear();
                generation = state.generation;
                let next = state.position;
                faded = 0;
                state.origin = next;
                state.applied = generation;
                finished = audio.as_ref().map(|(timeline, score)| {
                    Finished::new(timeline, score, state.gain.unwrap_or(0.0), next)
                });
            }
            if !state.playing || state.error.is_some() || audio.is_none() {
                speaker.stop();
                let _unused = shared.changed.wait(state).expect("sound state poisoned");
                continue;
            }
        }
        let (timeline, _) = audio.as_mut().expect("checked above");
        let finished = finished.as_mut().expect("made with the audio");
        while speaker.queued() < QUEUED {
            block.clear();
            if let Err(error) = finished.read(timeline, BLOCK, &mut block) {
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

/// Measures the plan's loudness gain on a thread of its own, with readers of its own. A newer
/// plan cancels it.
fn measure(
    shared: &Arc<Shared>,
    measuring: &Arc<AtomicU64>,
    generation: u64,
    map: &EditMap,
    media: &std::path::Path,
    score: &Score,
) {
    measuring.store(generation, Ordering::Relaxed);
    let (shared, measuring, score) = (shared.clone(), measuring.clone(), score.clone());
    let (map, media) = (map.clone(), media.to_owned());
    let spawned = std::thread::Builder::new()
        .name("shot-loudness".into())
        .spawn(move || {
            let mut audio = score::timeline_audio(&map, media, &score);
            if let Ok(Some(gain)) = score::gain(&mut audio, &score, &measuring, generation) {
                shared.lock().gain = Some(gain);
            }
        });
    // Without the thread the last plan's gain plays on.
    drop(spawned);
}
