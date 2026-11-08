//! The reel Mike scores (milestone 11 item 13): per preset, a short video over the generated
//! target take with every template, every transition, the kinetic type moments, the camera's
//! frame, both bookends and the end card, built into a `cut.lock` from a brand and exported at
//! 4K60 with its sound and music through the real export. Tests only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use shot_core::{
    Applied, Brand, ContentHash, Cut, Format, FormatLayer, Micros, Project, Script, Target,
    TransitionKind, Video,
};

use crate::export::{ExportOptions, export, preview_plan};
use crate::join::{BookendKind, Piece};

/// Seven chapters of the fixture take, each joined to the piece before it by another
/// transition, with every template placed on them.
const CUT: &str = r#"
chapter "Hook" {
    take t01 {
        keep from=10_000_000 to=18_000_000
        layout face-content from=10_000_000 to=18_000_000
        overlay pop from=11_000_000 to=12_400_000 {
            text "🔥"
        }
        overlay lower-third from=12_600_000 to=17_400_000 {
            text "Mike Wallner"
            subtitle "Coder's Cantina"
        }
    }
}
chapter "Why Fridays" {
    take t01 {
        keep from=100_000_000 to=110_000_000
        layout split from=100_000_000 to=110_000_000
        overlay link-card from=103_000_000 to=109_000_000 {
            url "https://github.com/coderscantina/shot-studio/pull/1"
            accent #true
        }
    }
}
chapter "The checks" {
    take t01 {
        keep from=240_000_000 to=250_000_000
        layout face-content from=240_000_000 to=250_000_000
        overlay highlight from=242_800_000 to=246_400_000 {
            x 820
            y 412
            width 1_340
            height 64
        }
        overlay callout from=245_600_000 to=249_400_000 {
            x 2_200
            y 600
            width 900
            height 120
            text "Deploy button"
        }
    }
}
chapter "Deploying" {
    take t01 {
        keep from=300_000_000 to=310_000_000
        layout content from=300_000_000 to=310_000_000
        zoom 150 from=301_500_000 to=306_500_000 x=1_800 y=700
        overlay callout from=302_200_000 to=305_800_000 {
            x 1_900
            y 760
            text "Click here"
        }
        overlay pop from=306_800_000 to=308_600_000 {
            text "Ship it"
            x 1_920
            y 1_080
            size 300
        }
    }
}
chapter "The dashboard" {
    take t01 {
        keep from=360_000_000 to=370_000_000
        layout split from=360_000_000 to=370_000_000
        overlay b-roll from=362_600_000 to=365_800_000 {
            file "sources/reel.png"
        }
        overlay blur from=366_000_000 to=369_500_000 {
            x 2_400
            y 1_500
            width 800
            height 200
        }
    }
}
chapter "The diff" {
    take t01 {
        keep from=510_000_000 to=520_000_000
        layout face from=510_000_000 to=520_000_000
        overlay code-diff from=511_000_000 to=518_600_000 {
            diff "sources/reel.diff"
            highlight-line 9
            highlight-at 516_000_000
        }
        overlay bleep from=514_000_000 to=514_400_000
    }
}
chapter "Shipping" {
    take t01 {
        keep from=660_000_000 to=672_000_000
        layout content from=660_000_000 to=672_000_000
        overlay pop from=661_000_000 to=663_000_000 {
            sticker "star"
            text "NEW"
        }
        overlay badge from=663_400_000 to=667_400_000 {
            text "v2 ships Friday"
        }
    }
}
"#;

/// How each chapter after the first joins the piece before it, by its start in the take.
const TRANSITIONS: [(u64, TransitionKind); 6] = [
    (100_000_000, TransitionKind::Dissolve),
    (240_000_000, TransitionKind::MaskWipe),
    (300_000_000, TransitionKind::ZoomThrough),
    (360_000_000, TransitionKind::Stinger),
    (510_000_000, TransitionKind::Cut),
    (660_000_000, TransitionKind::Dip),
];

/// The camera's frame per preset, so the four reels show every shape.
pub(crate) const PRESETS: [(&str, &str); 4] = [
    ("reactive", "shape=circle corners=0 border=8 shadow=soft"),
    ("editorial", "shape=rounded corners=48 border=0 shadow=soft"),
    ("launch", "shape=rounded corners=32 border=6 shadow=soft"),
    ("calm", "shape=square corners=0 border=0 shadow=hard"),
];

fn brand(preset: &str, camera: &str) -> String {
    format!(
        r##"colors accent="#c8f25a" text="#f4f6f8" card="#141821" background="#101418"
logo "brand/logo.svg"
sounds transition=swoosh transition-impact=impact
intro bumper="brand/media/intro.mp4" title=#true
outro bumper="brand/media/outro.json"
end-card length=8_000_000
camera {camera}
motion opinion={preset}
transition stinger="brand/stinger.json" peak=300_000
template badge file="brand/badge.json" {{
    slot text text required=#true
    slot accent color
}}
template star file="brand/star.json" {{
    slot text text
    slot accent color
}}
on video-start {{
    transition dip
    music "sources/bed.flac" gain=-20 duck=-12
}}
on chapter-start {{
    overlay chapter-band for=2_500_000
    transition dissolve
}}
"##
    )
}

fn prop(k: serde_json::Value) -> serde_json::Value {
    json!({ "a": 0, "k": k })
}

fn eased(t: u32, s: serde_json::Value) -> serde_json::Value {
    json!({ "t": t, "s": s, "o": { "x": [0.3], "y": [0.0] }, "i": { "x": [0.2], "y": [1.0] } })
}

fn transform(
    position: [u32; 2],
    scale: serde_json::Value,
    opacity: serde_json::Value,
) -> serde_json::Value {
    json!({ "a": prop(json!([0, 0])), "s": scale, "o": opacity, "r": prop(json!(0)),
            "p": prop(json!(position)) })
}

fn group(shape: serde_json::Value, fill: serde_json::Value) -> serde_json::Value {
    json!([{ "ty": "gr", "it": [
        shape,
        { "ty": "fl", "c": fill, "o": prop(json!(100)) },
        { "ty": "tr", "a": prop(json!([0, 0])), "p": prop(json!([0, 0])),
          "s": prop(json!([100, 100])), "r": prop(json!(0)), "o": prop(json!(100)) }
    ] }])
}

/// A full-frame Lottie template: a card in the accent popping in at the lower right, then its
/// `text` slot fading in on it.
fn badge() -> Vec<u8> {
    let pop = json!({ "a": 1, "k": [
        eased(0, json!([0, 0])), eased(14, json!([108, 108])), { "t": 22, "s": [100, 100] }
    ] });
    let fade = json!({ "a": 1, "k": [eased(10, json!([0])), { "t": 22, "s": [100] }] });
    let file = json!({
        "v": "5.7.0", "fr": 60, "ip": 0, "op": 180, "w": 1920, "h": 1080, "layers": [
            { "ty": 5, "nm": "Label", "ind": 1, "ip": 0, "op": 180, "st": 0,
              "ks": transform([1480, 866], prop(json!([100, 100])), fade),
              "t": { "d": { "sid": "text", "k": [{ "t": 0, "s": {
                  "s": 54, "t": "Badge", "j": 2, "fc": [0.06, 0.08, 0.1], "f": "Sans"
              } }] } } },
            { "ty": 4, "nm": "Card", "ind": 2, "ip": 0, "op": 180, "st": 0,
              "ks": transform([1480, 846], pop, prop(json!(100))),
              "shapes": group(
                  json!({ "ty": "rc", "p": prop(json!([0, 0])), "s": prop(json!([620, 150])),
                          "r": prop(json!(36)) }),
                  json!({ "a": 0, "k": [0.78, 0.95, 0.35], "sid": "accent" })) }
        ]
    });
    serde_json::to_vec(&file).expect("json")
}

/// The outro's Lottie bumper: a disc in the accent opening into a ring around the middle.
fn outro() -> Vec<u8> {
    let disc = |ind: u32, size: u32, color: [f32; 3], from: u32, to: u32| {
        let grow =
            json!({ "a": 1, "k": [eased(from, json!([0, 0])), { "t": to, "s": [100, 100] }] });
        json!({ "ty": 4, "nm": format!("Disc {ind}"), "ind": ind, "ip": 0, "op": 120, "st": 0,
                "ks": transform([960, 540], grow, prop(json!(100))),
                "shapes": group(
                    json!({ "ty": "el", "p": prop(json!([0, 0])), "s": prop(json!([size, size])) }),
                    prop(json!(color))) })
    };
    let file = json!({
        "v": "5.7.0", "fr": 60, "ip": 0, "op": 120, "w": 1920, "h": 1080, "layers": [
            disc(1, 760, [0.063, 0.078, 0.094], 16, 64),
            disc(2, 820, [0.78, 0.95, 0.35], 0, 48),
        ]
    });
    serde_json::to_vec(&file).expect("json")
}

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
}

/// A channel repo in a temp dir with the reel's brand, files and a migrated video over the
/// fixture take, its lock built like `shot style build` does with the transitions set per
/// chapter.
pub(crate) struct Reel {
    _root: tempfile::TempDir,
    pub video: PathBuf,
    pub project: Project,
}

impl Reel {
    pub(crate) fn new(preset: &str, camera: &str) -> Self {
        let root = tempfile::tempdir().expect("a temp dir");
        let at = |path: &str| root.path().join(path);
        for dir in ["brand/media", "videos/reel/sources"] {
            std::fs::create_dir_all(at(dir)).expect("dirs");
        }
        let write = |path: &str, bytes: &[u8]| std::fs::write(at(path), bytes).expect("write");
        write("brand/logo.svg", crate::join_reel::LOGO.as_bytes());
        write("brand/stinger.json", &crate::join_reel::stinger());
        write("brand/star.json", &crate::theme::tests::star());
        write("brand/badge.json", &badge());
        write("brand/media/outro.json", &outro());
        let brand_text = brand(preset, camera);
        write("brand/brand.kdl", brand_text.as_bytes());
        // A 3 s bumper: the fixture camera with a tone.
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-t", "3", "-i"])
            .arg(fixtures().join("camera-30s.mp4"))
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=660:sample_rate=48000:duration=3",
            ])
            .args([
                "-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-ac", "1",
            ])
            .args(["-af", "volume=-12dB", "-shortest"])
            .arg(at("brand/media/intro.mp4"))
            .status()
            .expect("ffmpeg runs");
        assert!(status.success(), "ffmpeg made the bumper");

        let video = at("videos/reel");
        std::os::unix::fs::symlink(fixtures().join("target-project/media"), video.join("media"))
            .expect("media link");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../shot-audio/assets/sounds/bed-pulse.flac"),
            video.join("sources/bed.flac"),
        )
        .expect("the bundled bed");
        std::fs::write(video.join(crate::reel::DIFF_FILE), crate::reel::DIFF).expect("diff");
        let still = crate::reel::still();
        image::RgbaImage::from_raw(still.width, still.height, still.data.data().to_vec())
            .expect("still")
            .save(video.join(crate::reel::STILL_FILE))
            .expect("still saved");
        let video_text = "title \"Why we ship on Fridays\"\nstatus editing\nformat opinion\n";
        std::fs::write(video.join("video.kdl"), video_text).expect("video.kdl");
        std::fs::write(video.join("cut.kdl"), CUT).expect("cut.kdl");

        let brand = Brand::parse(&brand_text).expect("the reel's brand");
        let assets: BTreeMap<String, ContentHash> = brand
            .assets()
            .into_iter()
            .map(|file| {
                let bytes = std::fs::read(at(file)).expect("brand file");
                (file.to_owned(), ContentHash::of(&bytes))
            })
            .collect();
        let cut = Cut::parse(CUT).expect("the reel's cut");
        let mut lock = shot_core::build(&shot_core::StyleInput {
            imported: &[],
            brand: &brand,
            format: &FormatLayer::default(),
            video: &FormatLayer::default(),
            cut: &cut,
            elements: &[],
            assets: &assets,
        })
        .expect("the lock builds");
        for instance in lock.takes.iter_mut().flat_map(|t| &mut t.instances) {
            if instance.element.target != Target::ChapterStart {
                continue;
            }
            if let Applied::Transition { kind, at, .. } = &mut instance.applied {
                *kind = TRANSITIONS
                    .iter()
                    .find(|(start, _)| Micros(*start) == *at)
                    .map_or(TransitionKind::Cut, |(_, k)| *k);
            }
        }
        std::fs::write(video.join("cut.lock"), lock.to_kdl()).expect("cut.lock");

        let format_text = std::fs::read_to_string(fixtures().join("target-project/format.kdl"))
            .expect("format.kdl")
            .replace(
                "layouts face split",
                "layouts face face-content split content",
            )
            + "zoom-motion smooth\n";
        std::fs::write(video.join("format.kdl"), &format_text).expect("format.kdl");
        let project = Project {
            video: Video::parse(video_text).expect("video.kdl"),
            script: Script::parse("# Why we ship on Fridays\n").expect("script"),
            format: Format::parse(&format_text).expect("format"),
            cut,
        };
        Self {
            _root: root,
            video,
            project,
        }
    }

    /// What appears when in the exported reel, in output time, as Markdown table rows.
    pub(crate) fn moments(&self) -> Vec<(u64, String)> {
        let plan = preview_plan(&self.video, &self.project).expect("the reel plans");
        let time = |frame: u64| frame * 1_000_000 / u64::from(plan.fps);
        let mut moments = Vec::new();
        for (i, bookend) in plan.bookends.iter().enumerate() {
            let what = match bookend.kind {
                BookendKind::Intro => "intro: video bumper with a tone, the title over it",
                BookendKind::Outro => "outro: Lottie bumper under the end card",
            };
            moments.push((
                time(plan.range_of(Piece::Bookend(i)).start),
                what.to_owned(),
            ));
        }
        if let Some(card) = &plan.end_card {
            moments.push((time(card.start), "end card with the logo".to_owned()));
        }
        for t in &plan.transitions {
            moments.push((
                time(t.window.start),
                format!(
                    "{} transition, cut at {}",
                    t.kind.as_str(),
                    clock(time(t.edge))
                ),
            ));
        }
        for chapter in &plan.chapters {
            moments.push((
                time(chapter.output_frame),
                format!("chapter \"{}\"", chapter.title),
            ));
        }
        for planned in &plan.segments {
            let start = planned.source_frames.start;
            let to_output = |at: Micros| {
                let frame = (at.0 * u64::from(plan.fps)).div_ceil(1_000_000).max(start);
                time(planned.output_frame + frame - start)
            };
            for overlay in &planned.segment.overlays {
                moments.push((
                    to_output(overlay.range.from()),
                    format!("overlay {}", overlay.template),
                ));
            }
            for zoom in &planned.segment.zooms {
                moments.push((
                    to_output(zoom.range.from()),
                    format!("zoom {}%", zoom.scale),
                ));
            }
        }
        moments.sort();
        moments.dedup();
        moments
    }
}

impl Reel {
    /// Each overlay entrance the beat moved, in output microseconds, with how far it lands from
    /// the nearest beat the bed's detection found. The bed starts with the output.
    pub(crate) fn snapped(&self) -> Vec<(u64, u64)> {
        let snapped = preview_plan(&self.video, &self.project).expect("the reel plans");
        let lock = crate::lock::read(&self.video)
            .expect("lock")
            .expect("a lock");
        let plain = shot_core::Timeline::styled(&self.project, &lock);
        let plain = crate::plan::ExportPlan::new(plain.main(), snapped.fps, |_| {
            shot_core::MediaId::default()
        });
        let bed = crate::voice::read_samples(&self.video.join("sources/bed.flac")).expect("bed");
        let beats = shot_audio::beats::detect(&bed).at;
        let length = bed.len() as u64;
        let mut moved = Vec::new();
        for (a, b) in plain.segments.iter().zip(&snapped.segments) {
            for (before, after) in a.segment.overlays.iter().zip(&b.segment.overlays) {
                let first = time_of(b.source_frames.start, snapped.fps);
                let last = time_of(b.source_frames.end, snapped.fps);
                let from = after.range.from().0;
                if before.range.from() == after.range.from() || !(first..last).contains(&from) {
                    continue;
                }
                let at = time_of(b.output_frame, snapped.fps) + from - first;
                let sample = at * 48_000 / 1_000_000;
                let distance = beats
                    .iter()
                    .flat_map(|b| {
                        [
                            b + sample / length * length,
                            b + (sample / length + 1) * length,
                        ]
                    })
                    .map(|b| b.abs_diff(sample) * 1_000_000 / 48_000)
                    .min()
                    .expect("beats");
                moved.push((at, distance));
            }
        }
        moved.dedup();
        moved
    }
}

fn time_of(frame: u64, fps: u32) -> u64 {
    frame * 1_000_000 / u64::from(fps)
}

/// `m:ss.ss` of a time in microseconds.
pub(crate) fn clock(t: u64) -> String {
    format!("{}:{:05.2}", t / 60_000_000, (t % 60_000_000) as f64 / 1e6)
}

fn evidence() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/m11");
    std::fs::create_dir_all(&dir).expect("evidence dir");
    dir
}

#[test]
#[ignore = "needs the generated target project, ffmpeg and the hardware HEVC encoder; writes docs/evidence/m11"]
fn reel_per_preset_at_4k60() {
    for (preset, camera) in PRESETS {
        let reel = Reel::new(preset, camera);
        let out = evidence().join(format!("reel-{preset}.mp4"));
        let report = export(
            &reel.video,
            &reel.project,
            "main",
            &out,
            ExportOptions::default(),
            &|_| {},
        )
        .expect("the reel exports");
        let total = report.render_time + report.join_time;
        println!(
            "{preset}: {} frames ({:.1} s), {} chunks, {:.1} fps",
            report.frames,
            report.duration().as_secs_f64(),
            report.chunks,
            report.frames as f64 / total.as_secs_f64()
        );
        for (at, what) in reel.moments() {
            println!("| {} | {what} |", clock(at));
        }
        let snapped = reel.snapped();
        println!(
            "{preset}: {} entrances on the beat, at most {} µs from a detected beat",
            snapped.len(),
            snapped.iter().map(|(_, d)| *d).max().unwrap_or(0)
        );
        assert!(
            snapped.iter().all(|(_, d)| *d <= 1_000_000 / 60),
            "{snapped:?}"
        );
    }
}

#[test]
#[ignore = "needs the GPU, ffmpeg and the generated target project"]
fn every_reel_frame_is_the_same_reached_by_playing_or_by_seeking() {
    use crate::graph::Renderer;
    use crate::paint::Painter;
    use crate::surface::{BufferPool, Matrix, Range, read_rgb};

    let (preset, camera) = PRESETS[1];
    let reel = Reel::new(preset, camera);
    let plan = preview_plan(&reel.video, &reel.project).expect("the reel plans");
    let effects = reel.project.format.effects();
    let pool = BufferPool::new(1280, 720, Range::Full, Matrix::Bt601).expect("a pool");
    let digest = |painter: &mut Painter, renderer: &Renderer, frame: u64| {
        let target = pool.buffer().expect("a buffer");
        painter
            .paint(renderer, &plan, frame, &target)
            .expect("the frame paints");
        blake3::hash(read_rgb(&target).expect("pixels").as_flattened())
    };
    let renderer = Renderer::new().expect("a renderer");
    let mut playing = Painter::new(&reel.video, effects);
    let played: Vec<blake3::Hash> = (0..plan.frames)
        .map(|f| digest(&mut playing, &renderer, f))
        .collect();
    let windows: Vec<u64> = plan
        .transitions
        .iter()
        .flat_map(|t| t.window.clone())
        .collect();
    let fresh = Renderer::new().expect("a renderer");
    let mut seeking = Painter::new(&reel.video, effects);
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for seek in 0..200 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // A quarter of the seeks land inside a transition, where both sides may draw.
        let f = if seek % 4 == 0 {
            windows[(state % windows.len() as u64) as usize]
        } else {
            state % plan.frames
        };
        let hash = if seek % 2 == 0 {
            digest(&mut playing, &renderer, f)
        } else {
            digest(&mut seeking, &fresh, f)
        };
        assert_eq!(
            hash,
            played[f as usize],
            "frame {f} ({})",
            clock(time_of(f, plan.fps))
        );
    }
}
