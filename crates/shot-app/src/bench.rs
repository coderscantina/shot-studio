//! `--bench`: plays for a fixed time, then runs random seeks, repeated for several runs, and prints
//! what it measured with a pass or fail verdict. The process exits 0 on pass and 1 on fail.
//!
//! Playback counts frames by their timestamps against the clock: a due frame replaced before it
//! was shown is dropped. Seek latency runs from the seek request to the render that hands the
//! decoded frame to gpui; the GPU puts it on screen at the next display refresh.
//!
//! Every render interval over 1.5 refreshes is reported with what the main thread did meanwhile:
//! its CPU time and where a watchdog found it, see [`StallWatch`]. An idle main thread with a
//! late render means the display link did not call in time, which is outside our code; a busy or
//! blocked main thread names the function it was in.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::metrics::{self, StallSample, StallWatch};
use crate::preview::{PlaybackStats, Preview};

const WARM_UP: Duration = Duration::from_secs(1);

/// The acceptance limit for seek p95.
const SEEK_P95_LIMIT: Duration = Duration::from_millis(100);
/// Playback may drop one frame per this much played time (decision 0012).
const SECONDS_PER_ALLOWED_DROP: f64 = 300.0;

/// A render interval over this means a 60 Hz refresh went by without a render.
const LONG_INTERVAL: Duration = Duration::from_micros(25_000);

pub struct BenchOptions {
    pub seconds: u64,
    pub seeks: usize,
    pub runs: usize,
}

enum Phase {
    Starting,
    Playing {
        started: Instant,
        media_start: Duration,
        cpu: Duration,
        service_cpu: Duration,
    },
    Seeking {
        target: Option<Duration>,
        settle: u32,
    },
}

/// A render interval over [`LONG_INTERVAL`] and what the main thread did during it.
struct Gap {
    at: Duration,
    interval: Duration,
    main_cpu: Duration,
    samples: Vec<StallSample>,
    load: f64,
}

impl Gap {
    fn cause(&self) -> String {
        let idle = self
            .samples
            .iter()
            .filter(|sample| sample.is_idle())
            .count();
        let busy = self.samples.iter().filter(|sample| !sample.waiting).count();
        let blocked = self.samples.len() - idle - busy;
        let top = |filter: &dyn Fn(&StallSample) -> bool| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for sample in self.samples.iter().filter(|sample| filter(sample)) {
                *counts
                    .entry(
                        sample
                            .frames
                            .iter()
                            .take(4)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" < "),
                    )
                    .or_default() += 1;
            }
            counts
                .into_iter()
                .max_by_key(|(_, count)| *count)
                .map(|(stack, _)| stack)
                .unwrap_or_default()
        };
        let summary = format!(
            "{} samples: {idle} idle, {blocked} blocked, {busy} running",
            self.samples.len()
        );
        if self.samples.is_empty() {
            format!("{summary}, the watchdog missed it")
        } else if idle * 2 >= self.samples.len() {
            format!("{summary}. Main thread idle in its run loop, the display link called late")
        } else if busy >= blocked && self.main_cpu * 2 < self.interval {
            format!(
                "{summary}. Main thread runnable but not on a core, in {}",
                top(&|sample| !sample.waiting)
            )
        } else if busy >= blocked {
            format!(
                "{summary}. Main thread busy in {}",
                top(&|sample| !sample.waiting)
            )
        } else {
            format!(
                "{summary}. Main thread blocked in {}",
                top(&|sample| sample.waiting && !sample.is_idle())
            )
        }
    }
}

/// Render timing of the playback phase.
#[derive(Default)]
struct RenderTiming {
    last: Option<(Instant, Duration)>,
    intervals: Vec<Duration>,
    /// Main thread CPU time per render interval: our render plus gpui's layout, paint and submit.
    main_cpu: Vec<Duration>,
    /// Time spent in `Preview::render`.
    render: Vec<Duration>,
    gaps: Vec<Gap>,
}

struct Run {
    seconds: f64,
    /// Media time advanced per host second. Below 1 when locked to a display slower than the video.
    clock_rate: f64,
    stats: PlaybackStats,
    cpu_percent: f64,
    service_cpu_percent: f64,
    timing: RenderTiming,
    /// Latency of each seek with the number of frames decoded before the target.
    seeks: Vec<(Duration, u32)>,
    wrong_frames: usize,
}

impl Run {
    fn seek_percentile(&self, p: f64) -> Duration {
        percentile(self.seeks.iter().map(|(latency, _)| *latency).collect(), p)
    }

    /// Dropped frames are judged over all runs together, see [`allowed_drops`].
    fn passed(&self) -> bool {
        let stats = self.stats;
        stats.late == 0
            && stats.underruns == 0
            && self.wrong_frames == 0
            && self.seek_percentile(0.95) < SEEK_P95_LIMIT
    }
}

pub struct Bench {
    options: BenchOptions,
    phase: Phase,
    stalls: StallWatch,
    rng: u64,
    renders: u64,
    peak_footprint: u64,
    peak_resident: u64,
    timing: RenderTiming,
    /// The playback half of the run in progress, until its seeks are done.
    current: Option<Run>,
    runs: Vec<Run>,
}

impl Bench {
    /// Call on the main thread, which the bench watches for stalls.
    pub fn new(options: BenchOptions) -> Self {
        Self {
            options,
            phase: Phase::Starting,
            stalls: StallWatch::start(),
            rng: 0x5eed,
            renders: 0,
            peak_footprint: 0,
            peak_resident: 0,
            timing: RenderTiming::default(),
            current: None,
            runs: Vec::new(),
        }
    }

    pub fn phase_label(&self) -> String {
        let run = format!("run {}/{}", self.runs.len() + 1, self.options.runs);
        match &self.phase {
            Phase::Starting => format!("{run} starting"),
            Phase::Playing { started, .. } => format!(
                "{run} playing {:.0}/{} s",
                started.elapsed().as_secs_f64(),
                self.options.seconds
            ),
            Phase::Seeking { .. } => {
                let done = self.current.as_ref().map_or(0, |run| run.seeks.len());
                format!("{run} seek {done}/{}", self.options.seeks)
            }
        }
    }

    /// Records how long `Preview::render` took, while playing.
    pub fn rendered(&mut self, took: Duration) {
        if matches!(self.phase, Phase::Playing { .. }) {
            self.timing.render.push(took);
        }
    }

    /// Advances the bench by one render. Returns the verdict once every run is done.
    pub fn step(&mut self, preview: &mut Preview) -> Option<bool> {
        self.renders += 1;
        let samples = self.stalls.rendered();
        if self.renders.is_multiple_of(30) {
            let (footprint, resident) = metrics::memory();
            self.peak_footprint = self.peak_footprint.max(footprint);
            self.peak_resident = self.peak_resident.max(resident);
        }

        match &mut self.phase {
            Phase::Starting => {
                // Press play after the window has been up for a moment, like a user would.
                if !preview.playing && preview.host_now() >= WARM_UP {
                    preview.set_playing(true);
                }
                if preview.pacer.clock.is_playing() {
                    preview.stats = PlaybackStats::default();
                    self.timing = RenderTiming::default();
                    self.phase = Phase::Playing {
                        started: Instant::now(),
                        media_start: preview.pacer.clock.media_time(preview.host_now()),
                        cpu: metrics::cpu_time(),
                        service_cpu: metrics::decoder_service_cpu_time(),
                    };
                }
            }
            Phase::Playing {
                started,
                media_start,
                cpu,
                service_cpu,
            } => {
                let now = Instant::now();
                let main_cpu = metrics::thread_cpu_time();
                if let Some((last, last_cpu)) = self.timing.last {
                    let interval = now - last;
                    let cpu = main_cpu.saturating_sub(last_cpu);
                    self.timing.intervals.push(interval);
                    self.timing.main_cpu.push(cpu);
                    if interval > LONG_INTERVAL {
                        self.timing.gaps.push(Gap {
                            at: started.elapsed(),
                            interval,
                            main_cpu: cpu,
                            samples,
                            load: metrics::load_average(),
                        });
                    }
                }
                self.timing.last = Some((now, main_cpu));

                let played = preview.pacer.clock.media_time(preview.host_now()) - *media_start;
                if played >= Duration::from_secs(self.options.seconds) {
                    let wall = started.elapsed().as_secs_f64();
                    let percent = |cpu: Duration| 100.0 * cpu.as_secs_f64() / wall;
                    self.current = Some(Run {
                        seconds: played.as_secs_f64(),
                        clock_rate: played.as_secs_f64() / wall,
                        stats: preview.stats,
                        cpu_percent: percent(metrics::cpu_time() - *cpu),
                        service_cpu_percent: percent(
                            metrics::decoder_service_cpu_time().saturating_sub(*service_cpu),
                        ),
                        timing: std::mem::take(&mut self.timing),
                        seeks: Vec::with_capacity(self.options.seeks),
                        wrong_frames: 0,
                    });
                    preview.set_playing(false);
                    self.phase = Phase::Seeking {
                        target: None,
                        settle: 0,
                    };
                }
            }
            Phase::Seeking { target, settle } => {
                if preview.pending_seek.is_some() {
                    return None;
                }
                let run = self.current.as_mut().expect("seeking follows playback");
                if let Some(requested) = target.take() {
                    let (latency, shown) = preview.last_seek.expect("a finished seek");
                    let info = preview.player.info();
                    let plan = info.index.plan(requested).expect("the clip has frames");
                    let forward = ((plan.target - plan.keyframe).as_secs_f64()
                        / info.frame_duration.as_secs_f64())
                    .round() as u32;
                    run.seeks.push((latency, forward));
                    if plan.target != shown {
                        run.wrong_frames += 1;
                    }
                    // Roughly a fast scrub.
                    *settle = 3;
                }
                if *settle > 0 {
                    *settle -= 1;
                    return None;
                }
                if run.seeks.len() < self.options.seeks {
                    let next = preview
                        .player
                        .info()
                        .duration
                        .mul_f64(next_random(&mut self.rng));
                    *target = Some(next);
                    preview.seek(next);
                    return None;
                }
                let run = self.current.take().expect("seeking follows playback");
                print_run(self.runs.len() + 1, self.options.runs, &run, preview);
                self.runs.push(run);
                if self.runs.len() < self.options.runs {
                    self.phase = Phase::Starting;
                    return None;
                }
                return Some(self.report(preview));
            }
        }
        None
    }

    /// Prints the summary over all runs and returns whether they pass together.
    fn report(&self, preview: &Preview) -> bool {
        let info = preview.player.info();
        println!(
            "video: {} frames, {} keyframes, {:.3} s, {:.2} fps, hardware decode {}",
            info.index.len(),
            info.index.keyframe_count(),
            info.duration.as_secs_f64(),
            1.0 / info.frame_duration.as_secs_f64(),
            info.hardware_decode
        );
        if let Some(frame) = &preview.current {
            println!(
                "frame: {}x{}, pixel format {}, matrix {}, IOSurface {}",
                frame.width(),
                frame.height(),
                fourcc(frame.pixel_format()),
                frame.ycbcr_matrix().as_deref().unwrap_or("untagged"),
                frame.is_iosurface_backed()
            );
        }
        println!(
            "zoom {:.1}x, pacing {}",
            preview.zoom.get(),
            if preview.pacer.is_locked() {
                "locked to display"
            } else {
                "wall clock"
            }
        );
        let seeks: Vec<(Duration, u32)> =
            self.runs.iter().flat_map(|run| run.seeks.clone()).collect();
        let latencies: Vec<Duration> = seeks.iter().map(|(latency, _)| *latency).collect();
        if !latencies.is_empty() {
            println!(
                "all seeks: {}, p50 {}, p95 {}, max {}",
                latencies.len(),
                ms(percentile(latencies.clone(), 0.5)),
                ms(percentile(latencies.clone(), 0.95)),
                ms(percentile(latencies, 1.0)),
            );
        }
        let longest = seeks.iter().map(|(_, forward)| *forward).max().unwrap_or(0);
        for low in (0..=longest).step_by(10) {
            let bucket: Vec<Duration> = seeks
                .iter()
                .filter(|(_, forward)| (low..low + 10).contains(forward))
                .map(|(latency, _)| *latency)
                .collect();
            if !bucket.is_empty() {
                let mean = bucket.iter().sum::<Duration>() / bucket.len() as u32;
                println!(
                    "  decoding {low}-{} frames past the keyframe: {}, mean {}, max {}",
                    low + 9,
                    bucket.len(),
                    ms(mean),
                    ms(percentile(bucket, 1.0))
                );
            }
        }
        println!(
            "memory: peak footprint {:.0} MB, peak resident {:.0} MB",
            self.peak_footprint as f64 / 1e6,
            self.peak_resident as f64 / 1e6
        );
        let failed = self.runs.iter().filter(|run| !run.passed()).count();
        let dropped: u64 = self.runs.iter().map(|run| run.stats.dropped).sum();
        let worst_p95 = self
            .runs
            .iter()
            .map(|run| run.seek_percentile(0.95))
            .max()
            .unwrap_or_default();
        let allowed = allowed_drops(self.runs.iter().map(|run| run.seconds).sum());
        let passed = failed == 0 && dropped <= allowed;
        let verdict = if passed { "PASS" } else { "FAIL" };
        println!(
            "{verdict}: {} of {} runs passed, {dropped} dropped frames (limit {allowed}), worst seek p95 {} (limit {})",
            self.runs.len() - failed,
            self.runs.len(),
            ms(worst_p95),
            ms(SEEK_P95_LIMIT)
        );
        passed
    }
}

fn print_run(number: usize, runs: usize, run: &Run, preview: &Preview) {
    let fps = 1.0 / preview.player.info().frame_duration.as_secs_f64();
    let stats = run.stats;
    println!(
        "run {number}/{runs}: {}. played {:.1} s, presented {} of {:.0}, dropped {}, late {}, underruns {}; seeks p50 {}, p95 {}, max {}, wrong frame {}",
        if run.passed() { "pass" } else { "FAIL" },
        run.seconds,
        stats.presented,
        run.seconds * fps,
        stats.dropped,
        stats.late,
        stats.underruns,
        ms(run.seek_percentile(0.5)),
        ms(run.seek_percentile(0.95)),
        ms(run.seek_percentile(1.0)),
        run.wrong_frames,
    );
    let timing = &run.timing;
    println!(
        "  render interval p50 {}, p99 {}, max {}; main thread cpu per frame p50 {}, p99 {}, max {}; Preview::render p99 {}, max {}",
        ms(percentile(timing.intervals.clone(), 0.5)),
        ms(percentile(timing.intervals.clone(), 0.99)),
        ms(percentile(timing.intervals.clone(), 1.0)),
        ms(percentile(timing.main_cpu.clone(), 0.5)),
        ms(percentile(timing.main_cpu.clone(), 0.99)),
        ms(percentile(timing.main_cpu.clone(), 1.0)),
        ms(percentile(timing.render.clone(), 0.99)),
        ms(percentile(timing.render.clone(), 1.0)),
    );
    println!(
        "  clock rate {:.5}, cpu app {:.1}%, VTDecoderXPCService {:.1}% of one core",
        run.clock_rate, run.cpu_percent, run.service_cpu_percent
    );
    for gap in &timing.gaps {
        println!(
            "  gap at {:.2} s: {}, main thread cpu {}, load {:.1}, {}",
            gap.at.as_secs_f64(),
            ms(gap.interval),
            ms(gap.main_cpu),
            gap.load,
            gap.cause()
        );
    }
}

/// The nearest-rank percentile, zero for no values.
fn percentile(mut values: Vec<Duration>, p: f64) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    values.sort_unstable();
    let rank = ((values.len() as f64 * p).ceil() as usize).clamp(1, values.len());
    values[rank - 1]
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// Uniform in [0, 1). The bench starts from a fixed seed, so every bench seeks to the same places.
fn next_random(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 11) as f64 / (1u64 << 53) as f64
}

fn fourcc(code: u32) -> String {
    code.to_be_bytes()
        .iter()
        .map(|&byte| byte as char)
        .collect()
}

/// Dropped frames allowed over `played` seconds of playback: one per five minutes, rounded.
fn allowed_drops(played: f64) -> u64 {
    (played / SECONDS_PER_ALLOWED_DROP).round() as u64
}

#[cfg(test)]
mod drop_tests {
    use super::allowed_drops;

    #[test]
    fn five_minute_runs_allow_one_drop() {
        assert_eq!(allowed_drops(60.0), 0);
        assert_eq!(allowed_drops(299.6), 1);
        assert_eq!(allowed_drops(600.0), 2);
    }
}
