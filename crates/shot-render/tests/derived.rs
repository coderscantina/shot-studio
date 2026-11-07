//! Derived cuts and overlays through the real export. Needs `SHOT_FIXTURES`, the hardware
//! encoder, and for the face test `swift` and `ffmpeg` to generate a camera clip.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use shot_core::{Cut, Format, Id, Micros, Project, Script, Timeline, Video};
use shot_render::derived::{self, DerivedInputs, DerivedPlan, FaceTrack};
use shot_render::export::{ExportOptions, ExportReport, export};

fn project(dir: &Path, media: &Path, cut: &str) -> Project {
    if !dir.join("media").exists() {
        std::os::unix::fs::symlink(media, dir.join("media")).unwrap();
    }
    Project {
        video: Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
        script: Script::parse("# t\n").unwrap(),
        format: Format::parse(
            &std::fs::read_to_string(fixtures().join("target-project/format.kdl")).unwrap(),
        )
        .unwrap(),
        cut: Cut::parse(cut).unwrap(),
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
}

fn run(dir: &Path, project: &Project, cut: &str) -> ExportReport {
    export(
        dir,
        project,
        cut,
        &dir.join(format!("{cut}.mp4")),
        ExportOptions::default(),
        &|_| {},
    )
    .unwrap()
}

const CUT: &str = r#"chapter "One" {
    take t01 {
        keep from=800_000 to=4_800_000
        layout split from=2_800_000 to=4_800_000
        overlay link-card from=1_000_000 to=2_000_000 {
            url "https://github.com/coderscantina/shot-studio/pull/1"
        }
        overlay link-card from=3_000_000 to=3_900_000 {
            url "https://news.ycombinator.com/item?id=1"
        }
    }
}

short s {
    clip t01 from=1_300_000 to=4_100_000
}
"#;

#[test]
#[ignore = "needs the generated target project and the hardware HEVC encoder"]
fn a_short_reuses_main_chunks_and_a_parameter_re_renders_only_its_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let media = fixtures().join("target-project/media");
    let first = project(dir.path(), &media, CUT);
    let main = run(dir.path(), &first, "main");
    // 0.8 s to 4.8 s, split at 2.8 s: 48..60, then 4 chunks, 168..180, then 3 chunks + 1.
    assert_eq!((main.chunks, main.rendered), (10, 10));

    // 1.3 s to 4.1 s: frames 78..246 in eight derived chunks, all inside main chunks.
    let short = run(dir.path(), &first, "s");
    assert_eq!((short.chunks, short.rendered), (8, 8));
    assert_eq!((short.base_chunks, short.base_rendered), (8, 0));
    let again = run(dir.path(), &first, "s");
    assert_eq!(again.rendered, 0);

    // The second card covers 3 s to 3.9 s: chunks 180..210 and 210..240 of the split segment.
    let changed = CUT.replace("item?id=1", "item?id=2");
    let second = project(dir.path(), &media, &changed);
    let main = run(dir.path(), &second, "main");
    assert_eq!(main.rendered, 2);
    let short = run(dir.path(), &second, "s");
    assert_eq!((short.rendered, short.base_rendered), (2, 0));
}

/// Draws the Apple Color Emoji person with medium skin tone, which Vision's face detector finds
/// (the flat-shaded fixture face and the other person emoji it does not) at a size it finds in a
/// 1080p frame, and moves it across a 10 s 1080p60 camera clip, left to right.
fn moving_face_clip(dir: &Path) -> PathBuf {
    let swift = dir.join("face.swift");
    std::fs::write(
        &swift,
        r#"import AppKit
let size = 910
let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
    space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
("🧑🏽" as NSString).draw(at: NSPoint(x: 10, y: 20), withAttributes: [.font: NSFont(name: "Apple Color Emoji", size: 700)!])
let rep = NSBitmapImageRep(cgImage: ctx.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
"#,
    )
    .unwrap();
    let png = dir.join("face.png");
    let status = Command::new("swift")
        .arg(&swift)
        .arg(&png)
        .status()
        .expect("swift runs");
    assert!(status.success());
    let camera = dir.join("media/t01/camera.mp4");
    std::fs::create_dir_all(camera.parent().unwrap()).unwrap();
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("color=c=0x3a4a6b:s=1920x1080:r=60:d=10")
        .arg("-i")
        .arg(&png)
        .args([
            "-filter_complex",
            "[0:v][1:v]overlay=x='100+80*t':y=80,format=yuv420p",
            "-frames:v",
            "600",
            "-c:v",
            "libx265",
            "-preset",
            "superfast",
            "-x265-params",
            "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0",
            "-tag:v",
            "hvc1",
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof",
        ])
        .arg(&camera)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());
    camera
}

#[test]
#[ignore = "needs swift, ffmpeg with libx265 and Vision"]
fn reframing_keeps_the_face_inside_the_short() {
    let dir = tempfile::tempdir().unwrap();
    moving_face_clip(dir.path());
    let take = Id::new("t01").unwrap();
    let cut = "chapter \"One\" {\n    take t01 {\n        keep from=0 to=10_000_000\n        layout face from=0 to=10_000_000\n    }\n}\n\nshort s {\n    clip t01 from=0 to=10_000_000\n}\n";
    let project = Project {
        video: Video::parse("title \"t\"\nstatus editing\nformat opinion\n").unwrap(),
        script: Script::parse("# t\n").unwrap(),
        format: shot_core::FormatKind::Opinion.preset(),
        cut: Cut::parse(cut).unwrap(),
    };
    let timeline = Timeline::evaluate(&project);
    let short = &timeline.outputs[1];
    let times = derived::face_times(short).remove(&take).unwrap();
    let track = shot_analyze::vision::face_track(dir.path(), &take, &times).unwrap();
    let found = track.samples.iter().filter(|(_, f)| f.is_some()).count();
    assert!(
        found * 10 >= track.samples.len() * 9,
        "Vision found a face in {found} of {} samples",
        track.samples.len()
    );
    let faces = std::collections::HashMap::from([(
        take.clone(),
        FaceTrack {
            size: track.size,
            samples: track.samples.clone(),
        },
    )]);
    let plan = DerivedPlan::new(
        timeline.main(),
        short,
        &DerivedInputs {
            fps: 60,
            media: &|_| shot_core::MediaId::default(),
            assets: &|_| Vec::new(),
            faces: &faces,
            transcripts: &std::collections::HashMap::new(),
        },
    );
    let width = derived::window_width(shot_core::DerivedKind::Short);
    let mut checked = 0;
    for (t, face) in &track.samples {
        let Some(face) = face else { continue };
        if *t >= Micros(10_000_000) {
            continue;
        }
        // In the face layout the 1920x1080 camera fills the 3840x2160 canvas.
        let (left, right) = (face.x * 3840.0, (face.x + face.w) * 3840.0);
        let frame = (t.0 * 60 / 1_000_000) as usize;
        let window = plan
            .chunks
            .iter()
            .flatten()
            .flat_map(|c| &c.windows)
            .nth(frame)
            .copied()
            .map(f32::from)
            .unwrap();
        assert!(
            window <= left && right <= window + width,
            "{t:?}: face {left}..{right} outside window {window}..{}",
            window + width
        );
        checked += 1;
    }
    assert!(checked >= 18, "{checked} samples checked");
    println!(
        "Vision found the face in {found} of {} samples, all {checked} inside the window",
        track.samples.len()
    );
}
