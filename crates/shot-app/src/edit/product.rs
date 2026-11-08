//! The Product demo workspace: the preview with the product video effects, their settings from
//! `format.kdl` on the right, the code-diff overlay at the playhead, and lanes for the screen,
//! clicks, zooms and keys over the whole video.
//!
//! Every setting writes `format.kdl`. Auto zoom writes its zooms into `cut.kdl` and runs again
//! whenever its triggers or scale change, replacing only its own zooms.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Context, Div, FontWeight, Hsla, Window, div, prelude::*, px};
use shot_core::{
    AutoZoom, Color, CursorStyle, DeviceFrame, Format, Id, Keystrokes, Micros, Shadow, WindowStyle,
    ZoomBy, ZoomMotion, ZoomTrigger,
};
use shot_render::code_diff::{CodeDiff, CodeDiffOverlay, LineKind};
use shot_render::events::EventLog;
use shot_render::frame::key_label;
use shot_render::plan::snap;

use super::EditScreen;
use super::parts;
use crate::model::{FPS, fraction};
use crate::theme::*;
use crate::ui::{self, Segment, mono, segmented};

const NAME_WIDTH: f32 = 92.;
/// Typed keys closer than this share one block in the Keys lane.
const KEY_RUN_GAP: u64 = 800_000;

/// What the workspace reads besides the project files: event logs and diff snapshots.
#[derive(Default)]
pub struct ProductState {
    events: HashMap<Id, Option<Arc<EventLog>>>,
    diffs: HashMap<String, Option<Arc<CodeDiff>>>,
}

pub fn render(this: &mut EditScreen, _: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .px(px(8.))
        .pb(px(8.))
        .children(parts::error_strip(this))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(8.))
                .child(stage(this, cx))
                .child(settings(this, cx)),
        )
        .child(
            div()
                .h(px(226.))
                .flex_none()
                .flex()
                .gap(px(8.))
                .child(code_pane(this, cx))
                .child(lanes(this, cx)),
        )
}

fn stage(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    ui::pane()
        .flex_1()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .rounded(px(11.))
                .bg(WELL)
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(div().w(gpui::relative(0.92)).child(parts::preview(this))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(div().flex_1().child(parts::transport(this, true, cx)))
                .child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(MUTED)
                        .child("Cursor recorded as data. Change zoom, size or path any time."),
                ),
        )
}

// Settings.

fn format_of(this: &EditScreen, cx: &Context<EditScreen>) -> Format {
    this.folder.read(cx).project.format.clone()
}

impl EditScreen {
    /// Writes `format.kdl` with `change` applied, then runs auto zoom again when it changed.
    fn change_format(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Format)) {
        let result = self.folder.update(cx, |folder, cx| {
            let mut format = folder.project.format.clone();
            change(&mut format);
            // A migrated video's zooms come from the style rules, not the auto-zoom settings.
            let rerun =
                format.auto_zoom != folder.project.format.auto_zoom && !folder.is_migrated();
            folder.write_format(format)?;
            if rerun {
                let cut = shot_render::rules::auto_zoom(
                    &folder.dir,
                    &folder.project.cut,
                    &folder.project.format,
                )
                .map_err(|e| e.to_string())?;
                folder.write_cut(cut)?;
            }
            cx.notify();
            Ok::<_, String>(())
        });
        self.error = result.err();
        cx.notify();
    }
}

/// The next value after `current` in `values`, wrapping around.
fn next<T: PartialEq + Copy>(values: &[T], current: T) -> T {
    let index = values
        .iter()
        .position(|v| *v == current)
        .map_or(0, |i| i + 1);
    values[index % values.len()]
}

fn switch(id: &'static str, on: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .w(px(36.))
        .h(px(21.))
        .flex_none()
        .rounded(px(11.))
        .p(px(2.))
        .flex()
        .cursor_pointer()
        .bg(if on { ACCENT } else { FLOAT })
        .when(on, |this| this.justify_end())
        .child(
            div()
                .size(px(17.))
                .rounded(px(9.))
                .bg(if on { ON_ACCENT } else { MUTED }),
        )
}

/// A setting row whose value steps to the next option on click.
fn field(
    id: &'static str,
    name: &'static str,
    value: impl Into<gpui::SharedString>,
    mono_value: bool,
    on_click: impl Fn(&mut EditScreen, &mut Context<EditScreen>) + 'static,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let value = value.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .py(px(8.))
        .cursor_pointer()
        .child(div().text_color(MUTED).child(name))
        .child(
            div()
                .px(px(6.))
                .rounded(px(6.))
                .hover(|s| s.bg(FLOAT))
                .when(mono_value, |this| this.font_family(MONO))
                .child(value),
        )
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
}

fn group() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(14.))
        .py(px(6.))
        .rounded(px(12.))
        .bg(CARD)
}

fn heading(text: &'static str) -> Div {
    div()
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(17.))
        .child(text)
}

/// Triggers in canonical order.
const TRIGGERS: [ZoomTrigger; 3] = [ZoomTrigger::Click, ZoomTrigger::Typing, ZoomTrigger::Cursor];
const SCALES: [u16; 4] = [140, 160, 200, 250];
const CURSOR_SIZES: [u16; 5] = [0, 100, 140, 180, 220];
const PADDINGS: [u16; 5] = [0, 32, 48, 64, 96];
const CORNERS: [u16; 4] = [0, 12, 24, 36];
/// The background swatches: navy, graphite, brand blue, light.
const BACKGROUNDS: [u32; 4] = [0x1C2340, 0x2A2D33, 0x4F6BFF, 0xE9EBEF];

fn settings(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let format = format_of(this, cx);
    let effects = format.effects();
    let rule = format.auto_zoom.clone();
    let on = rule.as_ref().is_some_and(|r| !r.triggers.is_empty());
    let triggers = rule
        .as_ref()
        .map(|r| r.triggers.clone())
        .unwrap_or_default();
    let scale = rule.as_ref().map_or(160, |r| r.scale);
    let trigger = |label: &'static str, trigger: ZoomTrigger, cx: &mut Context<EditScreen>| {
        Segment::new(
            label,
            triggers.contains(&trigger),
            cx.listener(move |this, _, _, cx| {
                this.change_format(cx, |f| {
                    let rule = f.auto_zoom.get_or_insert_with(default_rule);
                    if let Some(i) = rule.triggers.iter().position(|t| *t == trigger) {
                        rule.triggers.remove(i);
                    } else {
                        rule.triggers.push(trigger);
                        rule.triggers
                            .sort_by_key(|t| TRIGGERS.iter().position(|o| o == t));
                    }
                })
            }),
        )
    };
    let style = effects.window;
    ui::pane()
        .w(px(330.))
        .flex_none()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .p(px(4.))
                .child(heading("Auto zoom"))
                .child(div().flex_1())
                .child(
                    switch("auto-zoom", on).on_click(cx.listener(move |this, _, _, cx| {
                        this.change_format(cx, |f| {
                            let rule = f.auto_zoom.get_or_insert_with(default_rule);
                            rule.triggers = if on {
                                Vec::new()
                            } else {
                                default_rule().triggers
                            };
                        })
                    })),
                ),
        )
        .child(segmented(
            "triggers",
            WELL,
            vec![
                trigger("Clicks", ZoomTrigger::Click, cx),
                trigger("Typing", ZoomTrigger::Typing, cx),
                trigger("Cursor", ZoomTrigger::Cursor, cx),
            ],
        ))
        .child(
            group()
                .child(field(
                    "scale",
                    "Zoom",
                    format!("{scale}%"),
                    true,
                    move |this, cx| {
                        this.change_format(cx, |f| {
                            let rule = f.auto_zoom.get_or_insert_with(default_rule);
                            rule.scale = next(&SCALES, rule.scale);
                        })
                    },
                    cx,
                ))
                .child(field(
                    "motion",
                    "Motion",
                    match effects.zoom_motion {
                        ZoomMotion::Instant => "Instant",
                        ZoomMotion::Smooth => "Smooth",
                    },
                    false,
                    move |this, cx| {
                        this.change_format(cx, |f| {
                            f.zoom_motion = Some(next(
                                &[ZoomMotion::Instant, ZoomMotion::Smooth],
                                effects.zoom_motion,
                            ))
                        })
                    },
                    cx,
                ))
                .child(field(
                    "cursor-size",
                    "Cursor size",
                    match effects.cursor.size {
                        0 => "Hidden".to_owned(),
                        size => format!("{size}%"),
                    },
                    true,
                    move |this, cx| {
                        this.change_format(cx, |f| {
                            f.cursor = Some(CursorStyle {
                                size: next(&CURSOR_SIZES, effects.cursor.size),
                                ..effects.cursor
                            })
                        })
                    },
                    cx,
                ))
                .child(field(
                    "smoothing",
                    "Smoothing",
                    match effects.cursor.smoothing.0 {
                        0 => "Off".to_owned(),
                        us => format!("{} ms", us / 1000),
                    },
                    true,
                    move |this, cx| {
                        this.change_format(cx, |f| {
                            f.cursor = Some(CursorStyle {
                                smoothing: next(
                                    &[Micros(0), Micros(80_000), Micros(120_000), Micros(250_000)],
                                    effects.cursor.smoothing,
                                ),
                                ..effects.cursor
                            })
                        })
                    },
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py(px(8.))
                        .child(div().text_color(MUTED).child("Click ripple"))
                        .child(
                            switch("ripple", effects.cursor.ripple).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.change_format(cx, |f| {
                                        f.cursor = Some(CursorStyle {
                                            ripple: !effects.cursor.ripple,
                                            ..effects.cursor
                                        })
                                    })
                                },
                            )),
                        ),
                )
                .child(field(
                    "keystrokes",
                    "Keystrokes",
                    match effects.keystrokes {
                        Keystrokes::Off => "Off",
                        Keystrokes::Shortcuts => "Shortcuts only",
                        Keystrokes::All => "All keys",
                    },
                    false,
                    move |this, cx| {
                        this.change_format(cx, |f| {
                            f.keystrokes = Some(next(
                                &[Keystrokes::Off, Keystrokes::Shortcuts, Keystrokes::All],
                                effects.keystrokes,
                            ))
                        })
                    },
                    cx,
                )),
        )
        .child(
            div()
                .px(px(4.))
                .pt(px(10.))
                .pb(px(2.))
                .child(heading("Window")),
        )
        .child(
            group()
                .child(window_field(
                    "padding",
                    "Padding",
                    format!("{} px", style.padding),
                    true,
                    style,
                    |s| WindowStyle {
                        padding: next(&PADDINGS, s.padding),
                        ..s
                    },
                    cx,
                ))
                .child(window_field(
                    "corners",
                    "Corners",
                    format!("{} px", style.corners),
                    true,
                    style,
                    |s| WindowStyle {
                        corners: next(&CORNERS, s.corners),
                        ..s
                    },
                    cx,
                ))
                .child(window_field(
                    "shadow",
                    "Shadow",
                    match style.shadow {
                        Shadow::None => "None",
                        Shadow::Soft => "Soft",
                        Shadow::Hard => "Hard",
                    },
                    false,
                    style,
                    |s| WindowStyle {
                        shadow: next(&[Shadow::None, Shadow::Soft, Shadow::Hard], s.shadow),
                        ..s
                    },
                    cx,
                ))
                .child(window_field(
                    "frame",
                    "Device frame",
                    match style.frame {
                        DeviceFrame::None => "None",
                        DeviceFrame::Macbook => "MacBook",
                        DeviceFrame::Iphone => "iPhone",
                    },
                    false,
                    style,
                    |s| WindowStyle {
                        frame: next(
                            &[DeviceFrame::None, DeviceFrame::Macbook, DeviceFrame::Iphone],
                            s.frame,
                        ),
                        ..s
                    },
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py(px(8.))
                        .child(div().text_color(MUTED).child("Background"))
                        .child(div().flex().gap(px(6.)).children(
                            BACKGROUNDS.iter().enumerate().map(|(index, rgb)| {
                                let [_, r, g, b] = rgb.to_be_bytes();
                                let color = Color([r, g, b]);
                                let picked = style.background == color;
                                div()
                                    .id(("background", index))
                                    .size(px(22.))
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(gpui::rgb(*rgb))
                                    .when(picked, |this| this.border_2().border_color(ACCENT))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.change_format(cx, |f| {
                                            f.window = Some(WindowStyle {
                                                background: color,
                                                ..style
                                            })
                                        })
                                    }))
                            }),
                        )),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn window_field(
    id: &'static str,
    name: &'static str,
    value: impl Into<gpui::SharedString>,
    mono_value: bool,
    style: WindowStyle,
    step: fn(WindowStyle) -> WindowStyle,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    field(
        id,
        name,
        value,
        mono_value,
        move |this, cx| this.change_format(cx, |f| f.window = Some(step(style))),
        cx,
    )
}

/// The rule auto zoom starts from: the product tour preset's.
fn default_rule() -> AutoZoom {
    shot_core::FormatKind::ProductTour
        .preset()
        .auto_zoom
        .unwrap_or(AutoZoom {
            triggers: vec![ZoomTrigger::Click, ZoomTrigger::Typing],
            scale: 160,
            hold: Micros(1_500_000),
        })
}

// Code from git.

fn code_pane(this: &mut EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let dir = this.folder.read(cx).dir.clone();
    // The code-diff overlay at the playhead, or else the first one in the cut.
    let at_playhead = this
        .map
        .span_at(this.playhead)
        .and_then(|s| s.overlays.iter().find_map(CodeDiffOverlay::of));
    let overlay = at_playhead.or_else(|| {
        this.map
            .spans
            .iter()
            .flat_map(|s| &s.overlays)
            .find_map(CodeDiffOverlay::of)
    });
    let diff = overlay.as_ref().and_then(|o| {
        this.product
            .diffs
            .entry(o.diff.clone())
            .or_insert_with(|| {
                std::fs::read_to_string(dir.join(&o.diff))
                    .ok()
                    .map(|text| Arc::new(CodeDiff::parse(&text)))
            })
            .clone()
    });
    let line = |kind: LineKind, text: String, highlighted: bool| {
        let (color, tint): (Hsla, Option<Hsla>) = match kind {
            LineKind::File | LineKind::Hunk => (FAINT.into(), None),
            LineKind::Context => (BODY.into(), None),
            LineKind::Added => (ACCENT.into(), Some(Hsla::from(ACCENT).opacity(0.07))),
            LineKind::Removed => (CUT_TEXT.into(), Some(Hsla::from(CUT).opacity(0.08))),
        };
        let tint = if highlighted {
            Some(Hsla::from(ACCENT).opacity(0.16))
        } else {
            tint
        };
        div()
            .font_family(MONO)
            .text_size(TEXT_XS)
            .line_height(px(21.))
            .px(px(12.))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_color(color)
            .when_some(tint, |this, tint| this.bg(tint))
            .child(text)
    };
    let body = match (&overlay, &diff) {
        (Some(overlay), Some(diff)) => div()
            .flex_1()
            .min_h_0()
            .rounded(px(10.))
            .bg(WELL)
            .overflow_hidden()
            .py(px(8.))
            .flex()
            .flex_col()
            .children(diff.lines.iter().take(7).map(|l| {
                line(
                    l.kind,
                    l.text.clone(),
                    overlay.highlight_line == Some(l.number),
                )
            })),
        (Some(overlay), None) => div()
            .flex_1()
            .text_color(RECORD_TEXT)
            .child(format!("{} is missing", overlay.diff)),
        (None, _) => div()
            .flex_1()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(8.))
            .text_size(TEXT_SM)
            .text_color(MUTED)
            .child("No code from git in this video yet. Snapshot a commit range with")
            .child(mono("shot code-diff --code <repo> --range <from>..<to>").text_color(BODY)),
    };
    let subtitle = overlay.as_ref().map(|o| {
        format!(
            "{} · {}",
            o.diff.trim_start_matches("sources/"),
            crate::model::seconds(o.range.duration())
        )
    });
    ui::pane()
        .w(px(440.))
        .flex_none()
        .px(px(14.))
        .py(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Code from git"),
                )
                .children(subtitle.map(|s| mono(s).text_size(TEXT_SM).text_color(MUTED))),
        )
        .child(body)
        .child(
            div()
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .child("Types out the real diff, then highlights the line you talk about."),
        )
}

// Lanes.

fn lanes(this: &mut EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let total = this.map.frames.max(1);
    let x = |frame: u64| fraction(frame, total);
    let mut clicks = Vec::new();
    let mut keys = Vec::new();
    let mut zooms = Vec::new();
    let dir = this.folder.read(cx).dir.clone();
    for span in &this.map.spans {
        let events = this
            .product
            .events
            .entry(span.take.clone())
            .or_insert_with(|| {
                shot_render::assets::Assets::new(&dir)
                    .events(&span.take)
                    .ok()
                    .flatten()
            })
            .clone();
        let out = |time: Micros| {
            let frame = snap_frame(time);
            span.source_frames
                .contains(&frame)
                .then(|| span.output_frame + frame - span.source_frames.start)
        };
        if let Some(events) = &events {
            clicks.extend(events.clicks().iter().filter_map(|c| out(c.t)));
            let all = events.keys();
            let mut start = 0;
            for end in 1..=all.len() {
                if end < all.len() && all[end].t.0 - all[end - 1].t.0 < KEY_RUN_GAP {
                    continue;
                }
                let last = &all[end - 1];
                if let (Some(from), Some(label)) =
                    (out(all[start].t), key_label(all, Keystrokes::All, last.t))
                {
                    let to = out(Micros(last.t.0 + 400_000)).unwrap_or(span.output_frames().end);
                    keys.push((from, to, label));
                }
                start = end;
            }
        }
        for zoom in &span.zooms {
            let snapped = snap(zoom.range, FPS);
            let from = snapped.start.max(span.source_frames.start);
            let to = snapped.end.min(span.source_frames.end);
            if from < to {
                let at = |f: u64| span.output_frame + (f - span.source_frames.start);
                zooms.push((at(from), at(to), zoom.scale, zoom.by));
            }
        }
    }
    let chapters: Vec<_> = this
        .map
        .chapters
        .iter()
        .filter(|c| !c.frames.is_empty())
        .map(|c| (c.frames.clone(), c.title.clone()))
        .collect();
    let playhead = x(this.playhead);
    ui::pane()
        .flex_1()
        .py(px(8.))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .h(px(24.))
                .child(name("Timeline"))
                .child(
                    div()
                        .flex_1()
                        .mr(px(14.))
                        .flex()
                        .justify_between()
                        .font_family(MONO)
                        .text_size(TEXT_XS)
                        .text_color(FAINT)
                        .children((0..=4).map(|i| crate::model::timecode(total * i / 4))),
                ),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .pt(px(4.))
                .child(lane(
                    "Screen",
                    div()
                        .size_full()
                        .children(chapters.into_iter().map(|(frames, title)| {
                            block(x(frames.start), x(frames.end), BLOCK, BODY, title)
                        })),
                ))
                .child(lane(
                    "Clicks",
                    div().size_full().children(clicks.into_iter().map(|frame| {
                        div()
                            .absolute()
                            .top(px(4.))
                            .left(gpui::relative(x(frame)))
                            .size(px(9.))
                            .rounded(px(5.))
                            .bg(ACCENT)
                    })),
                ))
                .child(lane(
                    "Zoom",
                    div()
                        .size_full()
                        .children(zooms.into_iter().map(|(from, to, scale, by)| {
                            let by = match by {
                                Some(ZoomBy::Click) => " · click",
                                Some(ZoomBy::Typing) => " · typing",
                                Some(ZoomBy::Cursor) => " · cursor",
                                Some(ZoomBy::Marker) => " · marker",
                                Some(ZoomBy::JumpCut) => " · jump cut",
                                Some(ZoomBy::Emphasis) => " · emphasis",
                                None => "",
                            };
                            block(
                                x(from),
                                x(to),
                                ZOOM_TINT,
                                ZOOM_TEXT,
                                format!("{scale}%{by}"),
                            )
                        })),
                ))
                .child(lane(
                    "Keys",
                    div()
                        .size_full()
                        .children(keys.into_iter().map(|(from, to, label)| {
                            block(x(from), x(to), FLOAT, KBD_TEXT, label)
                        })),
                ))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(NAME_WIDTH))
                        .right(px(14.))
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(gpui::relative(playhead))
                                .w(px(2.))
                                .rounded(px(1.))
                                .bg(ACCENT),
                        ),
                ),
        )
}

fn snap_frame(time: Micros) -> u64 {
    (time.0 * u64::from(FPS) + 500_000) / 1_000_000
}

fn name(text: &'static str) -> Div {
    div()
        .w(px(NAME_WIDTH))
        .flex_none()
        .pl(px(14.))
        .text_size(TEXT_SM)
        .text_color(MUTED)
        .child(text)
}

fn lane(title: &'static str, track: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(21.))
        .child(name(title))
        .child(
            div()
                .flex_1()
                .relative()
                .h(px(17.))
                .mr(px(14.))
                .rounded(px(5.))
                .bg(WELL)
                .overflow_hidden()
                .child(track),
        )
}

fn block(
    left: f32,
    right: f32,
    background: impl Into<Hsla>,
    color: impl Into<Hsla>,
    text: String,
) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(gpui::relative(left))
        .w(gpui::relative((right - left).max(0.002)))
        .rounded(px(5.))
        .px(px(6.))
        .bg(background.into())
        .text_color(color.into())
        .text_size(TEXT_XS)
        .line_height(px(17.))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(text)
}
