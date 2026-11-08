//! `--bench-edit <video-folder>`: plays the edited timeline through the Edit screen's preview
//! engine, which paints every frame as export does, bookends and transitions included, then
//! seeks at random,
//! and prints the same measurements and PASS or FAIL rule as [`crate::bench`].
//!
//! It runs headless: a 60 Hz ticker takes frames from the engine's queue the way the Edit
//! screen's clock does, without sound and without the display. A dropped frame is a frame number
//! the ticker skipped because a newer one was already due. A late frame is shown after its due
//! tick, and a starved tick found the queue empty. Frames skipped on a tick that itself came more
//! than [`LATE_TICK`] after the one before were lost to the bench thread not being scheduled, not
//! to the engine: they are counted apart and do not fail the run. Seek latency runs from the seek
//! request to the frame being ready in the queue, the display hand-off excluded.
//!
//! `--glass` draws a launch preset reel (glass card with a soft shadow, tilt and kinetic headline,
//! a motion blurred badge, word pop captions) over every frame, 6 s of every 12 s, the costliest
//! motion the renderer has, in a pass of its own over the painted frame. Each run prints the
//! bookends and transitions it played through.

use std::time::{Duration, Instant};

use shot_core::{Micros, TimeRange};
use shot_render::drawing::{Font, Glass, Placement, Rgba, Shape, Sheet, SoftShadow};
use shot_render::frame::Frame;
use shot_render::kinetic::{self, Kinetic};
use shot_render::layout::{Canvas, Rect};
use shot_render::materials::{self, LIGHT};
use shot_render::motion::{Part, Preset};

use crate::bench::{BenchOptions, SEEK_P95_LIMIT, allowed_drops, ms, next_random, percentile};
use crate::engine::{Engine, Plan};
use crate::folder::VideoFolder;
use crate::metrics;
use crate::model::{EditMap, FPS, View};

const TICK: Duration = Duration::from_nanos(1_000_000_000 / FPS as u64);
/// Ticks further apart than this mean the bench thread was not scheduled for over a refresh.
const LATE_TICK: Duration = Duration::from_micros(25_000);
/// The pause between seeks, roughly a fast scrub.
const SCRUB_PAUSE: Duration = Duration::from_millis(50);
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Playback {
    seconds: f64,
    presented: u64,
    dropped: u64,
    late: u64,
    /// Skipped frames on late ticks, see the module docs.
    stalled: u64,
    starved: u64,
    late_ticks: u64,
    /// The frames shown right after a drop.
    drops_before: Vec<u64>,
    cpu_percent: f64,
    service_cpu_percent: f64,
}

struct Seeks {
    latencies: Vec<Duration>,
    wrong: usize,
}

/// Runs the bench on `dir` and returns whether it passed.
pub fn run(dir: &std::path::Path, options: &BenchOptions) -> Result<bool, String> {
    let folder = VideoFolder::open(dir)?;
    let project = &folder.shown;
    let map = EditMap::preview(dir, &folder.project, project, View::Edited)?;
    if map.frames == 0 {
        return Err("the edit has no frames".into());
    }
    let frames = map.frames;
    let joins = map.plan.clone();
    let engine = Engine::spawn();
    if options.glass {
        engine.set_decoration(launch_reel);
    }
    engine.set_plan(
        Plan {
            map,
            dir: dir.to_owned(),
            effects: project.format.effects(),
            theme: folder.theme.clone(),
        },
        0,
    );

    println!(
        "edit: {} frames ({:.1} s) at {FPS} fps, {} spans, {} bookends, {} transitions ({} two-sided), {}",
        frames,
        frames as f64 / f64::from(FPS),
        engine_spans(&folder),
        joins.bookends.len(),
        joins.transitions.len(),
        joins.transitions.iter().filter(|t| t.two_sided()).count(),
        if options.glass {
            "launch reel with glass and motion blur on"
        } else {
            "effects as the project has them"
        }
    );

    let play_frames = (options.seconds * u64::from(FPS)).min(frames);
    let mut rng = 0x5eed;
    let mut peak = (0, 0);
    let mut runs = Vec::new();
    for run in 0..options.runs {
        // Each run plays another stretch of the edit.
        let start = (frames - play_frames) * run as u64 / options.runs as u64;
        let playback = play(&engine, start, play_frames, &mut peak)?;
        let seeks = seek(&engine, frames, options.seeks, &mut rng, &mut peak)?;
        print_run(run + 1, options.runs, &playback, &seeks);
        let played = start..start + play_frames;
        let crossed: Vec<String> = joins
            .transitions
            .iter()
            .filter(|t| played.contains(&t.window.start) && played.contains(&(t.window.end - 1)))
            .map(|t| format!("{} at {:.1} s", t.kind, t.edge as f64 / f64::from(FPS)))
            .collect();
        let bookends = joins
            .bookends
            .iter()
            .filter(|b| played.contains(&b.output_frame))
            .count();
        let drops: Vec<String> = playback
            .drops_before
            .iter()
            .map(|f| format!("{:.2} s", *f as f64 / f64::from(FPS)))
            .collect();
        println!(
            "  played through {} bookend starts and {} transitions: {}; drops before {}",
            bookends,
            crossed.len(),
            crossed.join(", "),
            if drops.is_empty() {
                "none".to_owned()
            } else {
                drops.join(", ")
            }
        );
        runs.push((playback, seeks));
    }
    Ok(report(&runs, peak))
}

fn engine_spans(folder: &VideoFolder) -> String {
    let overlays: usize = folder
        .shown
        .cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .map(|t| t.overlays.len())
        .sum();
    let zooms: usize = folder
        .shown
        .cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .map(|t| t.zooms.len())
        .sum();
    format!("{overlays} overlays, {zooms} zooms")
}

/// Waits for the engine to queue its first frame and pops it.
fn first_frame(engine: &Engine) -> Result<u64, String> {
    let waited = Instant::now();
    loop {
        if let Some(error) = engine.error() {
            return Err(error);
        }
        if let Some(frame) = engine.take_next() {
            return Ok(frame.frame);
        }
        if waited.elapsed() > FIRST_FRAME_TIMEOUT {
            return Err("the engine rendered no frame".into());
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

fn play(
    engine: &Engine,
    start: u64,
    frames: u64,
    peak: &mut (u64, u64),
) -> Result<Playback, String> {
    engine.set_playing(false);
    engine.seek(start);
    first_frame(engine)?;
    engine.set_playing(true);
    let from = loop {
        if let Some(error) = engine.error() {
            return Err(error);
        }
        if let Some(front) = engine.front() {
            break front;
        }
        std::thread::sleep(Duration::from_micros(200));
    };

    let mut stats = Playback::default();
    let began = Instant::now();
    engine.take_due(from).ok_or("the first frame vanished")?;
    let mut last = from;
    stats.presented = 1;
    let cpu = metrics::cpu_time();
    let service_cpu = metrics::decoder_service_cpu_time();
    let mut previous = began;
    let mut tick = 0u32;
    loop {
        tick += 1;
        let at = began + TICK * tick;
        if let Some(wait) = at.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let now = Instant::now();
        let stalled = now - previous > LATE_TICK;
        previous = now;
        stats.late_ticks += u64::from(stalled);
        if let Some(error) = engine.error() {
            return Err(error);
        }
        let due = from + (began.elapsed().as_secs_f64() * f64::from(FPS)) as u64;
        match engine.take_due(due) {
            Some(frame) => {
                stats.presented += 1;
                let skipped = frame.frame.saturating_sub(last + 1);
                if stalled {
                    stats.stalled += skipped;
                } else if skipped > 0 {
                    stats.dropped += skipped;
                    stats.drops_before.push(frame.frame);
                }
                if frame.frame < due {
                    stats.late += 1;
                }
                last = frame.frame;
            }
            None if engine.front().is_none() => stats.starved += 1,
            None => {}
        }
        if tick.is_multiple_of(30) {
            let (footprint, resident) = metrics::memory();
            peak.0 = peak.0.max(footprint);
            peak.1 = peak.1.max(resident);
        }
        if last + 1 >= start + frames || due + 1 >= start + frames {
            break;
        }
    }
    let wall = began.elapsed().as_secs_f64();
    let percent = |cpu: Duration| 100.0 * cpu.as_secs_f64() / wall;
    stats.seconds = (last + 1 - from) as f64 / f64::from(FPS);
    stats.cpu_percent = percent(metrics::cpu_time() - cpu);
    stats.service_cpu_percent =
        percent(metrics::decoder_service_cpu_time().saturating_sub(service_cpu));
    engine.set_playing(false);
    Ok(stats)
}

fn seek(
    engine: &Engine,
    frames: u64,
    count: usize,
    rng: &mut u64,
    peak: &mut (u64, u64),
) -> Result<Seeks, String> {
    let mut seeks = Seeks {
        latencies: Vec::with_capacity(count),
        wrong: 0,
    };
    for _ in 0..count {
        let target = ((frames as f64 * next_random(rng)) as u64).min(frames - 1);
        let requested = Instant::now();
        engine.seek(target);
        let shown = first_frame(engine)?;
        seeks.latencies.push(requested.elapsed());
        if shown != target {
            seeks.wrong += 1;
        }
        let (footprint, resident) = metrics::memory();
        peak.0 = peak.0.max(footprint);
        peak.1 = peak.1.max(resident);
        std::thread::sleep(SCRUB_PAUSE);
    }
    Ok(seeks)
}

fn print_run(number: usize, runs: usize, playback: &Playback, seeks: &Seeks) {
    println!(
        "run {number}/{runs}: played {:.1} s, presented {} of {:.0}, dropped {}, late {}, starved ticks {}, {} lost to {} bench ticks over 25 ms apart; seeks p50 {}, p95 {}, max {}, wrong frame {}",
        playback.seconds,
        playback.presented,
        playback.seconds * f64::from(FPS),
        playback.dropped,
        playback.late,
        playback.starved,
        playback.stalled,
        playback.late_ticks,
        ms(percentile(seeks.latencies.clone(), 0.5)),
        ms(percentile(seeks.latencies.clone(), 0.95)),
        ms(percentile(seeks.latencies.clone(), 1.0)),
        seeks.wrong,
    );
    println!(
        "  cpu app {:.1}%, VTDecoderXPCService {:.1}% of one core, load {:.1}",
        playback.cpu_percent,
        playback.service_cpu_percent,
        metrics::load_average()
    );
}

/// Prints the summary over all runs and returns whether they pass together.
fn report(runs: &[(Playback, Seeks)], peak: (u64, u64)) -> bool {
    let latencies: Vec<Duration> = runs
        .iter()
        .flat_map(|(_, seeks)| seeks.latencies.clone())
        .collect();
    let p95 = percentile(latencies.clone(), 0.95);
    if !latencies.is_empty() {
        println!(
            "all seeks: {}, p50 {}, p95 {}, max {}",
            latencies.len(),
            ms(percentile(latencies.clone(), 0.5)),
            ms(p95),
            ms(percentile(latencies, 1.0)),
        );
    }
    println!(
        "memory: peak footprint {:.0} MB, peak resident {:.0} MB",
        peak.0 as f64 / 1e6,
        peak.1 as f64 / 1e6
    );
    let dropped: u64 = runs.iter().map(|(p, _)| p.dropped).sum();
    let wrong: usize = runs.iter().map(|(_, s)| s.wrong).sum();
    let allowed = allowed_drops(runs.iter().map(|(p, _)| p.seconds).sum());
    let passed = dropped <= allowed && wrong == 0 && p95 < SEEK_P95_LIMIT;
    println!(
        "{}: {dropped} dropped frames (limit {allowed}), {wrong} wrong seek frames, seek p95 {} (limit {})",
        if passed { "PASS" } else { "FAIL" },
        ms(p95),
        ms(SEEK_P95_LIMIT)
    );
    passed
}

fn range(from: u64, to: u64) -> TimeRange {
    TimeRange::new(Micros(from), Micros(to)).expect("from before to")
}

/// Every 12 s, a launch card on frosted glass flies in with a soft shadow, a kinetic headline and
/// a motion blurred badge, then word pop captions, all leaving again within 6 s.
fn launch_reel(t: Micros, canvas: Canvas, frame: &mut Frame) {
    let local = Micros(t.0 % 12_000_000);
    if local >= Micros(6_000_000) {
        return;
    }
    let preset = Preset::Launch;
    let s = canvas.width as f32 / 3840.0;
    let motion = preset.motion();
    let moves = preset.moves();
    let drawing = &mut frame.drawing;

    let card = range(400_000, 5_400_000);
    let size = [1500.0 * s, 380.0 * s];
    let rest = [120.0 * s + size[0] / 2.0, canvas.height as f32 * 0.62];
    let place = |t: Micros| {
        let pose = motion.pose(card, Part::WHOLE, t);
        let away = pose.offset();
        let placement = Placement {
            center: [
                rest[0] - away * (size[0] + 200.0 * s) * motion.travel.min(1.0),
                rest[1],
            ],
            scale: 1.0,
            tilt: [moves.tilt * 0.4 * away, -moves.tilt * away],
            opacity: pose.presence(),
        };
        let camera = materials::drift(t, Micros(2_900_000), 18.0 * s, [1.0, -0.3]);
        materials::parallax(placement, 0.8, camera)
    };
    let [open, close] = materials::shutter(local).map(place);
    if open.opacity > 0.0 || close.opacity > 0.0 {
        drawing.sheets.push(Sheet {
            size,
            radius: 36.0 * s,
            glass: Some(Glass {
                blur: 36.0 * s,
                tint: Rgba::hex(0x101216, 0.45),
            }),
            shapes: vec![Shape::Text {
                origin: [96.0 * s, 150.0 * s],
                text: "Ship it on Friday".into(),
                size: 120.0 * s,
                font: Font::Sans,
                color: Rgba::hex(0xFFFFFF, 1.0),
            }],
            placement: [open, close],
        });
        let now = place(local);
        let [dx, dy] = LIGHT.shadow_offset(70.0 * s);
        let half = [size[0] / 2.0 * now.scale, size[1] / 2.0 * now.scale];
        drawing.shadows.push(SoftShadow {
            shapes: vec![Shape::Rect {
                rect: Rect::new(
                    now.center[0] - half[0],
                    now.center[1] - half[1],
                    now.center[0] + half[0],
                    now.center[1] + half[1],
                ),
                radius: 36.0 * s,
                color: Rgba::hex(0x000000, 1.0),
            }],
            blur: 40.0 * s,
            color: Rgba::hex(0x000000, 0.5 * now.opacity),
            offset: [dx, dy],
        });
    }

    let headline = range(900_000, 5_000_000);
    if (headline.from()..headline.to()).contains(&local) {
        drawing.over.push(Shape::Kinetic(Box::new(Kinetic {
            origin: [160.0 * s, 150.0 * s],
            text: "Springs, glass and kinetic type".into(),
            size: 150.0 * s,
            font: Font::Sans,
            color: Rgba::hex(0xFFFFFF, 1.0),
            width: Some(2600.0 * s),
            reveal: moves.reveal,
            unit: moves.unit,
            motion,
            range: headline,
            t: local,
        })));
    }

    let badge = range(700_000, 5_200_000);
    let badge_size = [520.0 * s, 130.0 * s];
    let badge_at = |t: Micros| {
        let pose = motion.pose(badge, Part::WHOLE, t);
        Placement {
            center: [
                canvas.width as f32 - 640.0 * s
                    + pose.offset() * 1400.0 * s * motion.travel.min(1.0),
                canvas.height as f32 * 0.38,
            ],
            scale: 1.0,
            tilt: [0.0, 0.0],
            opacity: pose.presence(),
        }
    };
    let placement = materials::shutter(local).map(badge_at);
    if placement.iter().any(|p| p.opacity > 0.0) {
        drawing.sheets.push(Sheet {
            size: badge_size,
            radius: 0.0,
            glass: None,
            shapes: vec![Shape::Label {
                center: [badge_size[0] / 2.0, badge_size[1] / 2.0],
                text: "motion blur".into(),
                size: 72.0 * s,
                color: Rgba::hex(0x0A0B0D, 1.0),
                background: Rgba::hex(0xC8F25A, 1.0),
            }],
            placement,
        });
    }

    let words = ["word", "pop", "captions", "settle"];
    let start = 1_000_000;
    if (start..start + 4 * 700_000).contains(&local.0) {
        let index = ((local.0 - start) / 700_000) as usize;
        let since = Micros(local.0 - start - index as u64 * 700_000);
        drawing.over.push(Shape::Caption {
            center: [canvas.width as f32 / 2.0, canvas.height as f32 * 0.88],
            width: canvas.width as f32 * 0.8,
            words: words.iter().map(|w| (*w).to_owned()).collect(),
            highlight: Some(index),
            size: 96.0 * s,
            color: Rgba::hex(0xFFFFFF, 1.0),
            accent: Rgba::hex(0xC8F25A, 1.0),
            background: Rgba::hex(0x0A0B0D, 0.78),
            pop: kinetic::word_pop(since),
        });
    }
}
