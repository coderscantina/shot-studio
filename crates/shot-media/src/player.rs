//! A decode thread that keeps a few frames ahead of the screen and loops the clip.
//!
//! The UI thread pops due frames with [`Player::take_due`]; a seek drops the queued frames and
//! restarts decoding, and a newer seek cancels one still decoding.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::timing::{FrameIndex, take_due};
use crate::videotoolbox::{DecodeError, Decoder, Frame};

/// Frames decoded ahead while playing. 8 frames of 4K 4:2:0 are about 100 MB and cover 133 ms at 60 fps.
pub const RING_FRAMES: usize = 8;

/// Frames decoded ahead while paused. Keeping the decoder idle lets the next scrub seek start at once.
const PAUSED_FRAMES: usize = 1;

/// Runs on the decode thread for each frame before it is queued, for GPU effects.
pub type FrameStage = Box<dyn FnMut(Frame) -> Frame>;

/// What the decoder found when opening the file.
#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub index: FrameIndex,
    pub frame_duration: Duration,
    pub duration: Duration,
    pub hardware_decode: bool,
}

struct State {
    queue: VecDeque<Frame>,
    lookahead: usize,
    /// Counts seeks, so a frame decoded for an older seek is not queued.
    generation: u64,
    seek: Option<Duration>,
    quit: bool,
    error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            lookahead: RING_FRAMES,
            generation: 0,
            seek: None,
            quit: false,
            error: None,
        }
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("player state poisoned")
    }
}

pub struct Player {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    info: VideoInfo,
}

impl Player {
    /// Opens `path` on a new decode thread. `make_stage` runs once on that thread.
    pub fn spawn(
        path: PathBuf,
        make_stage: impl FnOnce() -> Option<FrameStage> + Send + 'static,
    ) -> Result<Self, DecodeError> {
        let shared = Arc::new(Shared::default());
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("shot-decode".into())
            .spawn({
                let shared = shared.clone();
                move || match Decoder::open(&path) {
                    Ok(decoder) => {
                        let info = VideoInfo {
                            index: decoder.index().clone(),
                            frame_duration: decoder.frame_duration(),
                            duration: decoder.duration(),
                            hardware_decode: decoder.uses_hardware(),
                        };
                        let _ = opened_tx.send(Ok(info));
                        decode_loop(decoder, make_stage(), &shared);
                    }
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                    }
                }
            })
            .expect("spawning the decode thread");
        let info = opened_rx.recv().expect("decode thread reports open")?;
        Ok(Self {
            shared,
            thread: Some(thread),
            info,
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Drops queued frames and decodes from `time`, clip time without loops.
    pub fn seek(&self, time: Duration) {
        let mut state = self.shared.lock();
        state.queue.clear();
        state.generation += 1;
        state.seek = Some(time);
        self.shared.changed.notify_all();
    }

    /// Decodes a full ring ahead while playing, and one frame while paused.
    pub fn set_playing(&self, playing: bool) {
        self.shared.lock().lookahead = if playing { RING_FRAMES } else { PAUSED_FRAMES };
        self.shared.changed.notify_all();
    }

    pub fn queued(&self) -> usize {
        self.shared.lock().queue.len()
    }

    pub fn error(&self) -> Option<String> {
        self.shared.lock().error.clone()
    }

    /// Pops frames due by `media_time` (stream time, growing across loops) and returns the newest,
    /// with the number of due frames that were never shown.
    pub fn take_due(&self, media_time: Duration) -> (Option<Frame>, usize) {
        let mut state = self.shared.lock();
        let taken = take_due(&mut state.queue, media_time, |frame| frame.pts);
        self.shared.changed.notify_all();
        taken
    }

    /// Pops the oldest queued frame regardless of time, for showing a seek result while paused.
    pub fn take_next(&self) -> Option<Frame> {
        let mut state = self.shared.lock();
        let frame = state.queue.pop_front();
        self.shared.changed.notify_all();
        frame
    }

    /// Presentation time of the oldest queued frame.
    pub fn front_pts(&self) -> Option<Duration> {
        self.shared.lock().queue.front().map(|frame| frame.pts)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn decode_loop(mut decoder: Decoder, mut stage: Option<FrameStage>, shared: &Shared) {
    let clip = decoder.duration();
    // Stream time of the clip start in the current loop pass.
    let mut loop_offset = Duration::ZERO;
    loop {
        let (generation, seek) = {
            let mut state = shared.lock();
            while !state.quit && state.seek.is_none() && state.queue.len() >= state.lookahead {
                state = shared.changed.wait(state).expect("player state poisoned");
            }
            if state.quit {
                return;
            }
            (state.generation, state.seek.take())
        };

        let decoded = match seek {
            Some(time) => {
                let clip_time = time.min(clip.saturating_sub(decoder.frame_duration()));
                loop_offset = Duration::ZERO;
                decoder.seek(clip_time, || shared.lock().seek.is_some())
            }
            None => match decoder.next_frame() {
                Ok(None) => {
                    loop_offset += clip;
                    decoder.seek(Duration::ZERO, || shared.lock().seek.is_some())
                }
                other => other,
            },
        };

        let mut frame = match decoded {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(error) => {
                shared.lock().error = Some(error.to_string());
                return;
            }
        };
        frame.pts += loop_offset;
        if let Some(stage) = stage.as_mut() {
            frame = stage(frame);
        }

        let mut state = shared.lock();
        if state.generation == generation && state.seek.is_none() {
            state.queue.push_back(frame);
            shared.changed.notify_all();
        }
    }
}
