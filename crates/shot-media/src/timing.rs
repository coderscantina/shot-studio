//! Frame timing without IO: where a seek starts decoding, which frame is due on screen.
//!
//! Host time is passed in as a `Duration` from any fixed origin, so tests control the clock.

use std::collections::VecDeque;
use std::time::Duration;

/// Presentation timestamps of a video track and which of them are keyframes.
#[derive(Debug, Clone, Default)]
pub struct FrameIndex {
    frames: Vec<Duration>,
    keyframes: Vec<Duration>,
}

/// What a seek decodes: from `keyframe` forward, showing the first frame at or after `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekPlan {
    pub keyframe: Duration,
    pub target: Duration,
}

impl FrameIndex {
    /// Builds the index from samples in decode order. Without B-frames this is presentation order.
    pub fn from_samples(samples: impl IntoIterator<Item = (Duration, bool)>) -> Self {
        let mut index = Self::default();
        for (pts, is_keyframe) in samples {
            index.frames.push(pts);
            if is_keyframe {
                index.keyframes.push(pts);
            }
        }
        index.frames.sort_unstable();
        index.keyframes.sort_unstable();
        index
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frames(&self) -> &[Duration] {
        &self.frames
    }

    pub fn keyframe_count(&self) -> usize {
        self.keyframes.len()
    }

    /// Snaps `time` to the frame showing at that time and finds the keyframe to decode from.
    pub fn plan(&self, time: Duration) -> Option<SeekPlan> {
        let target = *self.frames.get(at_or_before(&self.frames, time)?)?;
        let keyframe = *self.keyframes.get(at_or_before(&self.keyframes, target)?)?;
        Some(SeekPlan { keyframe, target })
    }
}

/// Index of the last element `<= time`, or the first element when `time` is before all of them.
fn at_or_before(sorted: &[Duration], time: Duration) -> Option<usize> {
    if sorted.is_empty() {
        return None;
    }
    Some(sorted.partition_point(|&t| t <= time).saturating_sub(1))
}

/// Maps host time to media time. Paused, it holds its position.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaybackClock {
    media_at_anchor: Duration,
    host_anchor: Option<Duration>,
}

impl PlaybackClock {
    pub fn is_playing(&self) -> bool {
        self.host_anchor.is_some()
    }

    pub fn media_time(&self, host_now: Duration) -> Duration {
        match self.host_anchor {
            Some(anchor) => self.media_at_anchor + host_now.saturating_sub(anchor),
            None => self.media_at_anchor,
        }
    }

    /// Starts or restarts playback so that `media` is shown at `host_now`.
    pub fn play_from(&mut self, media: Duration, host_now: Duration) {
        self.media_at_anchor = media;
        self.host_anchor = Some(host_now);
    }

    pub fn pause_at(&mut self, media: Duration) {
        self.media_at_anchor = media;
        self.host_anchor = None;
    }

    /// Moves media time by `seconds` from `host_now` on, forward or back.
    pub fn shift(&mut self, seconds: f64, host_now: Duration) {
        let media = self.media_time(host_now).as_secs_f64() + seconds;
        self.media_at_anchor = Duration::from_secs_f64(media.max(0.0));
        if self.is_playing() {
            self.host_anchor = Some(host_now);
        }
    }
}

/// Paces frames against the clock with their phase locked to the display.
///
/// Frames are picked half a refresh ahead of the clock, so frame boundaries fall halfway between
/// refreshes, where render jitter cannot make two frames due at once. When each frame spans a whole
/// number of refreshes, every newly shown frame pulls the clock a little toward that phase. A
/// "60 Hz" monitor that really runs at 59.95 Hz would otherwise drift a frame every 20 s against
/// 60 fps media and drop it; locked, playback follows the display 0.08 % slow and drops nothing.
#[derive(Debug, Clone, Copy)]
pub struct FramePacer {
    pub clock: PlaybackClock,
    lead: Duration,
    locked: bool,
    lock_allowed: bool,
}

impl FramePacer {
    /// Share of the phase error corrected per shown frame.
    const GAIN: f64 = 1.0 / 32.0;

    /// `lock_allowed` false paces by wall clock alone, for comparison.
    pub fn new(frame_duration: Duration, refresh_interval: Duration, lock_allowed: bool) -> Self {
        let mut pacer = Self {
            clock: PlaybackClock::default(),
            lead: Duration::ZERO,
            locked: false,
            lock_allowed,
        };
        pacer.set_refresh(frame_duration, refresh_interval);
        pacer
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Follows a change of display. The lock needs a frame to span a whole number of refreshes.
    pub fn set_refresh(&mut self, frame_duration: Duration, refresh_interval: Duration) {
        let refreshes = frame_duration.as_secs_f64() / refresh_interval.as_secs_f64();
        let whole = refreshes.round();
        self.lead = refresh_interval / 2;
        self.locked = self.lock_allowed && whole >= 1.0 && (refreshes / whole - 1.0).abs() <= 0.005;
    }

    /// Frames with a timestamp up to this are due at `host_now`.
    pub fn due(&self, host_now: Duration) -> Duration {
        self.clock.media_time(host_now) + self.lead
    }

    /// Reports that the frame at `pts` was newly put on screen at `host_now`.
    pub fn shown(&mut self, host_now: Duration, pts: Duration) {
        if !self.locked || !self.clock.is_playing() {
            return;
        }
        let phase = self.due(host_now).as_secs_f64() - pts.as_secs_f64();
        self.clock
            .shift((self.lead.as_secs_f64() - phase) * Self::GAIN, host_now);
    }
}

/// Pops every frame that is due by `media_time` and returns the newest one,
/// with the count of due frames that were passed over and never shown.
pub fn take_due<T>(
    queue: &mut VecDeque<T>,
    media_time: Duration,
    pts: impl Fn(&T) -> Duration,
) -> (Option<T>, usize) {
    let mut newest = None;
    let mut popped: usize = 0;
    while queue.front().is_some_and(|frame| pts(frame) <= media_time) {
        newest = queue.pop_front();
        popped += 1;
    }
    (newest, popped.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    /// 10 frames at 100 ms, keyframes every 4 frames.
    fn index() -> FrameIndex {
        FrameIndex::from_samples((0..10).map(|i| (ms(i * 100), i % 4 == 0)))
    }

    #[test]
    fn seek_snaps_to_the_frame_on_screen_and_decodes_from_the_keyframe_before_it() {
        let index = index();
        assert_eq!(
            index.plan(ms(650)),
            Some(SeekPlan {
                keyframe: ms(400),
                target: ms(600)
            })
        );
        assert_eq!(
            index.plan(ms(800)),
            Some(SeekPlan {
                keyframe: ms(800),
                target: ms(800)
            })
        );
        assert_eq!(index.plan(ms(5000)).map(|plan| plan.target), Some(ms(900)));
        assert_eq!(FrameIndex::default().plan(ms(0)), None);
    }

    #[test]
    fn clock_advances_with_host_time_only_while_playing() {
        let mut clock = PlaybackClock::default();
        clock.play_from(ms(2000), ms(10_000));
        assert_eq!(clock.media_time(ms(10_250)), ms(2250));
        clock.pause_at(ms(2250));
        assert_eq!(clock.media_time(ms(99_000)), ms(2250));
    }

    /// Plays 60 fps media for `minutes` on a display refreshing every `refresh`, with up to 1 ms
    /// of render jitter and a missed render every 1000. Returns frames shown, dropped, and renders missed.
    fn simulate(refresh: Duration, nominal: Duration, minutes: u64, lock: bool) -> (u64, u64, u64) {
        let frame = Duration::from_nanos(16_666_667);
        let mut pacer = FramePacer::new(frame, nominal, lock);
        pacer.clock.play_from(Duration::ZERO, Duration::ZERO);
        let mut queue = VecDeque::new();
        let mut next_pts = Duration::ZERO;
        let (mut shown, mut dropped) = (0, 0);
        let renders = minutes * 60 * 1_000_000_000 / refresh.as_nanos() as u64;
        let missed = renders / 1000;
        for render in 0..renders {
            if render % 1000 == 999 {
                continue;
            }
            while queue.len() < 8 {
                queue.push_back(next_pts);
                next_pts += frame;
            }
            let jitter = Duration::from_micros((render * 7919) % 1000);
            let now = Duration::from_nanos(refresh.as_nanos() as u64 * render) + jitter;
            let (frame, skipped) = take_due(&mut queue, pacer.due(now), |&t| t);
            dropped += skipped as u64;
            if let Some(pts) = frame {
                shown += 1;
                pacer.shown(now, pts);
            }
        }
        (shown, dropped, missed)
    }

    #[test]
    fn a_locked_pacer_drops_nothing_on_a_display_slightly_off_60_hz() {
        let nominal = Duration::from_nanos(16_666_667);
        let real = Duration::from_nanos(16_680_567); // 59.95 Hz
        let (_, dropped, missed) = simulate(real, nominal, 10, true);
        // A missed render costs the frame it would have shown, and nothing more.
        assert_eq!(dropped, missed);
        let (_, unlocked, _) = simulate(real, nominal, 10, false);
        assert!(
            unlocked > missed + 20,
            "wall clock pacing drifts: {unlocked}"
        );
    }

    #[test]
    fn a_locked_pacer_shows_every_frame_at_120_hz() {
        let refresh = Duration::from_nanos(8_333_333);
        let (shown, dropped, missed) = simulate(refresh, refresh, 1, true);
        // At two refreshes per frame a missed render costs nothing.
        assert!(missed > 0);
        assert_eq!(dropped, 0);
        assert!(shown >= 3590, "shown {shown}");
    }

    #[test]
    fn take_due_shows_the_newest_due_frame_and_counts_the_skipped_ones() {
        let mut queue: VecDeque<Duration> = (0..5).map(|i| ms(i * 16)).collect();
        assert_eq!(take_due(&mut queue, ms(10), |&t| t), (Some(ms(0)), 0));
        assert_eq!(take_due(&mut queue, ms(10), |&t| t), (None, 0));
        assert_eq!(take_due(&mut queue, ms(50), |&t| t), (Some(ms(48)), 2));
        assert_eq!(queue, [ms(64)]);
    }
}
