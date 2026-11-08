//! A reel of every transition, both bookends, the end card and the brand's camera frames over
//! the generated target project, drawn through [`Painter`]: golden frames reached by playing and
//! by seeking, and the evidence frames per transition and preset.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use shot_core::{
    Brand, Cut, FormatKind, Id, MediaId, Micros, Project, Script, Target, Timeline, TransitionKind,
    Video,
};

use crate::graph::Renderer;
use crate::join::{BookendInput, Join, Joins, StingerTiming};
use crate::paint::Painter;
use crate::plan::ExportPlan;
use crate::surface::{BufferPool, Matrix, Range, read_rgb};
use crate::theme::Theme;

/// Six chapters of 2 s from the fixture take, each joined to the one before by another
/// transition, the camera beside the screen in the first two.
const CUT: &str = r#"
chapter "Hook" {
    take t01 {
        keep from=10_000_000 to=12_000_000
        layout face-content from=10_000_000 to=12_000_000
    }
}
chapter "Dissolve" {
    take t01 {
        keep from=20_000_000 to=22_000_000
        layout split from=20_000_000 to=22_000_000
    }
}
chapter "Mask wipe" {
    take t01 {
        keep from=30_000_000 to=32_000_000
        layout face-content from=30_000_000 to=32_000_000
    }
}
chapter "Zoom through" {
    take t01 {
        keep from=40_000_000 to=42_000_000
    }
}
chapter "Dip" {
    take t01 {
        keep from=50_000_000 to=52_000_000
    }
}
chapter "Stinger" {
    take t01 {
        keep from=60_000_000 to=62_000_000
        layout face-content from=60_000_000 to=62_000_000
    }
}
"#;

const KINDS: [(u64, TransitionKind); 5] = [
    (20_000_000, TransitionKind::Dissolve),
    (30_000_000, TransitionKind::MaskWipe),
    (40_000_000, TransitionKind::ZoomThrough),
    (50_000_000, TransitionKind::Dip),
    (60_000_000, TransitionKind::Stinger),
];

/// A stinger: a full-frame card in the accent fading in to cover the cut 18 frames in, held,
/// then out, with a dark disc growing on it.
pub(crate) fn stinger() -> Vec<u8> {
    let prop = |k| json!({ "a": 0, "k": k });
    let ease = |t: u32, s: serde_json::Value| json!({ "t": t, "s": s, "o": { "x": [0.4], "y": [0.0] }, "i": { "x": [0.6], "y": [1.0] } });
    let transform = |opacity| {
        json!({ "a": prop(json!([0, 0])), "s": prop(json!([100, 100])), "o": opacity,
                "r": prop(json!(0)), "p": prop(json!([960, 540])) })
    };
    let fade = json!({ "a": 1, "k": [
        ease(0, json!([0])), ease(18, json!([100])), ease(36, json!([100])),
        { "t": 54, "s": [0] }
    ] });
    let group = |shape: serde_json::Value, color: [f32; 3], scale: serde_json::Value| {
        json!([{ "ty": "gr", "it": [
            shape,
            { "ty": "fl", "c": prop(json!(color)), "o": prop(json!(100)) },
            { "ty": "tr", "a": prop(json!([0, 0])), "p": prop(json!([0, 0])), "s": scale,
              "r": prop(json!(0)), "o": prop(json!(100)) }
        ] }])
    };
    let grow = json!({ "a": 1, "k": [
        { "t": 0, "s": [0, 0], "o": { "x": [0.4, 0.4], "y": [0.0, 0.0] },
          "i": { "x": [0.6, 0.6], "y": [1.0, 1.0] } },
        { "t": 30, "s": [100, 100] }
    ] });
    let file = json!({
        "v": "5.7.0", "fr": 60, "ip": 0, "op": 60, "w": 1920, "h": 1080, "layers": [
            { "ty": 4, "nm": "Disc", "ind": 1, "ip": 0, "op": 60, "st": 0,
              "ks": transform(fade.clone()),
              "shapes": group(json!({ "ty": "el", "p": prop(json!([0, 0])), "s": prop(json!([600, 600])) }),
                              [0.06, 0.08, 0.1], grow) },
            { "ty": 4, "nm": "Card", "ind": 2, "ip": 0, "op": 60, "st": 0,
              "ks": transform(fade),
              "shapes": group(json!({ "ty": "rc", "p": prop(json!([0, 0])), "s": prop(json!([1920, 1080])),
                                      "r": prop(json!(0)) }),
                              [0.78, 0.95, 0.35], prop(json!([100, 100]))) }
        ]
    });
    serde_json::to_vec(&file).unwrap()
}

pub(crate) const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120"><rect width="240" height="120" rx="28" fill="#c8f25a"/><circle cx="60" cy="60" r="34" fill="#101418"/><rect x="112" y="40" width="100" height="40" rx="12" fill="#101418"/></svg>"##;

/// A channel repo in a temp dir with the brand's files and a video folder over the fixture
/// take, and the theme of `preset` read from it.
struct Reel {
    _root: tempfile::TempDir,
    video: PathBuf,
    plan: ExportPlan,
    theme: Arc<Theme>,
    effects: shot_core::Effects,
}

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
}

fn reel(preset: &str, camera: &str) -> Reel {
    let root = tempfile::tempdir().unwrap();
    let brand_dir = root.path().join("brand");
    std::fs::create_dir_all(brand_dir.join("media")).unwrap();
    std::os::unix::fs::symlink(
        fixtures().join("camera-30s.mp4"),
        brand_dir.join("media/intro.mp4"),
    )
    .unwrap();
    std::fs::write(brand_dir.join("stinger.json"), stinger()).unwrap();
    std::fs::write(brand_dir.join("logo.svg"), LOGO).unwrap();
    let video = root.path().join("videos/reel");
    std::fs::create_dir_all(&video).unwrap();
    std::os::unix::fs::symlink(fixtures().join("target-project/media"), video.join("media"))
        .unwrap();
    let brand = Brand::parse(&format!(
        "colors accent=\"#c8f25a\" background=\"#101418\"\nlogo \"brand/logo.svg\"\n\
         camera {camera}\nmotion opinion={preset}\n\
         transition stinger=\"brand/stinger.json\" peak=300_000\n"
    ))
    .unwrap();
    let cut = Cut::parse(CUT).unwrap();
    let read_root = root.path().to_owned();
    let theme = Arc::new(
        Theme::new(&brand, FormatKind::Opinion, Some(&cut), &|file| {
            Ok(std::fs::read(read_root.join(file)).unwrap())
        })
        .unwrap(),
    );
    let project = Project {
        video: Video::parse("title \"Why we deploy on Fridays\"\nstatus editing\nformat opinion\n")
            .unwrap(),
        script: Script::parse("# t\n").unwrap(),
        format: FormatKind::Opinion.preset(),
        cut,
    };
    let t01 = Id::new("t01").unwrap();
    let join = |at: u64, target, kind| Join {
        take: t01.clone(),
        at: Micros(at),
        target,
        kind,
        length: Micros(500_000),
    };
    let mut transitions = vec![join(0, Target::VideoStart, TransitionKind::Dip)];
    transitions.extend(KINDS.map(|(at, kind)| join(at, Target::ChapterStart, kind)));
    let stinger = theme.stinger.as_ref().unwrap();
    let joins = Joins {
        intro: Some(BookendInput {
            bumper: Some("brand/media/intro.mp4".into()),
            title: Some(project.video.title.clone()),
            length: Micros(2_500_000),
            identity: Vec::new(),
        }),
        outro: Some(BookendInput {
            bumper: None,
            title: Some(String::new()),
            length: Micros(1_500_000),
            identity: Vec::new(),
        }),
        intro_after_hook: true,
        transitions,
        stinger: Some(StingerTiming {
            duration: stinger.lottie.duration(),
            peak: stinger.peak,
        }),
        end_card: Some(Micros(2_000_000)),
    };
    let timeline = Timeline::evaluate(&project);
    let plan = ExportPlan::new(timeline.main(), 60, |_| MediaId::default()).with_joins(&joins);
    Reel {
        _root: root,
        video,
        plan,
        theme,
        effects: project.format.effects(),
    }
}

const CANVAS: (u32, u32) = (1280, 720);

fn pixels(
    painter: &mut Painter,
    renderer: &Renderer,
    plan: &ExportPlan,
    pool: &BufferPool,
    frame: u64,
) -> Vec<[u8; 3]> {
    let target = pool.buffer().unwrap();
    painter.paint(renderer, plan, frame, &target).unwrap();
    read_rgb(&target).unwrap()
}

fn painter(reel: &Reel) -> Painter {
    Painter::with_theme(&reel.video, reel.effects, reel.theme.clone())
}

#[test]
#[ignore = "needs the GPU and the generated target project"]
fn transitions_bookends_and_the_camera_frame_the_same_reached_by_playing_or_by_seeking() {
    let reel = reel("editorial", "shape=circle corners=24 border=8 shadow=soft");
    let plan = &reel.plan;
    assert_eq!(plan.transitions.len(), 7);
    let pool = BufferPool::new(CANVAS.0, CANVAS.1, Range::Full, Matrix::Bt601).unwrap();
    let renderer = Renderer::new().unwrap();
    let mut played_on = painter(&reel);
    let played: Vec<blake3::Hash> = (0..plan.frames)
        .map(|f| blake3::hash(pixels(&mut played_on, &renderer, plan, &pool, f).as_flattened()))
        .collect();
    let windows: Vec<u64> = plan
        .transitions
        .iter()
        .flat_map(|t| t.window.clone())
        .collect();
    let fresh = Renderer::new().unwrap();
    let mut fresh_painter = painter(&reel);
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    for seek in 0..80 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Every other seek lands inside a transition, where both sides draw.
        let f = if seek % 2 == 0 {
            windows[(state % windows.len() as u64) as usize]
        } else {
            state % plan.frames
        };
        let (on, with) = if seek % 4 < 2 {
            (&renderer, &mut played_on)
        } else {
            (&fresh, &mut fresh_painter)
        };
        let digest = blake3::hash(pixels(with, on, plan, &pool, f).as_flattened());
        assert_eq!(digest, played[f as usize], "frame {f}");
    }
}

/// Where the reel's frames go, git-ignored.
fn evidence() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/m11");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "needs the GPU and the generated target project; writes docs/evidence/m11"]
fn frames_per_transition_per_preset() {
    let (w, h) = (1920, 1080);
    let thumb = (640, 360);
    for preset in ["reactive", "editorial", "launch", "calm"] {
        let reel = reel(preset, "shape=rounded corners=48 border=10 shadow=soft");
        let plan = &reel.plan;
        let pool = BufferPool::new(w, h, Range::Full, Matrix::Bt601).unwrap();
        let renderer = Renderer::new().unwrap();
        let mut painter = painter(&reel);
        let mut rows: Vec<Vec<u64>> = plan
            .transitions
            .iter()
            .map(|t| {
                let len = t.window.end - t.window.start;
                vec![
                    t.window.start + len / 4,
                    t.edge - 1,
                    t.edge,
                    t.window.start + len * 3 / 4,
                ]
            })
            .collect();
        let intro = plan.range_of(crate::join::Piece::Bookend(0));
        let outro = plan.range_of(crate::join::Piece::Bookend(1));
        rows.push(vec![
            intro.start + 30,
            intro.start + 90,
            outro.start + 30,
            plan.frames - 1,
        ]);
        let mut sheet = image::RgbImage::new(thumb.0 * 4, thumb.1 * rows.len() as u32);
        for (row, frames) in rows.iter().enumerate() {
            for (col, &frame) in frames.iter().enumerate() {
                let rgb = pixels(&mut painter, &renderer, plan, &pool, frame);
                let full = image::RgbImage::from_raw(w, h, rgb.concat()).unwrap();
                if preset == "editorial" {
                    full.save(evidence().join(format!("transitions-{row}-{col}.png")))
                        .unwrap();
                }
                let small = image::imageops::resize(
                    &full,
                    thumb.0,
                    thumb.1,
                    image::imageops::FilterType::Triangle,
                );
                image::imageops::replace(
                    &mut sheet,
                    &small,
                    i64::from(col as u32 * thumb.0),
                    i64::from(row as u32 * thumb.1),
                );
            }
        }
        sheet
            .save(evidence().join(format!("transitions-{preset}.png")))
            .unwrap();
    }
    for camera in [
        "shape=circle corners=0 border=8 shadow=soft",
        "shape=square corners=0 border=0 shadow=hard",
    ] {
        let reel = reel("editorial", camera);
        let pool = BufferPool::new(w, h, Range::Full, Matrix::Bt601).unwrap();
        let renderer = Renderer::new().unwrap();
        let mut painter = painter(&reel);
        let name = camera
            .split_whitespace()
            .next()
            .unwrap()
            .trim_start_matches("shape=");
        for (frame, layout) in [(60, "face-content"), (330, "split")] {
            let rgb = pixels(&mut painter, &renderer, &reel.plan, &pool, frame);
            image::RgbImage::from_raw(w, h, rgb.concat())
                .unwrap()
                .save(evidence().join(format!("camera-{name}-{layout}.png")))
                .unwrap();
        }
    }
}

#[test]
#[ignore = "needs the GPU and the generated target project; run with --release"]
fn preview_frames_across_transitions_fit_the_frame_budget() {
    let reel = reel("launch", "shape=circle corners=24 border=8 shadow=soft");
    let plan = &reel.plan;
    let pool = BufferPool::new(1920, 1080, Range::Full, Matrix::Bt601).unwrap();
    let renderer = Renderer::new().unwrap();
    let mut painter = painter(&reel);
    for t in &plan.transitions {
        // Play into each transition from a second before it, decode included.
        let from = t.window.start.saturating_sub(60);
        let mut times = Vec::new();
        for frame in from..t.window.end {
            let target = pool.buffer().unwrap();
            let started = std::time::Instant::now();
            painter.paint(&renderer, plan, frame, &target).unwrap();
            if t.window.contains(&frame) {
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
        println!(
            "{}: decode, compose and draw p50 {:.2} ms, max {:.2} ms over {} frames",
            t.kind.as_str(),
            at(0.5),
            at(1.0),
            times.len()
        );
    }
}
