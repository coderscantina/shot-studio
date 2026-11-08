//! The camera's cut-out (decision 0034) on the 30 s fixtures. Need `SHOT_FIXTURES` and the GPU.
//! Vision finds no person in the fixtures' stand-in face, so the tests write the masks a person
//! of its shape would have. Set `SHOT_EVIDENCE` to a folder to keep the frames there as PPM.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_analyze::masks::PersonMasks;
use shot_core::{Brand, Cut, FormatKind, Id, MediaId, Micros, Project, Script, Timeline, Video};
use shot_render::graph::Renderer;
use shot_render::paint::Painter;
use shot_render::plan::ExportPlan;
use shot_render::surface::{BufferPool, Matrix, Range, read_rgb};
use shot_render::theme::Theme;

const CANVAS: (u32, u32) = (1280, 720);
/// The face-content camera slot at [`CANVAS`]: the 4K slot over 3.
const SLOT: [f32; 4] = [968.0, 534.0, 1256.0, 696.0];
const MASK: (u32, u32) = (256, 144);
/// Frame 300 at 60 fps, the stand-in's mouth open.
const FRAME: u64 = 300;

fn fixtures() -> PathBuf {
    PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
}

/// A video of the 30 s screen and camera in `layout`, and its main cut's plan at 60 fps.
fn video(layout: &str) -> (tempfile::TempDir, Project, ExportPlan) {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media/t01");
    std::fs::create_dir_all(&media).unwrap();
    for (from, to) in [
        ("screen-30s.mp4", "screen.mp4"),
        ("camera-30s.mp4", "camera.mp4"),
        ("screen-30s.events.jsonl", "events.jsonl"),
    ] {
        std::os::unix::fs::symlink(fixtures().join(from), media.join(to)).unwrap();
    }
    let cut = format!(
        "chapter \"One\" {{\n    take t01 {{\n        keep from=0 to=10_000_000\n        layout {layout} from=0 to=10_000_000\n    }}\n}}\n"
    );
    let video = "title \"Cut out\"\nstatus editing\nformat opinion\n";
    std::fs::write(dir.path().join("video.kdl"), video).unwrap();
    std::fs::write(dir.path().join("cut.kdl"), &cut).unwrap();
    let project = Project {
        video: Video::parse(video).unwrap(),
        script: Script::parse("# t\n").unwrap(),
        format: FormatKind::Opinion.preset(),
        cut: Cut::parse(&cut).unwrap(),
    };
    let plan = ExportPlan::new(Timeline::evaluate(&project).main(), 60, |_| {
        MediaId::default()
    });
    (dir, project, plan)
}

/// Whether the stand-in face of `scripts/fixtures.sh` covers a point of the 1920x1080 camera:
/// hair and head, neck, shoulders.
fn person(x: f32, y: f32) -> bool {
    let boxes = [
        [700.0, 150.0, 1220.0, 740.0],
        [880.0, 680.0, 1040.0, 820.0],
        [560.0, 800.0, 1360.0, 1080.0],
    ];
    boxes
        .iter()
        .any(|[l, t, r, b]| (*l..*r).contains(&x) && (*t..*b).contains(&y))
}

/// Masks of the stand-in on every 60th of a second of the take, at `confidence`.
fn stand_in(confidence: u8) -> PersonMasks {
    let (w, h) = MASK;
    let alpha: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            if person(x * 1920.0 / w as f32, y * 1080.0 / h as f32) {
                255
            } else {
                0
            }
        })
        .collect();
    PersonMasks::new(
        w,
        h,
        (0..600).map(|f| (Micros(f * 1_000_000 / 60), confidence, alpha.clone())),
    )
}

fn theme(cutout: bool) -> Arc<Theme> {
    let brand = Brand {
        cutout: Some(cutout),
        ..Brand::default()
    };
    Arc::new(Theme::new(&brand, FormatKind::Opinion, None, &|_| unreachable!()).unwrap())
}

/// Frame [`FRAME`] of `plan` painted as the editor's preview paints it.
fn paint(dir: &Path, project: &Project, plan: &ExportPlan, cutout: bool) -> Vec<[u8; 3]> {
    let renderer = Renderer::new().unwrap();
    let pool = BufferPool::new(CANVAS.0, CANVAS.1, Range::Full, Matrix::Bt601).unwrap();
    let target = pool.buffer().unwrap();
    Painter::with_theme(dir, project.format.effects(), theme(cutout))
        .paint(&renderer, plan, FRAME, &target)
        .unwrap();
    read_rgb(&target).unwrap()
}

fn keep(name: &str, pixels: &[[u8; 3]]) {
    let Ok(dir) = std::env::var("SHOT_EVIDENCE") else {
        return;
    };
    let mut ppm = format!("P6\n{} {}\n255\n", CANVAS.0, CANVAS.1).into_bytes();
    ppm.extend(pixels.iter().flatten());
    std::fs::write(Path::new(&dir).join(format!("{name}.ppm")), ppm).unwrap();
}

/// Whether the canvas pixel `(x, y)` is clearly the person (`Some(true)`), clearly background
/// (`Some(false)`) or near an edge, where filtering and 4:2:0 chroma blend the two.
fn decided(x: u32, y: u32) -> Option<bool> {
    let [left, top, right, bottom] = SLOT;
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let margin = 3.0;
    if px < left - margin || px > right + margin || py < top - margin || py > bottom + margin {
        return Some(false);
    }
    if px < left + margin || px > right - margin || py < top + margin || py > bottom - margin {
        return None;
    }
    let to_camera = |p: f32, from: f32, to: f32, size: f32| (p - from) / (to - from) * size;
    let (cx, cy) = (
        to_camera(px, left, right, 1920.0),
        to_camera(py, top, bottom, 1080.0),
    );
    // Two canvas pixels around the point, in camera pixels.
    let reach = 2.0 * 1920.0 / (right - left);
    let corners = [-reach, reach].map(|dx| [-reach, reach].map(|dy| person(cx + dx, cy + dy)));
    let all = corners.as_flattened();
    if all.iter().all(|&p| p) {
        Some(true)
    } else if all.iter().all(|&p| !p) {
        Some(false)
    } else {
        None
    }
}

#[test]
#[ignore = "needs the GPU and the generated 30 s fixtures"]
fn the_cut_out_person_stands_over_the_screen() {
    let (dir, project, plan) = video("face-content");
    let take = Id::new("t01").unwrap();
    let plain = paint(dir.path(), &project, &plan, false);
    stand_in(255).write(dir.path(), &take).unwrap();
    let cut = paint(dir.path(), &project, &plan, true);
    let (content_dir, content_project, content_plan) = video("content");
    let screen = paint(content_dir.path(), &content_project, &content_plan, false);
    keep("cutout-plain", &plain);
    keep("cutout-over-screen", &cut);
    // Where the person is the plain camera shows, elsewhere the screen; edges are left out.
    let mut expected = cut.clone();
    let mut background = 0;
    for (i, pixel) in expected.iter_mut().enumerate() {
        let (x, y) = (i as u32 % CANVAS.0, i as u32 / CANVAS.0);
        match decided(x, y) {
            Some(true) => *pixel = plain[i],
            Some(false) => {
                *pixel = screen[i];
                background += usize::from(x as f32 > SLOT[0] && y as f32 > SLOT[1]);
            }
            None => {}
        }
    }
    assert!(
        background > 10_000,
        "the slot shows the screen around the person"
    );
    assert_ne!(plain, screen);
    shot_render::golden::assert_match("cut out over the screen", &cut, &expected);
}

#[test]
#[ignore = "needs the GPU and the generated 30 s fixtures"]
fn a_frame_without_a_person_shows_the_plain_camera() {
    let (dir, project, plan) = video("face-content");
    let plain = paint(dir.path(), &project, &plan, false);
    stand_in(0)
        .write(dir.path(), &Id::new("t01").unwrap())
        .unwrap();
    let fallback = paint(dir.path(), &project, &plan, true);
    shot_render::golden::assert_match("no person", &plain, &fallback);
}
