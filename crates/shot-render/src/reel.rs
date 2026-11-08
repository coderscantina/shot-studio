//! The templates reel: every overlay template on one 24 s stretch, per preset, for the golden
//! frames, the evidence frames and clips, the export timing and the preview budget. Tests only.

use std::collections::HashMap;
use std::sync::Arc;

use shot_core::{Brand, FormatKind, Micros, Overlay, ParamValue, TimeRange, Zoom};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use crate::code_diff::CodeDiff;
use crate::frame::Look;
use crate::motion::Preset;
use crate::overlay::overlay;
use crate::theme::Theme;

/// One pass of the reel.
pub(crate) const REEL: Micros = Micros(24_000_000);

/// The diff the code-diff and b-roll diff draw.
pub(crate) const DIFF: &str = "diff --git a/src/deploy.rs b/src/deploy.rs
--- a/src/deploy.rs
+++ b/src/deploy.rs
@@ -1,6 +1,7 @@
 pub fn deploy(day: Weekday) -> Result<(), Refusal> {
-    if day == Weekday::Fri {
-        return Err(Refusal::Friday);
-    }
+    let checks = Checks::run()?;
+    checks.require_green()?;
     ship()
 }
";
pub(crate) const DIFF_FILE: &str = "sources/reel.diff";
pub(crate) const STILL_FILE: &str = "sources/reel.png";

/// Where each template shows in the reel, from its start, and what it is called in evidence.
pub(crate) const MOMENTS: [(&str, u64); 11] = [
    ("chapter-band", 1_200_000),
    ("lower-third", 2_400_000),
    ("link-card", 4_400_000),
    ("highlight", 6_200_000),
    ("callout", 7_000_000),
    ("pop-emoji", 7_400_000),
    ("pop-word", 9_600_000),
    ("code-diff", 13_400_000),
    ("b-roll", 16_900_000),
    ("end-card", 20_500_000),
    ("blur", 3_600_000),
];

fn range(from: u64, to: u64) -> TimeRange {
    TimeRange::new(Micros(from), Micros(to)).expect("from before to")
}

/// Every template, on take time from `start`.
pub(crate) fn overlays(start: u64) -> Vec<Overlay> {
    let at = |from: u64, to: u64| range(start + from, start + to);
    let text = |s: &str| ParamValue::Text(s.to_owned());
    let int = ParamValue::Integer;
    vec![
        overlay(
            "chapter-band",
            at(200_000, 2_800_000),
            vec![
                ("text", text("Why we ship on Fridays")),
                ("label", text("Chapter 2")),
            ],
        ),
        overlay(
            "lower-third",
            at(1_000_000, 5_000_000),
            vec![
                ("text", text("Mike Wallner")),
                ("subtitle", text("Coder's Cantina")),
            ],
        ),
        overlay(
            "blur",
            at(3_000_000, 5_000_000),
            vec![
                ("x", int(2400)),
                ("y", int(1500)),
                ("width", int(800)),
                ("height", int(200)),
            ],
        ),
        overlay(
            "link-card",
            at(3_200_000, 8_600_000),
            vec![
                (
                    "url",
                    text("https://github.com/coderscantina/shot-studio/pull/1"),
                ),
                ("accent", ParamValue::Bool(true)),
            ],
        ),
        overlay(
            "highlight",
            at(5_000_000, 8_000_000),
            vec![
                ("x", int(820)),
                ("y", int(412)),
                ("width", int(1340)),
                ("height", int(64)),
            ],
        ),
        overlay(
            "callout",
            at(5_600_000, 9_000_000),
            vec![
                ("x", int(2200)),
                ("y", int(600)),
                ("width", int(900)),
                ("height", int(120)),
                ("text", text("Deploy button")),
            ],
        ),
        overlay("pop", at(6_600_000, 8_200_000), vec![("text", text("🔥"))]),
        overlay(
            "callout",
            at(8_400_000, 10_400_000),
            vec![
                ("x", int(3000)),
                ("y", int(1700)),
                ("text", text("Click here")),
            ],
        ),
        overlay(
            "pop",
            at(9_000_000, 10_400_000),
            vec![
                ("text", text("Ship it")),
                ("x", int(1920)),
                ("y", int(1080)),
                ("size", int(300)),
            ],
        ),
        overlay(
            "code-diff",
            at(10_600_000, 15_800_000),
            vec![
                ("diff", text(DIFF_FILE)),
                ("highlight-line", int(9)),
                ("highlight-at", int((start + 13_000_000) as i64)),
            ],
        ),
        overlay(
            "b-roll",
            at(16_000_000, 18_000_000),
            vec![("file", text(STILL_FILE))],
        ),
        overlay(
            "end-card",
            at(18_200_000, 24_000_000),
            vec![("subtitle", text("New videos every Friday"))],
        ),
    ]
}

/// The reel's zoom: springs in on the highlight and callout and out again.
pub(crate) fn zooms(start: u64) -> Vec<Zoom> {
    vec![Zoom {
        scale: 150,
        range: range(start + 4_600_000, start + 9_400_000),
        center: Some(shot_core::Point { x: 1800, y: 700 }),
        by: None,
    }]
}

/// The format whose default preset is `preset`.
pub(crate) fn format_of(preset: Preset) -> FormatKind {
    match preset {
        Preset::Reactive => FormatKind::Reaction,
        Preset::Editorial => FormatKind::Opinion,
        Preset::Launch => FormatKind::ProductTour,
        Preset::Calm => FormatKind::Tutorial,
    }
}

pub(crate) fn theme(preset: Preset) -> Theme {
    Theme::new(&Brand::default(), format_of(preset), None, &|_| {
        unreachable!("no brand files")
    })
    .expect("the default brand reads no files")
}

/// A still for the b-roll: a gradient with stripes, so the push in shows.
pub(crate) fn still() -> ImageData {
    let (width, height) = (1600u32, 900u32);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let stripe = ((x + y) / 60) % 2 == 0;
            let r = (40 + x * 120 / width) as u8;
            let g = (60 + y * 100 / height) as u8;
            let b = if stripe { 160 } else { 110 };
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    ImageData {
        data: Blob::new(Arc::new(pixels)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    }
}

/// The look a reel frame draws with: the preset's theme and the files the reel names.
pub(crate) fn look(preset: Preset, effects: shot_core::Effects) -> Look {
    let diff = Arc::new(CodeDiff::parse(DIFF));
    Look {
        effects,
        diffs: HashMap::from([(DIFF_FILE.to_owned(), diff)]),
        images: HashMap::from([(STILL_FILE.to_owned(), still())]),
        theme: Arc::new(theme(preset)),
        take: Some(shot_core::Id::new("t01").expect("an id")),
        ..Look::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::{Font, Rgba, Shape};
    use crate::export::{ExportOptions, ExportReport, export};
    use crate::frame::{Frame, Shot, compose};
    use crate::graph::Renderer;
    use crate::layout::{Canvas, Rect, Scene, Sources};
    use crate::surface::{BufferPool, Matrix, Range, read_rgb};
    use shot_core::{Effects, Layout, ZoomMotion};

    const PREVIEW: Canvas = Canvas {
        width: 1280,
        height: 720,
    };

    fn smooth() -> Effects {
        Effects {
            zoom_motion: ZoomMotion::Smooth,
            ..Effects::default()
        }
    }

    /// A busy screen to zoom into, point at and blur: stripes, blocks and text.
    fn screen(
        renderer: &Renderer,
    ) -> objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer> {
        let pool = BufferPool::new(3840, 2160, Range::Video, Matrix::Bt709).unwrap();
        let buffer = pool.buffer().unwrap();
        let mut drawing = crate::drawing::Drawing::default();
        for i in 0..24 {
            let x = i as f32 * 160.0;
            drawing.under.push(Shape::Rect {
                rect: Rect::new(x, 0.0, x + 80.0, 2160.0),
                radius: 0.0,
                color: Rgba::hex([0x2F6FEB, 0xEB6F2F, 0x2FEB6F][i % 3], 1.0),
            });
            drawing.under.push(Shape::Text {
                origin: [x + 10.0, 300.0 + (i % 7) as f32 * 220.0],
                text: format!("fn line_{i}()"),
                size: 64.0,
                font: Font::Mono,
                color: Rgba::hex(0xFFFFFF, 1.0),
            });
        }
        let frame = Frame {
            scene: Scene {
                background: [0.1, 0.1, 0.12],
                layers: Vec::new(),
            },
            drawing,
            cutaway: None,
        };
        renderer
            .draw(
                &frame,
                Sources {
                    screen: None,
                    camera: None,
                },
                &buffer,
            )
            .unwrap();
        buffer
    }

    fn frame_at(look: &Look, overlays: &[Overlay], t: Micros, canvas: Canvas) -> Frame {
        let shot = Shot {
            layout: Layout::Content,
            zooms: &zooms(0),
            overlays,
            time: t,
        };
        let sizes = Sources {
            screen: (3840, 2160),
            camera: (1920, 1080),
        };
        compose(&shot, look, canvas, sizes).unwrap()
    }

    fn render(
        renderer: &Renderer,
        source: &objc2_core_video::CVPixelBuffer,
        frame: &Frame,
        canvas: Canvas,
    ) -> Vec<[u8; 3]> {
        let pool =
            BufferPool::new(canvas.width, canvas.height, Range::Full, Matrix::Bt601).unwrap();
        let target = pool.buffer().unwrap();
        renderer
            .draw(
                frame,
                Sources {
                    screen: Some(source),
                    camera: None,
                },
                &target,
            )
            .unwrap();
        read_rgb(&target).unwrap()
    }

    #[test]
    #[ignore = "needs the GPU"]
    fn every_template_frame_is_the_same_reached_by_playing_or_by_seeking() {
        let renderer = Renderer::new().unwrap();
        let source = screen(&renderer);
        let fps = 60;
        let frames = REEL.0 * fps / 1_000_000;
        let overlays = overlays(0);
        for preset in Preset::ALL {
            let look = look(preset, smooth());
            let time = |frame: u64| Micros(frame * 1_000_000 / fps);
            let at = |on: &Renderer, f| {
                render(
                    on,
                    &source,
                    &frame_at(&look, &overlays, time(f), PREVIEW),
                    PREVIEW,
                )
            };
            // Seeks in a fixed pseudo-random order, on the same renderer and on a fresh one.
            let mut state: u64 = 0x2545_F491_4F6C_DD1D;
            let seeks: Vec<u64> = (0..60)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state % frames
                })
                .collect();
            let mut played = crate::golden::Played::new(frames, seeks.iter().copied());
            for f in 0..frames {
                played.push(&at(&renderer, f));
            }
            let fresh = Renderer::new().unwrap();
            for (seek, &f) in seeks.iter().enumerate() {
                let on = if seek % 2 == 0 { &renderer } else { &fresh };
                played.assert_same(&format!("{preset:?}"), f, &at(on, f));
            }
        }
    }

    /// Where the reel's clips and frames go, git-ignored.
    fn evidence() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/m11");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    #[ignore = "needs the GPU and the generated screen fixture; writes docs/evidence/m11"]
    fn frames_per_template_per_preset() {
        let fixtures = std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").unwrap());
        let decoder =
            shot_media::videotoolbox::Decoder::open_native(&fixtures.join("screen-30s.mp4"))
                .unwrap();
        let mut cursor = crate::cursor::FrameCursor::new(decoder);
        let renderer = Renderer::new().unwrap();
        let canvas = Canvas {
            width: 1920,
            height: 1080,
        };
        let overlays = overlays(0);
        for preset in Preset::ALL {
            let look = look(preset, smooth());
            for (name, t) in MOMENTS {
                let source = cursor
                    .goto(std::time::Duration::from_micros(t % 30_000_000))
                    .unwrap()
                    .unwrap()
                    .buffer
                    .clone();
                let frame = frame_at(&look, &overlays, Micros(t), canvas);
                let pixels = render(&renderer, &source, &frame, canvas);
                let image = image::RgbImage::from_raw(canvas.width, canvas.height, pixels.concat())
                    .unwrap();
                image
                    .save(evidence().join(format!("templates-{}-{name}.png", preset.name())))
                    .unwrap();
            }
        }
    }

    /// A video folder of `format` over the generated target project's media, with the reel's
    /// files in `sources/` with the screen recording in the content layout.
    fn project(dir: &std::path::Path, format: FormatKind, cut: &str) -> shot_core::Project {
        let fixtures =
            std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
                .join("target-project");
        std::os::unix::fs::symlink(fixtures.join("media"), dir.join("media")).unwrap();
        std::fs::create_dir_all(dir.join("sources")).unwrap();
        std::fs::write(dir.join(DIFF_FILE), DIFF).unwrap();
        let still = still();
        image::RgbaImage::from_raw(still.width, still.height, still.data.data().to_vec())
            .unwrap()
            .save(dir.join(STILL_FILE))
            .unwrap();
        shot_core::Project {
            video: shot_core::Video::parse(&format!(
                "title \"t\"\nstatus editing\nformat {}\n",
                format.as_str()
            ))
            .unwrap(),
            script: shot_core::Script::parse("# t\n").unwrap(),
            format: shot_core::Format::parse(&format!(
                "{}zoom-motion smooth\n",
                std::fs::read_to_string(fixtures.join("format.kdl"))
                    .unwrap()
                    .replace("layouts face split", "layouts content face split")
            ))
            .unwrap(),
            cut: shot_core::Cut::parse(cut).unwrap(),
        }
    }

    /// The reel's overlays and zoom every `period` of the take's kept ranges in `cut`.
    fn with_reels(project: &mut shot_core::Project, period: u64) {
        for take in project.cut.chapters.iter_mut().flat_map(|c| &mut c.takes) {
            let kept: Vec<TimeRange> = take
                .ranges
                .iter()
                .filter_map(|r| match r {
                    shot_core::RangeEdit::Keep(range) => Some(*range),
                    shot_core::RangeEdit::Remove { .. } => None,
                })
                .collect();
            let (Some(first), Some(last)) = (kept.first(), kept.last()) else {
                continue;
            };
            let mut start = first.from().0.div_ceil(period) * period;
            while start + REEL.0 <= last.to().0 {
                take.overlays.extend(overlays(start));
                take.zooms.extend(zooms(start));
                start += period;
            }
        }
    }

    fn timed_export(
        dir: &std::path::Path,
        project: &shot_core::Project,
        out: &std::path::Path,
    ) -> ExportReport {
        export(dir, project, "main", out, ExportOptions::default(), &|_| {}).unwrap()
    }

    #[test]
    #[ignore = "needs the generated target project and the hardware HEVC encoder; writes docs/evidence/m11"]
    fn template_clips_per_preset_through_the_export() {
        let cut = format!(
            "chapter \"Reel\" {{\n    take t01 {{\n        keep from=12_000_000 to={}\n    }}\n}}\n",
            12_000_000 + REEL.0
        );
        for preset in Preset::ALL {
            let dir = tempfile::tempdir().unwrap();
            let mut project = project(dir.path(), format_of(preset), &cut);
            with_reels(&mut project, 12_000_000);
            let out = evidence().join(format!("templates-{}.mp4", preset.name()));
            let report = timed_export(dir.path(), &project, &out);
            println!(
                "{}: {} frames in {:?}",
                preset.name(),
                report.frames,
                report.render_time
            );
        }
    }

    #[test]
    #[ignore = "needs the generated target project and the hardware HEVC encoder, takes minutes"]
    fn the_full_fixture_with_templates_exports_faster_than_realtime() {
        let fixtures = std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").unwrap());
        let cut = std::fs::read_to_string(fixtures.join("target-project/cut.kdl")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        // Launch is the costliest preset: glass, tilt and glyph by glyph type.
        let mut project = project(dir.path(), FormatKind::ProductTour, &cut);
        with_reels(&mut project, 60_000_000);
        let out = dir.path().join("main.mp4");
        let report = timed_export(dir.path(), &project, &out);
        let total = report.render_time + report.join_time;
        let speed = report.duration().as_secs_f64() / total.as_secs_f64();
        println!(
            "{} frames, render {:?}, join {:?}, {:.1} fps, {speed:.2}x realtime",
            report.frames,
            report.render_time,
            report.join_time,
            report.frames as f64 / total.as_secs_f64(),
        );
        assert!(speed > 1.0, "{speed:.2}x realtime");
    }

    #[test]
    #[ignore = "needs the generated screen fixture and the GPU; run with --release"]
    fn preview_frames_with_templates_fit_the_frame_budget() {
        let fixtures = std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").unwrap());
        let decoder =
            shot_media::videotoolbox::Decoder::open_native(&fixtures.join("screen-30s.mp4"))
                .unwrap();
        let mut cursor = crate::cursor::FrameCursor::new(decoder);
        let renderer = Renderer::new().unwrap();
        let preview = Canvas {
            width: 1920,
            height: 1080,
        };
        let pool =
            BufferPool::new(preview.width, preview.height, Range::Full, Matrix::Bt601).unwrap();
        let overlays = overlays(0);
        for preset in Preset::ALL {
            let look = look(preset, smooth());
            let mut times = Vec::new();
            for frame in 0..(REEL.0 * 60 / 1_000_000) {
                let t = Micros(frame * 1_000_000 / 60);
                let source = cursor
                    .goto(std::time::Duration::from_micros(t.0 % 30_000_000))
                    .unwrap()
                    .unwrap()
                    .buffer
                    .clone();
                let started = std::time::Instant::now();
                let drawn = frame_at(&look, &overlays, t, preview);
                let target = pool.buffer().unwrap();
                renderer
                    .draw(
                        &drawn,
                        Sources {
                            screen: Some(&source),
                            camera: None,
                        },
                        &target,
                    )
                    .unwrap();
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
            println!(
                "{}: compose and draw p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms over {} frames",
                preset.name(),
                at(0.5),
                at(0.99),
                at(1.0),
                times.len()
            );
            assert!(at(0.99) < 16.7);
        }
    }
}
