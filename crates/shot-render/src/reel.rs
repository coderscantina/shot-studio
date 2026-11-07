//! The motion reel: every motion system feature on one repeating stretch, per preset, drawn over
//! a frame, for the golden frames, the demo clips and the export timing. Tests only; the
//! templates take these moves over in milestone 11's later work.

use std::sync::{Arc, OnceLock};

use serde_json::json;
use shot_core::{Micros, TimeRange};

use crate::drawing::{Font, Glass, Placement, Rgba, Shape, Sheet, SoftShadow};
use crate::frame::Frame;
use crate::kinetic::{self, Kinetic};
use crate::layout::{Canvas, Rect};
use crate::lottie::{Filled, LottieTemplate, SlotDecl, SlotKind, SlotValue};
use crate::materials::{self, LIGHT};
use crate::motion::{Part, Preset, Reveal};

/// One pass of the reel; it repeats after this.
pub(crate) const CYCLE: Micros = Micros(6_000_000);

fn range(from: u64, to: u64) -> TimeRange {
    TimeRange::new(Micros(from), Micros(to)).expect("from before to")
}

/// Draws the reel of `preset` at media time `t`, repeating every `period`, its moves in the
/// first [`CYCLE`] of each.
pub(crate) fn decorate(
    preset: Preset,
    period: Micros,
    t: Micros,
    canvas: Canvas,
    frame: &mut Frame,
) {
    let local = Micros(t.0 % period.0);
    if local >= CYCLE {
        return;
    }
    let s = canvas.width as f32 / 3840.0;
    let motion = preset.motion();
    let moves = preset.moves();
    let drawing = &mut frame.drawing;

    // A card on a sheet: slides in from the left, tilting on the way, on glass where the preset
    // uses it, with a soft shadow that follows.
    let card = range(400_000, 5_400_000);
    let size = [1500.0 * s, 380.0 * s];
    let rest = [120.0 * s + size[0] / 2.0, canvas.height as f32 * 0.62];
    let place = |t: Micros| {
        let pose = motion.pose(card, Part::WHOLE, t);
        let away = pose.offset();
        let mut placement = Placement {
            center: [
                rest[0] - away * (size[0] + 200.0 * s) * motion.travel.min(1.0),
                rest[1],
            ],
            scale: if moves.reveal == Reveal::Pop {
                0.7 + 0.3 * pose.enter.max(0.0) * (1.0 - pose.exit * 0.3)
            } else {
                1.0
            },
            tilt: [moves.tilt * 0.4 * away, -moves.tilt * away],
            opacity: pose.presence(),
        };
        if moves.glass {
            let camera = materials::drift(t, Micros(2_900_000), 18.0 * s, [1.0, -0.3]);
            placement = materials::parallax(placement, 0.8, camera);
        }
        placement
    };
    let [open, close] = materials::shutter(local).map(place);
    if open.opacity > 0.0 || close.opacity > 0.0 {
        let panel = if moves.glass {
            Vec::new()
        } else {
            vec![Shape::Rect {
                rect: Rect::new(0.0, 0.0, size[0], size[1]),
                radius: 36.0 * s,
                color: Rgba::hex(0x15171C, 0.96),
            }]
        };
        let title = Shape::Text {
            origin: [96.0 * s, 150.0 * s],
            text: "Ship it on Friday".into(),
            size: 120.0 * s,
            font: Font::Sans,
            color: Rgba::hex(0xFFFFFF, 1.0),
        };
        let label = Shape::Text {
            origin: [96.0 * s, 60.0 * s],
            text: format!("{} preset", preset.name()),
            size: 56.0 * s,
            font: Font::Sans,
            color: Rgba::hex(0xC8F25A, 1.0),
        };
        let stripe = Shape::Rect {
            rect: Rect::new(0.0, 0.0, 18.0 * s, size[1]),
            radius: 9.0 * s,
            color: Rgba::hex(0xC8F25A, 1.0),
        };
        drawing.sheets.push(Sheet {
            size,
            radius: 36.0 * s,
            glass: moves.glass.then_some(Glass {
                blur: 36.0 * s,
                tint: Rgba::hex(0x101216, 0.45),
            }),
            shapes: panel.into_iter().chain([stripe, label, title]).collect(),
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

    // A headline in the preset's kinetic type.
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

    // A badge flying in from the right, motion blurred on the way.
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

    // A word pop caption, one word after another.
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

    // A Lottie sticker with its label drawn by parley.
    let sticker = range(1_200_000, 4_200_000);
    if (sticker.from()..sticker.to()).contains(&local) {
        drawing.over.push(Shape::Lottie {
            lottie: sticker_lottie(),
            origin: [canvas.width as f32 - 760.0 * s, 260.0 * s],
            scale: 1.4 * s,
            elapsed: Micros(local.0 - sticker.from().0),
            opacity: 1.0,
        });
    }
}

/// A star that pops in and spins, its color and label filled through slots. The label is
/// parented to the star, so it pops and turns with it.
fn sticker_lottie() -> Arc<Filled> {
    static STICKER: OnceLock<Arc<Filled>> = OnceLock::new();
    STICKER
        .get_or_init(|| {
            let prop = |k| json!({ "a": 0, "k": k });
            let file = json!({
                "v": "5.7.0", "fr": 60, "ip": 0, "op": 180, "w": 400, "h": 400, "layers": [
                    {
                        "ty": 5, "nm": "Label", "ind": 1, "parent": 2, "ip": 0, "op": 180, "st": 0,
                        "ks": { "a": prop(json!([0, 0])), "s": prop(json!([100, 100])),
                                "o": prop(json!(100)), "r": prop(json!(0)),
                                "p": prop(json!([0, 16])) },
                        "t": { "d": { "sid": "label", "k": [{ "t": 0, "s": {
                            "s": 44, "t": "NEW", "j": 2, "fc": [0.04, 0.04, 0.05], "f": "Sans"
                        } }] } }
                    },
                    {
                        "ty": 4, "nm": "Star", "ind": 2, "ip": 0, "op": 180, "st": 0,
                        "ks": {
                            "a": prop(json!([0, 0])), "o": prop(json!(100)),
                            "p": prop(json!([200, 200])),
                            "r": { "a": 1, "k": [
                                { "t": 0, "s": [-30], "i": { "x": [0.2], "y": [1] }, "o": { "x": [0.4], "y": [0] } },
                                { "t": 180, "s": [60] }
                            ] },
                            "s": { "a": 1, "k": [
                                { "t": 0, "s": [0, 0], "i": { "x": [0.3], "y": [1.6] }, "o": { "x": [0.4], "y": [0] } },
                                { "t": 24, "s": [100, 100], "i": { "x": [0.5], "y": [1] }, "o": { "x": [0.5], "y": [0] } },
                                { "t": 180, "s": [100, 100] }
                            ] }
                        },
                        "shapes": [{ "ty": "gr", "it": [
                            { "ty": "sr", "sy": 1, "d": 1, "pt": prop(json!(8)), "p": prop(json!([0, 0])),
                              "r": prop(json!(0)), "ir": prop(json!(110)), "is": prop(json!(0)),
                              "or": prop(json!(170)), "os": prop(json!(0)) },
                            { "ty": "fl", "c": { "a": 0, "k": [1, 0.5, 0.3], "sid": "accent" },
                              "o": prop(json!(100)) },
                            { "ty": "tr", "a": prop(json!([0, 0])), "p": prop(json!([0, 0])),
                              "s": prop(json!([100, 100])), "r": prop(json!(0)), "o": prop(json!(100)) }
                        ] }]
                    }
                ]
            });
            let bytes = serde_json::to_vec(&file).expect("json");
            let decl = |name: &str, kind| SlotDecl {
                name: name.to_owned(),
                kind,
                required: true,
            };
            let template = LottieTemplate::load(
                &bytes,
                &[decl("label", SlotKind::Text), decl("accent", SlotKind::Color)],
            )
            .expect("the sticker loads");
            let filled = template
                .fill(&[
                    ("label", SlotValue::Text("NEW".into())),
                    ("accent", SlotValue::Color(Rgba::hex(0xC8F25A, 1.0))),
                ])
                .expect("the sticker fills");
            Arc::new(filled)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Decoration, ExportOptions, ExportReport, export_decorated};
    use crate::frame::{Look, Shot, compose};
    use crate::graph::Renderer;
    use crate::layout::{Scene, Sources};
    use crate::surface::{BufferPool, Matrix, Range, read_rgb};
    use shot_core::{Effects, Layout, Point, Zoom, ZoomMotion};

    const PREVIEW: Canvas = Canvas {
        width: 1280,
        height: 720,
    };

    /// A busy screen to zoom into and blur: stripes, blocks and text.
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
        let scene = Scene {
            background: [0.1, 0.1, 0.12],
            layers: Vec::new(),
        };
        let frame = Frame {
            scene,
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

    fn frame_at(preset: Preset, t: Micros) -> Frame {
        let zooms = [Zoom {
            scale: 180,
            range: range(1_500_000, 4_500_000),
            center: Some(Point { x: 2600, y: 900 }),
            by: None,
        }];
        let look = Look {
            effects: Effects {
                zoom_motion: ZoomMotion::Smooth,
                ..Effects::default()
            },
            ..Look::default()
        };
        let shot = Shot {
            layout: Layout::Content,
            zooms: &zooms,
            overlays: &[],
            time: t,
        };
        let sizes = Sources {
            screen: (3840, 2160),
            camera: (1920, 1080),
        };
        let mut frame = compose(&shot, &look, PREVIEW, sizes).unwrap();
        decorate(preset, CYCLE, t, PREVIEW, &mut frame);
        frame
    }

    fn render(
        renderer: &Renderer,
        source: &objc2_core_video::CVPixelBuffer,
        frame: &Frame,
    ) -> blake3::Hash {
        let pool =
            BufferPool::new(PREVIEW.width, PREVIEW.height, Range::Full, Matrix::Bt601).unwrap();
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
        let pixels: Vec<u8> = read_rgb(&target).unwrap().into_iter().flatten().collect();
        blake3::hash(&pixels)
    }

    #[test]
    #[ignore = "needs the GPU"]
    fn every_frame_is_the_same_reached_by_playing_or_by_seeking() {
        let renderer = Renderer::new().unwrap();
        let source = screen(&renderer);
        let fps = 60;
        let frames = CYCLE.0 * fps / 1_000_000;
        for preset in Preset::ALL {
            let time = |frame: u64| Micros(frame * 1_000_000 / fps);
            let played: Vec<blake3::Hash> = (0..frames)
                .map(|f| render(&renderer, &source, &frame_at(preset, time(f))))
                .collect();
            // Seeks in a fixed pseudo-random order, on the same renderer and on a fresh one.
            let fresh = Renderer::new().unwrap();
            let mut state: u64 = 0x2545_F491_4F6C_DD1D;
            for seek in 0..40 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let f = state % frames;
                let on = if seek % 2 == 0 { &renderer } else { &fresh };
                let seen = render(on, &source, &frame_at(preset, time(f)));
                assert_eq!(seen, played[f as usize], "{preset:?} frame {f}");
            }
        }
    }

    /// Where the reel's clips and frames go, git-ignored.
    fn evidence() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/m11");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A video folder over the generated target project's media, with smooth zooms.
    fn project(dir: &std::path::Path, cut: &str) -> shot_core::Project {
        let fixtures =
            std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
                .join("target-project");
        std::os::unix::fs::symlink(fixtures.join("media"), dir.join("media")).unwrap();
        let format = std::fs::read_to_string(fixtures.join("format.kdl")).unwrap();
        shot_core::Project {
            video: shot_core::Video::parse("title \"t\"\nstatus editing\nformat opinion\n")
                .unwrap(),
            script: shot_core::Script::parse("# t\n").unwrap(),
            format: shot_core::Format::parse(&format!("{format}zoom-motion smooth\n")).unwrap(),
            cut: shot_core::Cut::parse(cut).unwrap(),
        }
    }

    fn export(
        dir: &std::path::Path,
        project: &shot_core::Project,
        out: &std::path::Path,
        decoration: Decoration,
    ) -> ExportReport {
        export_decorated(
            dir,
            project,
            "main",
            out,
            ExportOptions::default(),
            Some(decoration),
            &|_| {},
        )
        .unwrap()
    }

    fn reactive(t: Micros, canvas: Canvas, frame: &mut Frame) {
        decorate(Preset::Reactive, CYCLE, t, canvas, frame);
    }
    fn editorial(t: Micros, canvas: Canvas, frame: &mut Frame) {
        decorate(Preset::Editorial, CYCLE, t, canvas, frame);
    }
    fn launch(t: Micros, canvas: Canvas, frame: &mut Frame) {
        decorate(Preset::Launch, CYCLE, t, canvas, frame);
    }
    fn calm(t: Micros, canvas: Canvas, frame: &mut Frame) {
        decorate(Preset::Calm, CYCLE, t, canvas, frame);
    }
    /// Launch moves, the costliest, for 6 s of every 20 s.
    fn typical(t: Micros, canvas: Canvas, frame: &mut Frame) {
        decorate(Preset::Launch, Micros(20_000_000), t, canvas, frame);
    }

    #[test]
    #[ignore = "needs the generated target project and the hardware HEVC encoder; writes docs/evidence/m11"]
    fn demo_clips_per_preset_through_the_export() {
        // 6 s of the take from a whole cycle in, with a zoom springing in and out.
        let cut = r#"chapter "Reel" {
    take t01 {
        keep from=12_000_000 to=18_000_000
        zoom 160 from=13_500_000 to=16_500_000 x=2600 y=900
    }
}
"#;
        let reels = [
            Decoration {
                name: "reactive",
                draw: reactive,
            },
            Decoration {
                name: "editorial",
                draw: editorial,
            },
            Decoration {
                name: "launch",
                draw: launch,
            },
            Decoration {
                name: "calm",
                draw: calm,
            },
        ];
        for reel in reels {
            let dir = tempfile::tempdir().unwrap();
            let project = project(dir.path(), cut);
            let out = evidence().join(format!("{}.mp4", reel.name));
            let report = export(dir.path(), &project, &out, reel);
            println!(
                "{}: {} frames in {:?}",
                reel.name, report.frames, report.render_time
            );
        }
    }

    #[test]
    #[ignore = "needs the generated target project and the hardware HEVC encoder, takes minutes"]
    fn the_full_fixture_with_glass_and_motion_blur_exports_faster_than_realtime() {
        let fixtures = std::path::PathBuf::from(std::env::var("SHOT_FIXTURES").unwrap());
        let cut = std::fs::read_to_string(fixtures.join("target-project/cut.kdl")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let project = project(dir.path(), &cut);
        let out = dir.path().join("main.mp4");
        let report = export(
            dir.path(),
            &project,
            &out,
            Decoration {
                name: "typical",
                draw: typical,
            },
        );
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
    fn preview_frames_with_glass_and_motion_blur_fit_the_frame_budget() {
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
        let zooms = [Zoom {
            scale: 180,
            range: range(1_500_000, 4_500_000),
            center: Some(Point { x: 2600, y: 900 }),
            by: None,
        }];
        let look = Look {
            effects: Effects {
                zoom_motion: ZoomMotion::Smooth,
                ..Effects::default()
            },
            ..Look::default()
        };
        for preset in Preset::ALL {
            let mut times = Vec::new();
            for frame in 0..(CYCLE.0 * 60 / 1_000_000) {
                let t = Micros(frame * 1_000_000 / 60);
                let source = cursor
                    .goto(std::time::Duration::from_micros(t.0))
                    .unwrap()
                    .unwrap()
                    .buffer
                    .clone();
                let started = std::time::Instant::now();
                let shot = Shot {
                    layout: Layout::Content,
                    zooms: &zooms,
                    overlays: &[],
                    time: t,
                };
                let sizes = Sources {
                    screen: crate::surface::size(&source),
                    camera: (1, 1),
                };
                let mut drawn = compose(&shot, &look, preview, sizes).unwrap();
                decorate(preset, CYCLE, t, preview, &mut drawn);
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
