//! Re-measures word times against the audio.
//!
//! Whisper's word times follow its attention, not the sound, so a word often carries the pause
//! after it. This pass measures RMS in 10 ms windows and moves each word's edges to where the
//! speech is:
//!
//! - quiet windows at the start and end of a word are trimmed,
//! - speech that runs on past an edge moves the edge out to where it stops, at most
//!   [`MAX_GROW_US`] and never past a neighbour: Whisper often starts a word after a pause
//!   late, and a cut there would leave the word's first sound behind,
//! - a quiet run of 200 ms or more inside a word ends the word where the run starts,
//! - a word never gets shorter than 60 ms.
//!
//! "Quiet" is relative to the audio given: the 10th percentile of the window levels estimates
//! the noise floor, and a window is quiet when it is under that floor plus [`MARGIN_DB`]. The
//! threshold never drops below [`MIN_THRESHOLD_DB`] (so digital silence does not make faint
//! noise count as speech) and never rises above [`MAX_BELOW_PEAK_DB`] under the 95th percentile
//! (so a noisy take does not turn speech into silence).

use std::path::Path;

use shot_core::{Micros, TimeRange, Transcript, Word};

use crate::{Error, decode_mono};

/// Samples per window at 16 kHz, 10 ms.
const WINDOW: usize = 160;
const WINDOW_US: u64 = 10_000;
/// A quiet run of this many windows inside a word ends it.
const GAP_WINDOWS: usize = 20;
const MIN_WORD_US: u64 = 60_000;
/// How far an edge may move out into the gap next to its word.
const MAX_GROW_US: u64 = 400_000;
const FLOOR_PERCENTILE: f32 = 0.10;
const PEAK_PERCENTILE: f32 = 0.95;
const MARGIN_DB: f32 = 10.0;
const MIN_THRESHOLD_DB: f32 = -60.0;
const MAX_BELOW_PEAK_DB: f32 = 20.0;
/// Level of a window of digital silence.
const SILENCE_DB: f32 = -100.0;

/// Moves the edges of `words` to the measured speech. `samples` are 16 kHz mono and start at
/// media time `origin`. Words must be in time order, and none may start before `after`, the end
/// of the word before them. The word after them is not known yet, so the last word's end does
/// not grow. Words outside the samples stay as they are.
pub fn refine(samples: &[f32], origin: Micros, after: Micros, words: &mut [Word]) {
    let until = words.last().map(|w| w.range.to());
    refine_levels(&levels(samples), origin, after, until, words);
}

/// Re-measures the words of a finished take against its whole mic track at `mic`, for
/// transcripts made before the edges could move out.
pub fn refine_track(mic: &Path, transcript: &Transcript) -> Result<Transcript, Error> {
    let mut levels = Vec::new();
    let mut window = Vec::with_capacity(WINDOW * 3);
    let mut size = 0;
    decode_mono(mic, |_, samples, rate| {
        size = (rate as usize / 100).max(1);
        for &sample in samples {
            window.push(sample);
            if window.len() == size {
                levels.push(level(&window));
                window.clear();
            }
        }
    })?;
    if !window.is_empty() {
        levels.push(level(&window));
    }
    let mut words = transcript.words().to_vec();
    refine_levels(&levels, Micros(0), Micros(0), None, &mut words);
    let mut out = Transcript::new(transcript.take().clone());
    for word in words {
        out.push(word)
            .expect("refined words stay in order and apart");
    }
    Ok(out)
}

/// See [`refine`]; `until` bounds the last word's end, `None` leaves it free.
fn refine_levels(
    levels: &[f32],
    origin: Micros,
    after: Micros,
    until: Option<Micros>,
    words: &mut [Word],
) {
    let Some(threshold) = threshold(levels) else {
        return;
    };
    let mut floor = after;
    for i in 0..words.len() {
        let ceiling = words.get(i + 1).map(|w| w.range.from()).or(until);
        let bounds = Bounds { floor, ceiling };
        if let Some(range) = measure(levels, threshold, origin, words[i].range, bounds) {
            words[i].range = range;
        }
        floor = words[i].range.to();
    }
}

/// How far a word's edges may move out: not before the end of the word before, and not past
/// the start of the word after.
struct Bounds {
    floor: Micros,
    ceiling: Option<Micros>,
}

/// RMS level in dB of each 10 ms window.
fn levels(samples: &[f32]) -> Vec<f32> {
    samples.chunks(WINDOW).map(level).collect()
}

fn level(chunk: &[f32]) -> f32 {
    let mean_square = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
    (10.0 * mean_square.log10()).max(SILENCE_DB)
}

fn threshold(levels: &[f32]) -> Option<f32> {
    if levels.is_empty() {
        return None;
    }
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let at = |p: f32| sorted[((sorted.len() - 1) as f32 * p) as usize];
    let floor = (at(FLOOR_PERCENTILE) + MARGIN_DB).max(MIN_THRESHOLD_DB);
    Some(floor.min(at(PEAK_PERCENTILE) - MAX_BELOW_PEAK_DB))
}

/// The speech of the word at `span`, or `None` when the span cannot be measured or holds no
/// speech. Edges inside the span trim to the speech, and speech running on past an edge grows
/// the word up to `bounds`.
fn measure(
    levels: &[f32],
    threshold: f32,
    origin: Micros,
    span: TimeRange,
    bounds: Bounds,
) -> Option<TimeRange> {
    if span.from() < origin {
        return None;
    }
    let window = |t: Micros| (t.0.saturating_sub(origin.0) / WINDOW_US) as usize;
    let time = |w: usize| Micros(origin.0 + w as u64 * WINDOW_US);
    let first = window(span.from());
    let end = span.to().0.saturating_sub(origin.0).div_ceil(WINDOW_US) as usize;
    let inside = levels.get(first..end.min(levels.len()))?;
    let loud = |l: &f32| *l >= threshold;

    let start = inside.iter().position(loud)?;
    let mut stop = start;
    let mut quiet = 0;
    for (i, level) in inside.iter().enumerate().skip(start) {
        if loud(level) {
            quiet = 0;
            stop = i + 1;
        } else {
            quiet += 1;
            if quiet >= GAP_WINDOWS {
                break;
            }
        }
    }

    let mut from = Micros(time(first + start).0.max(span.from().0));
    let mut to = Micros(time(first + stop).0.min(span.to().0));
    // A word that sounds from its first window may have started earlier, one that sounds to its
    // last may run on: follow the loud windows out into the gaps.
    if start == 0 {
        let lowest = Micros(
            bounds
                .floor
                .0
                .max(span.from().0.saturating_sub(MAX_GROW_US)),
        );
        let mut w = first;
        while w > 0 && time(w - 1) >= lowest && levels.get(w - 1).is_some_and(loud) {
            w -= 1;
        }
        from = from.min(time(w).max(lowest));
    }
    if first + stop >= end && quiet == 0 {
        let highest = Micros(span.to().0 + MAX_GROW_US);
        let highest = bounds.ceiling.map_or(highest, |c| highest.min(c));
        let mut w = end;
        while time(w + 1) <= highest && levels.get(w).is_some_and(loud) {
            w += 1;
        }
        to = to.max(time(w).min(highest));
    }
    let to = to.max(Micros((from.0 + MIN_WORD_US).min(span.to().0)));
    TimeRange::new(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: usize = 16_000;

    /// Tone bursts and silences as `(milliseconds, loud)` segments, with faint noise under the
    /// silences.
    fn audio(segments: &[(usize, bool)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(ms, loud) in segments {
            for i in 0..ms * RATE / 1000 {
                let tone = (i as f32 * 0.17).sin();
                out.push(if loud { 0.3 * tone } else { 0.0005 * tone });
            }
        }
        out
    }

    fn word(from_ms: u64, to_ms: u64) -> Word {
        Word {
            text: "word".into(),
            range: TimeRange::new(Micros(from_ms * 1000), Micros(to_ms * 1000)).unwrap(),
            confidence: 90,
        }
    }

    fn spans(words: &[Word]) -> Vec<(u64, u64)> {
        words
            .iter()
            .map(|w| (w.range.from().0 / 1000, w.range.to().0 / 1000))
            .collect()
    }

    #[test]
    fn trims_quiet_at_both_edges() {
        let samples = audio(&[(300, false), (400, true), (500, false)]);
        let mut words = [word(100, 1000)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(300, 700)]);
    }

    #[test]
    fn an_inner_gap_of_200_ms_ends_the_word() {
        let samples = audio(&[(300, true), (250, false), (300, true)]);
        let mut words = [word(0, 850)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 300)]);
    }

    #[test]
    fn a_shorter_gap_stays_inside_the_word() {
        let samples = audio(&[(300, true), (150, false), (300, true)]);
        let mut words = [word(0, 750)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 750)]);
    }

    #[test]
    fn a_word_keeps_60_ms_and_stays_inside_its_span() {
        let samples = audio(&[(500, false), (20, true), (500, false), (500, true)]);
        let mut words = [word(400, 700)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(500, 560)]);
    }

    #[test]
    fn a_late_start_moves_back_to_where_the_speech_starts() {
        let samples = audio(&[(200, true), (100, false), (700, true)]);
        let mut words = [word(0, 250), word(500, 1000)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 200), (300, 1000)]);
    }

    #[test]
    fn edges_grow_only_up_to_the_neighbours() {
        let samples = audio(&[(1000, true), (500, false)]);
        let mut words = [word(0, 400), word(500, 1000)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 500), (500, 1000)]);
    }

    #[test]
    fn neighbours_stay_in_order_and_apart() {
        let samples = audio(&[(300, true), (400, false), (300, true), (200, false)]);
        let mut words = [word(0, 700), word(700, 1200)];
        refine(&samples, Micros(0), Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 300), (700, 1000)]);
    }
}
