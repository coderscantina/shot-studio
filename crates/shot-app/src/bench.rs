//! `--bench`: plays for a fixed time, then runs random seeks, and prints what it measured.
//!
//! Playback counts frames by their timestamps against the clock: a due frame replaced before it
//! was shown is dropped. Seek latency runs from the seek request to the render that hands the
//! decoded frame to gpui; the GPU puts it on screen at the next display refresh.

use std::time::{Duration, Instant};

use crate::metrics;
use crate::preview::{PlaybackStats, Preview};

const WARM_UP: Duration = Duration::from_secs(1);

pub struct BenchOptions {
    pub seconds: u64,
    pub seeks: usize,
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

struct PlaybackResult {
    seconds: f64,
    /// Media time advanced per host second. Below 1 when locked to a display slower than the video.
    clock_rate: f64,
    stats: PlaybackStats,
    cpu_percent: f64,
    service_cpu_percent: f64,
    max_render_gap: Duration,
    long_render_gaps: Vec<Duration>,
}

pub struct Bench {
    options: BenchOptions,
    phase: Phase,
    rng: u64,
    renders: u64,
    last_render: Option<Instant>,
    max_render_gap: Duration,
    long_render_gaps: Vec<Duration>,
    peak_footprint: u64,
    peak_resident: u64,
    playback: Option<PlaybackResult>,
    /// Latency of each seek with the number of frames decoded before the target.
    seek_latencies: Vec<(Duration, u32)>,
    wrong_frames: usize,
}

impl Bench {
    pub fn new(options: BenchOptions) -> Self {
        Self {
            options,
            phase: Phase::Starting,
            rng: 0x5eed,
            renders: 0,
            last_render: None,
            max_render_gap: Duration::ZERO,
            long_render_gaps: Vec::new(),
            peak_footprint: 0,
            peak_resident: 0,
            playback: None,
            seek_latencies: Vec::new(),
            wrong_frames: 0,
        }
    }

    pub fn phase_label(&self) -> String {
        match &self.phase {
            Phase::Starting => "starting".into(),
            Phase::Playing { started, .. } => {
                format!(
                    "playing {:.0}/{} s",
                    started.elapsed().as_secs_f64(),
                    self.options.seconds
                )
            }
            Phase::Seeking { .. } => {
                format!("seek {}/{}", self.seek_latencies.len(), self.options.seeks)
            }
        }
    }

    /// Advances the bench by one render. Returns true when it printed its report.
    pub fn step(&mut self, preview: &mut Preview) -> bool {
        self.renders += 1;
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
                if let Some(last) = self.last_render {
                    let gap = now - last;
                    self.max_render_gap = self.max_render_gap.max(gap);
                    // A gap of more than 1.5 frames at 60 Hz means a refresh slot went by without a render.
                    if gap > Duration::from_micros(25_000) {
                        self.long_render_gaps.push(started.elapsed());
                    }
                }
                self.last_render = Some(now);

                let played = preview.pacer.clock.media_time(preview.host_now()) - *media_start;
                if played >= Duration::from_secs(self.options.seconds) {
                    let wall = started.elapsed().as_secs_f64();
                    let percent = |cpu: Duration| 100.0 * cpu.as_secs_f64() / wall;
                    self.playback = Some(PlaybackResult {
                        seconds: played.as_secs_f64(),
                        clock_rate: played.as_secs_f64() / wall,
                        stats: preview.stats,
                        cpu_percent: percent(metrics::cpu_time() - *cpu),
                        service_cpu_percent: percent(
                            metrics::decoder_service_cpu_time().saturating_sub(*service_cpu),
                        ),
                        max_render_gap: self.max_render_gap,
                        long_render_gaps: self.long_render_gaps.clone(),
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
                    return false;
                }
                if let Some(requested) = target.take() {
                    let (latency, shown) = preview.last_seek.expect("a finished seek");
                    let info = preview.player.info();
                    let plan = info.index.plan(requested).expect("the clip has frames");
                    let forward = ((plan.target - plan.keyframe).as_secs_f64()
                        / info.frame_duration.as_secs_f64())
                    .round() as u32;
                    self.seek_latencies.push((latency, forward));
                    if plan.target != shown {
                        self.wrong_frames += 1;
                    }
                    *settle = 3;
                }
                if *settle > 0 {
                    *settle -= 1;
                    return false;
                }
                if self.seek_latencies.len() >= self.options.seeks {
                    self.report(preview);
                    return true;
                }
                let next = preview
                    .player
                    .info()
                    .duration
                    .mul_f64(next_random(&mut self.rng));
                *target = Some(next);
                preview.seek(next);
            }
        }
        false
    }

    fn report(&self, preview: &Preview) {
        let info = preview.player.info();
        let fps = 1.0 / info.frame_duration.as_secs_f64();
        println!("shot-app bench");
        println!(
            "video: {} frames, {} keyframes, {:.3} s, {:.2} fps, hardware decode {}",
            info.index.len(),
            info.index.keyframe_count(),
            info.duration.as_secs_f64(),
            fps,
            info.hardware_decode
        );
        if let Some(frame) = &preview.current {
            println!(
                "frame: {}x{}, pixel format {}, IOSurface {}",
                frame.width(),
                frame.height(),
                fourcc(frame.pixel_format()),
                frame.is_iosurface_backed()
            );
        }
        println!("zoom: {:.1}x", preview.zoom.get());
        println!(
            "pacing: {}",
            if preview.pacer.is_locked() {
                "locked to display"
            } else {
                "wall clock"
            }
        );
        if let Some(playback) = &self.playback {
            let stats = playback.stats;
            println!(
                "playback: {:.1} s, expected {:.0} frames, presented {}, dropped {}, late {}, underruns {}",
                playback.seconds,
                playback.seconds * fps,
                stats.presented,
                stats.dropped,
                stats.late,
                stats.underruns
            );
            println!(
                "clock rate: {:.5} (media seconds per host second)",
                playback.clock_rate
            );
            let at: Vec<String> = playback
                .long_render_gaps
                .iter()
                .map(|time| format!("{:.2} s", time.as_secs_f64()))
                .collect();
            println!(
                "render gaps: max {:.1} ms, over 25 ms {} [{}]",
                playback.max_render_gap.as_secs_f64() * 1000.0,
                playback.long_render_gaps.len(),
                at.join(", ")
            );
            println!(
                "cpu: app {:.1}%, VTDecoderXPCService {:.1}% (of one core)",
                playback.cpu_percent, playback.service_cpu_percent
            );
        }
        let mut latencies: Vec<f64> = self
            .seek_latencies
            .iter()
            .map(|(latency, _)| latency.as_secs_f64() * 1000.0)
            .collect();
        latencies.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            let rank = ((latencies.len() as f64 * p).ceil() as usize).clamp(1, latencies.len());
            latencies[rank - 1]
        };
        if !latencies.is_empty() {
            println!(
                "seeks: {}, p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms, over 100 ms {}, wrong frame {}",
                latencies.len(),
                percentile(0.5),
                percentile(0.95),
                latencies[latencies.len() - 1],
                latencies.iter().filter(|&&ms| ms >= 100.0).count(),
                self.wrong_frames
            );
        }
        for (low, high) in [(0, 19), (20, 39), (40, 59)] {
            let bucket: Vec<f64> = self
                .seek_latencies
                .iter()
                .filter(|(_, forward)| (low..=high).contains(forward))
                .map(|(latency, _)| latency.as_secs_f64() * 1000.0)
                .collect();
            if let Some(max) = bucket.iter().copied().reduce(f64::max) {
                let mean = bucket.iter().sum::<f64>() / bucket.len() as f64;
                println!(
                    "seeks decoding {low}-{high} frames past the keyframe: {}, mean {mean:.1} ms, max {max:.1} ms",
                    bucket.len()
                );
            }
        }
        println!(
            "memory: peak footprint {:.0} MB, peak resident {:.0} MB",
            self.peak_footprint as f64 / 1e6,
            self.peak_resident as f64 / 1e6
        );
    }
}

/// Uniform in [0, 1). The bench starts from a fixed seed, so every run seeks to the same places.
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
