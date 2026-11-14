//! Product video effects on the generated screen fixture and target project. Need `SHOT_FIXTURES`,
//! the GPU and, for export, the hardware encoder. Set `SHOT_EVIDENCE` to a folder to keep the
//! compared frames there as PPM files.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use shot_core::{
    CursorStyle, Cut, FormatKind, Keystrokes, Layout, Lock, Micros, Project, Script, Video,
};
use shot_media::videotoolbox::Decoder;
use shot_render::assets::Assets;
use shot_render::export::{ExportOptions, export};
use shot_render::frame::{Shot, compose};
use shot_render::graph::Renderer;
use shot_render::layout::{Canvas, Source, Sources};
use shot_render::surface::{BufferPool, Matrix, Range, read_rgb, size};

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
}

/// The zooms the auto-zoom rule wrote on the fixture's first click and typed run, centered so
/// the cursor stays in view.
const ZOOMS: &str = "        zoom 160 from=900_000 to=2_500_000 x=1200 y=675
        zoom 160 from=2_500_000 to=6_000_000 x=1992 y=900
";

/// A product tour of the 30 s screen fixture, `length` long, with [`ZOOMS`] and every
/// keystroke shown.
fn screen_project(length: u64) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().unwrap();
    let take = dir.path().join("media/t01");
    std::fs::create_dir_all(&take).unwrap();
    let link = |from: PathBuf, to: &str| std::os::unix::fs::symlink(from, take.join(to)).unwrap();
    link(fixtures().join("screen-30s.mp4"), "screen.mp4");
    link(fixtures().join("screen-30s.events.jsonl"), "events.jsonl");
    link(
        fixtures().join("target-project/media/t01/mic.mp4"),
        "mic.mp4",
    );
    let mut format = FormatKind::ProductTour.preset();
    format.keystrokes = Some(Keystrokes::All);
    let cut = Cut::parse(&format!(
        "chapter \"Tour\" {{\n    take t01 {{\n        keep from=0 to={length}\n{ZOOMS}    }}\n}}\n"
    ))
    .unwrap();
    let project = Project {
        video: Video::parse("title \"Tour\"\nstatus editing\nformat product-tour\n").unwrap(),
        script: Script::parse("# Tour\n").unwrap(),
        format,
        cut,
    };
    std::fs::write(dir.path().join("cut.kdl"), project.cut.to_kdl()).unwrap();
    std::fs::write(dir.path().join("cut.lock"), Lock::default().to_kdl()).unwrap();
    (dir, project)
}

/// Mean and 99th percentile of the absolute channel difference, in 8-bit code values.
fn difference(a: &[[u8; 3]], b: &[[u8; 3]]) -> (f64, u8) {
    let mut all: Vec<u8> = a
        .iter()
        .zip(b)
        .flat_map(|(a, b)| (0..3).map(move |i| a[i].abs_diff(b[i])))
        .collect();
    let mean = all.iter().map(|&d| f64::from(d)).sum::<f64>() / all.len() as f64;
    let p99 = all.len() * 99 / 100;
    let (_, p99, _) = all.select_nth_unstable(p99);
    (mean, *p99)
}

/// `pixels` of `width` halved in both directions by averaging each 2x2 block.
fn halve(pixels: &[[u8; 3]], width: usize) -> Vec<[u8; 3]> {
    let height = pixels.len() / width;
    let mut out = Vec::with_capacity(pixels.len() / 4);
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            let at = |dx, dy| pixels[(y + dy) * width + x + dx];
            let block = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
            out.push(
                [0, 1, 2].map(|i| (block.iter().map(|p| u32::from(p[i])).sum::<u32>() / 4) as u8),
            );
        }
    }
    out
}

fn save(name: &str, pixels: &[[u8; 3]], width: usize) {
    let Ok(dir) = std::env::var("SHOT_EVIDENCE") else {
        return;
    };
    let mut ppm = format!("P6\n{width} {}\n255\n", pixels.len() / width).into_bytes();
    ppm.extend(pixels.iter().flatten());
    std::fs::write(Path::new(&dir).join(format!("{name}.ppm")), ppm).unwrap();
}

#[test]
#[ignore = "needs the generated screen fixture, the GPU and the hardware HEVC encoder"]
fn preview_and_export_show_the_same_frames() {
    let (dir, project) = screen_project(6_000_000);
    let out = dir.path().join("out.mp4");
    export(
        dir.path(),
        &project,
        "main",
        &out,
        ExportOptions::default(),
        &|_| {},
    )
    .unwrap();
    let mut exported = Decoder::open(&out).unwrap();
    let mut screen = Decoder::open_native(&dir.path().join("media/t01/screen.mp4")).unwrap();
    let renderer = Renderer::new().unwrap();
    let preview = Canvas {
        width: 1920,
        height: 1080,
    };
    let pool = BufferPool::new(preview.width, preview.height, Range::Full, Matrix::Bt601).unwrap();
    let mut assets = Assets::new(dir.path());
    let edit = &project.cut.chapters[0].takes[0];
    let effects = project.format.effects();
    let look = assets.look(&edit.take, &edit.overlays, effects).unwrap();
    // A zoom easing in, a ripple and the typed run, a pan between zooms, a click on the right.
    for frame in [70u64, 186, 240, 296, 306] {
        let time = Duration::from_micros(frame * 1_000_000 / 60);
        // Frame times round down to whole microseconds; a millisecond more lands on the frame.
        let seek = time + Duration::from_millis(1);
        let source = screen.seek(seek, || false).unwrap().unwrap();
        let shot = Shot {
            layout: Layout::Window,
            zooms: &edit.zooms,
            overlays: &edit.overlays,
            time: Micros(time.as_micros() as u64),
            switch: None,
        };
        let sizes = Sources {
            screen: size(&source.buffer),
            camera: (1, 1),
        };
        let drawn = compose(&shot, &look, preview, sizes).unwrap();
        assert!(
            drawn
                .scene
                .layers
                .iter()
                .all(|l| l.source == Source::Screen)
        );
        let target = pool.buffer().unwrap();
        renderer
            .draw(
                &drawn,
                Sources {
                    screen: Some(&*source.buffer),
                    camera: None,
                },
                &target,
            )
            .unwrap();
        let previewed = read_rgb(&target).unwrap();
        let encoded = exported.seek(seek, || false).unwrap().unwrap();
        let (width, _) = size(&encoded.buffer);
        let full = read_rgb(&encoded.buffer).unwrap();
        let halved = halve(&full, width as usize);
        let (mean, p99) = difference(&previewed, &halved);
        println!("frame {frame}: mean {mean:.2}, p99 {p99}");
        save(&format!("frame-{frame}-preview"), &previewed, 1920);
        save(&format!("frame-{frame}-export"), &full, width as usize);
        // HEVC at the master quality and the different sampling of the two canvases leave about
        // one code value on average, more along sharp edges.
        assert!(
            mean < 2.0 && p99 < 24,
            "frame {frame} differs: mean {mean:.2}, p99 {p99}"
        );
    }
}

#[test]
#[ignore = "needs the generated target project and the hardware HEVC encoder"]
fn a_cursor_style_change_re_renders_only_chunks_that_show_the_screen() {
    let source = fixtures().join("target-project");
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(source.join("media"), dir.path().join("media")).unwrap();
    std::fs::write(dir.path().join("cut.lock"), Lock::default().to_kdl()).unwrap();
    let read = |name: &str| std::fs::read_to_string(source.join(name)).unwrap();
    // 2 s of `face`, the format's default, then 2 s of `split`.
    let cut = Cut::parse(
        "chapter \"A\" {\n    take t01 {\n        keep from=800_000 to=4_800_000\n        layout split from=2_800_000 to=4_800_000\n    }\n}\n",
    )
    .unwrap();
    let mut project = Project {
        video: Video::parse(&read("video.kdl")).unwrap(),
        script: Script::parse(&read("script.md")).unwrap(),
        format: FormatKind::Opinion.preset(),
        cut,
    };
    let out = dir.path().join("out.mp4");
    let run = |project: &Project| {
        export(
            dir.path(),
            project,
            "main",
            &out,
            ExportOptions::default(),
            &|_| {},
        )
        .unwrap()
    };
    let first = run(&project);
    assert_eq!(first.rendered, first.chunks);
    project.format.cursor = Some(CursorStyle {
        size: 160,
        smoothing: Micros(120_000),
        ripple: true,
    });
    let second = run(&project);
    // 48..60, 60..90, ..., 150..168 show the face; 168..180 to 270..288 show the split.
    assert_eq!((second.chunks, second.rendered), (10, 5));
    println!(
        "first export {:?} for {} chunks, cursor change {:?} for {} of {} chunks",
        first.render_time, first.rendered, second.render_time, second.rendered, second.chunks
    );
}
