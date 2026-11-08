//! Beats of a music file: an onset strength envelope, a tempo estimate from its autocorrelation,
//! and dynamic programming beat tracking (Ellis 2007), each beat then placed on the sharpest rise
//! of energy near it. Deterministic: the same samples give the same beats.

use std::f32::consts::TAU;

use realfft::RealFftPlanner;
use shot_core::PresetName;

use crate::SAMPLE_RATE;

/// Entrances snap to a beat at most this far away, 150 ms ([0023]).
///
/// [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md
pub const SNAP_WINDOW: u64 = SAMPLE_RATE as u64 * 3 / 20;

const WINDOW: usize = 1024;
const HOP: usize = 256;
/// Frames of the onset envelope per second.
const FPS: f64 = SAMPLE_RATE as f64 / HOP as f64;
/// Onsets count above the mean of this many frames on each side, about 85 ms.
const LOCAL: usize = 16;
const MIN_BPM: f64 = 50.0;
const MAX_BPM: f64 = 220.0;
/// Tempo prior: a log-normal around 120 BPM, one octave wide.
const PRIOR_BPM: f64 = 120.0;
/// How hard beat tracking holds to the tempo against the onsets.
const TIGHTNESS: f64 = 100.0;
/// Energy blocks for placing a beat, 1.3 ms.
const BLOCK: usize = 64;

/// The beats of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Beats {
    /// Tempo in beats per minute, 0 when no beat was found.
    pub bpm: f64,
    /// Sample of each beat in the track, ascending.
    pub at: Vec<u64>,
}

/// Detects the beats of mono 48 kHz `samples`.
pub fn detect(samples: &[f32]) -> Beats {
    let none = Beats {
        bpm: 0.0,
        at: Vec::new(),
    };
    let onset = onset_strength(samples);
    let Some(period) = period(&onset) else {
        return none;
    };
    let frames = track(&onset, period);
    let at: Vec<u64> = frames.iter().map(|&f| place(samples, f)).collect();
    let bpm = match at.len() {
        0 => return none,
        1 => 60.0 * FPS / period,
        _ => 60.0 * f64::from(SAMPLE_RATE) / slope(&at),
    };
    Beats { bpm, at }
}

/// Whether a preset snaps entrances to the beat: the brand's `beat-sync`, else reactive and
/// editorial do and launch and calm do not.
pub fn beat_sync(preset: PresetName, brand: Option<bool>) -> bool {
    brand.unwrap_or(matches!(
        preset,
        PresetName::Reactive | PresetName::Editorial
    ))
}

/// Spectral flux of log magnitudes per frame, above its local mean, scaled to unit deviation.
/// Before the first frame is silence, so a track that starts on a hit has a beat at 0.
fn onset_strength(samples: &[f32]) -> Vec<f32> {
    if samples.len() < WINDOW {
        return Vec::new();
    }
    let frames = (samples.len() - WINDOW) / HOP + 1;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
    let hann: Vec<f32> = (0..WINDOW)
        .map(|i| 0.5 - 0.5 * (TAU * i as f32 / WINDOW as f32).cos())
        .collect();
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let mut previous = vec![0.0f32; spectrum.len()];
    let mut flux = Vec::with_capacity(frames);
    for f in 0..frames {
        let frame = &samples[f * HOP..f * HOP + WINDOW];
        for ((x, s), w) in input.iter_mut().zip(frame).zip(&hann) {
            *x = s * w;
        }
        fft.process_with_scratch(&mut input, &mut spectrum, &mut scratch)
            .expect("buffers come from the plan");
        let mut rise = 0.0;
        for (p, s) in previous.iter_mut().zip(&spectrum) {
            let level = (1.0 + 100.0 * s.norm()).ln();
            rise += (level - *p).max(0.0);
            *p = level;
        }
        flux.push(rise);
    }

    let mut prefix = vec![0.0f64; frames + 1];
    for (i, f) in flux.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*f);
    }
    let onset: Vec<f32> = (0..frames)
        .map(|i| {
            let from = i.saturating_sub(LOCAL);
            let to = (i + LOCAL + 1).min(frames);
            let mean = (prefix[to] - prefix[from]) / (to - from) as f64;
            (f64::from(flux[i]) - mean).max(0.0) as f32
        })
        .collect();
    let power = onset.iter().map(|o| o * o).sum::<f32>() / frames as f32;
    let deviation = power.sqrt();
    if deviation == 0.0 {
        return onset;
    }
    onset.iter().map(|o| o / deviation).collect()
}

/// The beat period in frames, fractional: the lag whose autocorrelation, with that of two and
/// four beats on, scores highest under the tempo prior. The longer lags favor the beat that
/// bars repeat on over a faster grid of off-beats or triplets.
fn period(onset: &[f32]) -> Option<f64> {
    let min_lag = (FPS * 60.0 / MAX_BPM).ceil() as usize;
    let max_lag = ((FPS * 60.0 / MIN_BPM).floor() as usize).min(onset.len() / 2);
    if max_lag < min_lag + 2 {
        return None;
    }
    let longest = (4 * max_lag + 5).min(onset.len() - 1);
    let acf: Vec<f64> = (0..=longest)
        .map(|lag| {
            let n = onset.len() - lag;
            onset[..n]
                .iter()
                .zip(&onset[lag..])
                .map(|(a, b)| f64::from(a * b))
                .sum::<f64>()
                / n as f64
        })
        .collect();
    let near = |lag: usize| {
        acf.get(lag.saturating_sub(1)..(lag + 2).min(acf.len()))
            .map_or(0.0, |a| a.iter().fold(0.0f64, |m, v| m.max(*v)))
    };
    let weighted = |lag: usize| {
        let octaves = (FPS * 60.0 / lag as f64 / PRIOR_BPM).log2();
        let periodic = acf[lag] + 0.5 * near(2 * lag) + 0.25 * near(4 * lag);
        periodic * (-0.5 * octaves * octaves).exp()
    };
    let scores: Vec<f64> = (min_lag - 1..=max_lag + 1).map(weighted).collect();
    let best =
        (1..scores.len() - 1).max_by(|&a, &b| scores[a].total_cmp(&scores[b]).then(b.cmp(&a)))?;
    if scores[best] <= 0.0 {
        return None;
    }
    let (l, c, r) = (scores[best - 1], scores[best], scores[best + 1]);
    let curve = l - 2.0 * c + r;
    let shift = if curve < 0.0 {
        0.5 * (l - r) / curve
    } else {
        0.0
    };
    Some((min_lag - 1 + best) as f64 + shift)
}

/// Beat frames by dynamic programming: each beat scores its onset plus the best earlier beat,
/// less a penalty for a gap away from the period. A chain only links to an earlier beat that
/// adds to it, so silence before the music starts no chain.
fn track(onset: &[f32], period: f64) -> Vec<usize> {
    let lo = ((period / 2.0).round() as usize).max(1);
    let hi = (period * 2.0).round() as usize;
    let penalty: Vec<f64> = (0..=hi)
        .map(|lag| TIGHTNESS * (lag as f64 / period).ln().powi(2))
        .collect();
    let mut score = vec![0.0f64; onset.len()];
    let mut back = vec![None; onset.len()];
    for i in 0..onset.len() {
        let mut best = 0.0;
        for lag in lo..=hi.min(i) {
            let candidate = score[i - lag] - penalty[lag];
            if candidate > best {
                best = candidate;
                back[i] = Some(i - lag);
            }
        }
        score[i] = f64::from(onset[i]) + best;
    }
    let Some(last) =
        (0..onset.len()).max_by(|&a, &b| score[a].total_cmp(&score[b]).then(b.cmp(&a)))
    else {
        return Vec::new();
    };
    let mut beats = vec![last];
    while let Some(previous) = back[*beats.last().expect("starts with one")] {
        beats.push(previous);
    }
    beats.reverse();
    trim(onset, beats)
}

/// Drops weak beats at either end, where the chain ran on into silence or a fade.
fn trim(onset: &[f32], mut beats: Vec<usize>) -> Vec<usize> {
    let strength = |f: usize| {
        onset[f.saturating_sub(2)..(f + 3).min(onset.len())]
            .iter()
            .fold(0.0f32, |a, b| a.max(*b))
    };
    let power = beats.iter().map(|&f| strength(f).powi(2)).sum::<f32>() / beats.len() as f32;
    let floor = 0.5 * power.sqrt();
    let end = beats
        .iter()
        .rposition(|&f| strength(f) >= floor)
        .map_or(0, |i| i + 1);
    beats.truncate(end);
    let start = beats
        .iter()
        .position(|&f| strength(f) >= floor)
        .unwrap_or(beats.len());
    beats.drain(..start);
    beats
}

/// The sample of the sharpest energy rise in the window of onset frame `frame`.
fn place(samples: &[f32], frame: usize) -> u64 {
    let from = (frame * HOP).saturating_sub(HOP);
    let to = (frame * HOP + WINDOW).min(samples.len());
    let energy =
        |start: usize, end: usize| -> f32 { samples[start..end].iter().map(|s| s * s).sum() };
    let mut best = (f32::NEG_INFINITY, from);
    let mut previous = energy(from.saturating_sub(BLOCK), from);
    for start in (from..to).step_by(BLOCK) {
        let current = energy(start, (start + BLOCK).min(to));
        if current - previous > best.0 {
            best = (current - previous, start);
        }
        previous = current;
    }
    best.1 as u64
}

/// Samples per beat, the least squares slope of beat position over beat number.
fn slope(at: &[u64]) -> f64 {
    let n = at.len() as f64;
    let mean_i = (n - 1.0) / 2.0;
    let mean_t = at.iter().map(|&t| t as f64).sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, &t) in at.iter().enumerate() {
        let di = i as f64 - mean_i;
        num += di * (t as f64 - mean_t);
        den += di * di;
    }
    num / den
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame at 60 fps.
    const FRAME: u64 = SAMPLE_RATE as u64 / 60;

    /// Clicks at `bpm` from `offset` samples for `seconds`, over a quiet hiss. Every fourth click
    /// is louder, like a bar's downbeat.
    fn click_track(bpm: f64, offset: u64, seconds: u64) -> (Vec<f32>, Vec<u64>) {
        let len = (seconds * u64::from(SAMPLE_RATE)) as usize;
        let mut rng = 1u32;
        let mut noise = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f32 / (1 << 23) as f32 - 1.0
        };
        let mut samples: Vec<f32> = (0..len).map(|_| 0.002 * noise()).collect();
        let step = 60.0 * f64::from(SAMPLE_RATE) / bpm;
        let mut truth = Vec::new();
        let mut k = 0;
        loop {
            let at = offset + (k as f64 * step).round() as u64;
            if at as usize + 2400 > len {
                break;
            }
            let amp = if k % 4 == 0 { 0.8 } else { 0.5 };
            for i in 0..2400 {
                let t = i as f32 / SAMPLE_RATE as f32;
                samples[at as usize + i] +=
                    amp * (-t * 60.0).exp() * (TAU * 1800.0 * t).sin() + amp * 0.3 * noise();
            }
            truth.push(at);
            k += 1;
        }
        (samples, truth)
    }

    #[test]
    fn click_tracks_give_their_tempo_and_every_beat_within_a_frame() {
        for (bpm, offset) in [(90.0, 12_000), (120.0, 30_000), (128.0, 0), (150.0, 7_777)] {
            let (samples, truth) = click_track(bpm, offset, 30);
            let beats = detect(&samples);
            assert!(
                (beats.bpm - bpm).abs() < 0.2,
                "{bpm} BPM detected as {}",
                beats.bpm
            );
            assert_eq!(beats.at.len(), truth.len(), "{bpm} BPM beat count");
            for (got, want) in beats.at.iter().zip(&truth) {
                assert!(
                    got.abs_diff(*want) < FRAME,
                    "{bpm} BPM: beat at {got}, click at {want}"
                );
            }
        }
    }

    #[test]
    fn silence_has_no_beats() {
        assert!(detect(&vec![0.0; SAMPLE_RATE as usize * 5]).at.is_empty());
    }

    #[test]
    fn presets_snap_by_default_unless_the_brand_says_otherwise() {
        assert!(beat_sync(PresetName::Reactive, None));
        assert!(!beat_sync(PresetName::Calm, None));
        assert!(beat_sync(PresetName::Calm, Some(true)));
        assert!(!beat_sync(PresetName::Reactive, Some(false)));
    }
}
