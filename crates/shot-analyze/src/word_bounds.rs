//! Re-measures word times against the audio.
//!
//! Whisper's word times follow its attention, not the sound, so a word often carries the pause
//! after it. This pass measures RMS in 10 ms windows and moves each word's edges to where the
//! speech is:
//!
//! - quiet windows at the start and end of a word are trimmed,
//! - a quiet run of 200 ms or more inside a word ends the word where the run starts,
//! - a word never gets shorter than 60 ms and never leaves its own span, so it cannot cross a
//!   neighbour.
//!
//! "Quiet" is relative to the audio given: the 10th percentile of the window levels estimates
//! the noise floor, and a window is quiet when it is under that floor plus [`MARGIN_DB`]. The
//! threshold never drops below [`MIN_THRESHOLD_DB`] (so digital silence does not make faint
//! noise count as speech) and never rises above [`MAX_BELOW_PEAK_DB`] under the 95th percentile
//! (so a noisy take does not turn speech into silence).

use shot_core::{Micros, TimeRange, Word};

/// Samples per window at 16 kHz, 10 ms.
const WINDOW: usize = 160;
const WINDOW_US: u64 = 10_000;
/// A quiet run of this many windows inside a word ends it.
const GAP_WINDOWS: usize = 20;
const MIN_WORD_US: u64 = 60_000;
const FLOOR_PERCENTILE: f32 = 0.10;
const PEAK_PERCENTILE: f32 = 0.95;
const MARGIN_DB: f32 = 10.0;
const MIN_THRESHOLD_DB: f32 = -60.0;
const MAX_BELOW_PEAK_DB: f32 = 20.0;
/// Level of a window of digital silence.
const SILENCE_DB: f32 = -100.0;

/// Moves the edges of `words` to the measured speech. `samples` are 16 kHz mono and start at
/// media time `origin`. Words must be in time order. Words outside the samples stay as they are.
pub fn refine(samples: &[f32], origin: Micros, words: &mut [Word]) {
    if words.is_empty() {
        return;
    }
    let levels = levels(samples);
    let Some(threshold) = threshold(&levels) else {
        return;
    };
    for word in words {
        if let Some(range) = measure(&levels, threshold, origin, word.range) {
            word.range = range;
        }
    }
}

/// RMS level in dB of each 10 ms window.
fn levels(samples: &[f32]) -> Vec<f32> {
    samples
        .chunks(WINDOW)
        .map(|chunk| {
            let mean_square = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            (10.0 * mean_square.log10()).max(SILENCE_DB)
        })
        .collect()
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

/// The speech inside `span`, or `None` when the span cannot be measured or holds no speech.
fn measure(levels: &[f32], threshold: f32, origin: Micros, span: TimeRange) -> Option<TimeRange> {
    if span.from() < origin {
        return None;
    }
    let window = |t: Micros| (t.0.saturating_sub(origin.0) / WINDOW_US) as usize;
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

    let from = Micros((origin.0 + (first + start) as u64 * WINDOW_US).max(span.from().0));
    let to = Micros((origin.0 + (first + stop) as u64 * WINDOW_US).min(span.to().0));
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
        refine(&samples, Micros(0), &mut words);
        assert_eq!(spans(&words), [(300, 700)]);
    }

    #[test]
    fn an_inner_gap_of_200_ms_ends_the_word() {
        let samples = audio(&[(300, true), (250, false), (300, true)]);
        let mut words = [word(0, 850)];
        refine(&samples, Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 300)]);
    }

    #[test]
    fn a_shorter_gap_stays_inside_the_word() {
        let samples = audio(&[(300, true), (150, false), (300, true)]);
        let mut words = [word(0, 750)];
        refine(&samples, Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 750)]);
    }

    #[test]
    fn a_word_keeps_60_ms_and_stays_inside_its_span() {
        let samples = audio(&[(500, false), (20, true), (500, false), (500, true)]);
        let mut words = [word(400, 700)];
        refine(&samples, Micros(0), &mut words);
        assert_eq!(spans(&words), [(500, 560)]);
    }

    #[test]
    fn neighbours_stay_in_order_and_apart() {
        let samples = audio(&[(300, true), (400, false), (300, true), (200, false)]);
        let mut words = [word(0, 700), word(700, 1200)];
        refine(&samples, Micros(0), &mut words);
        assert_eq!(spans(&words), [(0, 300), (700, 1000)]);
    }
}
