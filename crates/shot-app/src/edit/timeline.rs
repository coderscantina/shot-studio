//! The Timeline workspace: preview, change list and overlays on top, the whole-video overview,
//! then one lane per kind around the playhead: Layout, Zoom, B-roll, Highlight, Words and Voice.

use std::sync::Arc;

use gpui::{
    Bounds, Context, Div, FontWeight, Hsla, MouseButton, MouseDownEvent, Pixels, Window, canvas,
    div, fill, point, prelude::*, px, size,
};
use shot_core::{RemovedBy, WordState};
use shot_render::plan::frame_time;

use super::EditScreen;
use super::parts::{self, layout_name};
use crate::model::{self, FPS, changes, seconds, timecode};
use crate::peaks::Peaks;
use crate::theme::*;
use crate::ui::{self, icon, kbd, label, mono};

/// Seconds of output the lanes show, before and after the playhead.
const BEFORE: u64 = 5;
const AFTER: u64 = 10;
const NAME_WIDTH: f32 = 112.;
/// The preview pane's width, so a 16:9 preview and the transport fit the 420 px top row.
const STAGE_WIDTH: f32 = 653.;
/// Average width of a character at 13 px, for packing words into rows.
const WORD_CHAR: f32 = 7.2;

pub fn render(this: &mut EditScreen, win: &mut Window, cx: &mut Context<EditScreen>) -> Div {
    let window = lane_window(this);
    // The lane tracks span the window minus the pane margins, the names and the right margin.
    let track = f32::from(win.viewport_size().width) - 16. - NAME_WIDTH - 16.;
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(parts::error_strip(this))
        .child(
            div()
                .h(px(420.))
                .flex_none()
                .flex()
                .gap(px(8.))
                .mx(px(8.))
                .child(
                    ui::pane()
                        .w(px(STAGE_WIDTH))
                        .flex_none()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(parts::preview(this))
                        .child(parts::transport(this, true, cx)),
                )
                .child(change_list(this, cx))
                .child(super::overlays::pane(this, cx)),
        )
        .child(lanes(this, window, track, cx))
}

/// The output frames the lanes show.
fn lane_window(this: &EditScreen) -> std::ops::Range<u64> {
    let fps = u64::from(FPS);
    let span = (BEFORE + AFTER) * fps;
    let start = this
        .playhead
        .saturating_sub(BEFORE * fps)
        .min(this.map.frames.saturating_sub(span));
    start..(start + span).min(this.map.frames.max(1))
}

fn change_list(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let folder = this.folder.read(cx);
    let changes = changes(&folder.project.cut);
    let raw: u64 = model::blocks(&folder.project.cut)
        .filter_map(|(_, edit)| {
            Some(edit.ranges.last()?.range().to().0 - edit.ranges.first()?.range().from().0)
        })
        .sum();
    let line =
        |name: &'static str, color: gpui::Rgba, text: String, count: usize, delta: String| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(10.))
                .py(px(8.))
                .rounded(RADIUS_SM)
                .child(icon(name, ICON, color))
                .child(div().flex_1().child(text))
                .child(mono(count.to_string()).text_size(px(14.)).text_color(MUTED))
                .child(
                    mono(delta)
                        .w(px(64.))
                        .text_size(px(14.))
                        .flex()
                        .justify_end(),
                )
        };
    ui::pane()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(12.))
        .py(px(16.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(10.))
                .px(px(10.))
                .pb(px(10.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Changes"),
                )
                .child(div().text_color(MUTED).child(format!(
                    "against the raw takes, {}",
                    timecode(raw * u64::from(FPS) / 1_000_000)
                ))),
        )
        .child(
            div()
                .id("change-lines")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(changes.removed.iter().map(|(by, count, length)| {
                    let name = match by {
                        RemovedBy::Manual => "Your cuts".to_owned(),
                        RemovedBy::Rule(rule) => format!("{} cuts", capitalized(rule.as_str())),
                    };
                    line(
                        "cut",
                        CUT,
                        name,
                        *count,
                        format!("−{}", timecode(length.0 * u64::from(FPS) / 1_000_000)),
                    )
                }))
                .child(line(
                    "zoom",
                    ZOOM,
                    "Punch-in zooms".into(),
                    changes.zooms,
                    String::new(),
                ))
                .child(line(
                    "layout",
                    TEXT,
                    "Layout changes".into(),
                    changes.layouts,
                    String::new(),
                )),
        )
        .children(this.active_card().map(|card| {
            div()
                .flex_none()
                .pt(px(8.))
                .child(super::proposals::card_element(this, card, true, cx))
        }))
}

fn capitalized(text: &str) -> String {
    let text = text.replace('-', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn lanes(
    this: &EditScreen,
    window: std::ops::Range<u64>,
    track: f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let chapter = this
        .map
        .chapter_at(this.playhead)
        .and_then(|c| this.map.chapters.get(c))
        .map(|c| c.title.clone())
        .unwrap_or_default();
    let x = {
        let window = window.clone();
        move |frame: u64| {
            model::fraction(
                frame.saturating_sub(window.start),
                window.end - window.start,
            )
        }
    };
    let fps = u64::from(FPS);
    let ruler: Vec<(f32, String)> = (window.start.div_ceil(5 * fps)..)
        .map(|i| i * 5 * fps)
        .take_while(|f| *f < window.end)
        .map(|f| (x(f), timecode(f)))
        .collect();
    ui::pane()
        .flex_1()
        .min_h_0()
        .m(px(8.))
        .pt(px(12.))
        .pb(px(10.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .px(px(16.))
                .h(px(24.))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(chapter))
                .child(
                    mono(format!(
                        "{} to {}",
                        timecode(window.start),
                        timecode(window.end)
                    ))
                    .text_size(TEXT_SM)
                    .text_color(MUTED),
                )
                .child(div().flex_1())
                .child(
                    label("Cut selected words")
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(kbd("⌫")),
                ),
        )
        .child(
            div()
                .flex()
                .ml(px(NAME_WIDTH))
                .mr(px(16.))
                .child(parts::overview(this, window.clone(), px(20.), cx)),
        )
        .child(
            div()
                .relative()
                .h(px(18.))
                .ml(px(NAME_WIDTH))
                .mr(px(16.))
                .font_family(MONO)
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .children(
                    ruler
                        .into_iter()
                        .map(|(at, text)| div().absolute().left(gpui::relative(at)).child(text)),
                ),
        )
        .child(
            div()
                .relative()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(lane(
                    "layout",
                    "Layout",
                    px(36.),
                    layout_lane(this, &window, &x),
                ))
                .child(lane("zoom", "Zoom", px(36.), zoom_lane(this, &window, &x)))
                .child(lane(
                    "picture",
                    "B-roll",
                    px(36.),
                    super::overlays::lane(this, &window, &x, "b-roll"),
                ))
                .child(lane(
                    "edit",
                    "Highlight",
                    px(36.),
                    super::overlays::lane(this, &window, &x, "highlight"),
                ))
                .child(lane(
                    "text",
                    "Words",
                    px(96.),
                    words_lane(this, &window, &x, track, cx),
                ))
                .child(lane("mic", "Voice", px(60.), voice_lane(this, &window, cx)))
                .child(playhead_line(this, &window)),
        )
}

fn lane(icon_name: &str, name: &'static str, height: Pixels, track: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .child(
            div()
                .w(px(NAME_WIDTH))
                .flex_none()
                .pl(px(16.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(TEXT_SM)
                .font_weight(FontWeight::MEDIUM)
                .text_color(MUTED)
                .child(icon(icon_name, ICON, MUTED))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .relative()
                .mr(px(16.))
                .h(height)
                .rounded(px(6.))
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
    let background: Hsla = background.into();
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(gpui::relative(left))
        .w(gpui::relative((right - left).max(0.002)))
        .rounded(px(6.))
        .px(px(8.))
        .bg(background)
        .text_color(color)
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(36.))
        .overflow_hidden()
        .whitespace_nowrap()
        .child(text)
}

fn layout_lane(
    this: &EditScreen,
    window: &std::ops::Range<u64>,
    x: impl Fn(u64) -> f32,
) -> impl IntoElement {
    let mut runs: Vec<(u64, u64, shot_core::Layout)> = Vec::new();
    for span in &this.map.spans {
        let frames = span.output_frames();
        if frames.end <= window.start || frames.start >= window.end {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.2 == span.layout && last.1 == frames.start => last.1 = frames.end,
            _ => runs.push((frames.start, frames.end, span.layout)),
        }
    }
    div()
        .size_full()
        .children(runs.into_iter().map(|(from, to, layout)| {
            block(
                x(from.max(window.start)),
                x(to.min(window.end)) - 0.002,
                BLOCK,
                BODY,
                layout_name(layout).to_owned(),
            )
        }))
}

fn zoom_lane(
    this: &EditScreen,
    window: &std::ops::Range<u64>,
    x: impl Fn(u64) -> f32,
) -> impl IntoElement {
    let mut zooms = Vec::new();
    for span in &this.map.spans {
        let frames = span.output_frames();
        if frames.end <= window.start || frames.start >= window.end {
            continue;
        }
        for zoom in &span.zooms {
            let snapped = shot_render::plan::snap(zoom.range, FPS);
            let from = snapped.start.max(span.source_frames.start);
            let to = snapped.end.min(span.source_frames.end);
            if from < to {
                let out = |f: u64| span.output_frame + (f - span.source_frames.start);
                zooms.push((out(from), out(to), zoom.scale));
            }
        }
    }
    div()
        .size_full()
        .children(zooms.into_iter().map(|(from, to, scale)| {
            block(
                x(from.max(window.start)),
                x(to.min(window.end)),
                ZOOM_TINT,
                ZOOM_TEXT,
                format!("{scale}%"),
            )
        }))
}

fn words_lane(
    this: &EditScreen,
    window: &std::ops::Range<u64>,
    x: impl Fn(u64) -> f32,
    track: f32,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let mut words = Vec::new();
    // Where each of the two word rows is free again, in pixels. A word that fits neither row
    // shows only through the waveform.
    let mut rows = [f32::MIN; 2];
    for block in &this.doc {
        for (index, word) in block.words.iter().enumerate() {
            let Some(frame) = word.output.filter(|f| window.contains(f)) else {
                continue;
            };
            if word.state != WordState::Kept && this.view == model::View::Edited {
                continue;
            }
            let selected = this
                .selection
                .is_some_and(|s| s.at == block.at && s.words().contains(&index));
            let at = block.at;
            let left = x(frame) * track;
            let width = word.text.chars().count() as f32 * WORD_CHAR + 6.;
            let Some(row) = rows.iter().position(|free| *free <= left) else {
                continue;
            };
            rows[row] = left + width + 4.;
            words.push(
                div()
                    .id((
                        "lane-word",
                        at.chapter * 1_000_000 + at.take * 100_000 + index,
                    ))
                    .absolute()
                    .top(px(8. + 32. * row as f32))
                    .left(gpui::relative(x(frame)))
                    .h(px(22.))
                    .px(px(3.))
                    .rounded(px(3.))
                    .text_size(TEXT_SM)
                    .line_height(px(22.))
                    .whitespace_nowrap()
                    .text_color(BODY)
                    .cursor_pointer()
                    .when(word.state != WordState::Kept, |w| {
                        w.text_color(CUT_TEXT).line_through()
                    })
                    .when(selected, |w| w.bg(SELECTION).text_color(STRONG))
                    .child(word.text.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.select_word(at, index, event.modifiers.shift, cx)
                        }),
                    )
                    .into_any_element(),
            );
        }
    }
    let seams: Vec<(f32, Option<String>)> = if this.view == model::View::Edited {
        this.doc
            .iter()
            .flat_map(|b| {
                b.paragraphs
                    .iter()
                    .flat_map(move |p| p.items.iter().map(move |i| (b, i)))
            })
            .filter_map(|(block, item)| match item {
                model::Item::Cut(run) => {
                    let frame = this.map.output_frame(block.at, run.range.to())?;
                    window
                        .contains(&frame)
                        .then(|| (x(frame), (!run.is_seam()).then(|| seconds(run.removed))))
                }
                model::Item::Word(_) => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    div()
        .size_full()
        .children(seams.into_iter().map(|(at, length)| {
            div()
                .absolute()
                .top(px(4.))
                .bottom(px(4.))
                .left(gpui::relative(at))
                .w(px(2.))
                .rounded(px(1.))
                .bg(if length.is_some() {
                    Hsla::from(CUT)
                } else {
                    Hsla::from(FAINT)
                })
                .children(length.map(|text| {
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .left(px(6.))
                        .font_family(MONO)
                        .text_size(px(11.))
                        .text_color(CUT_TEXT)
                        .whitespace_nowrap()
                        .child(text)
                }))
        }))
        .children(words)
}

/// Peaks of the mic under each column, through the edit. Removed media in Untrimmed shows red.
fn voice_lane(
    this: &EditScreen,
    window: &std::ops::Range<u64>,
    cx: &mut Context<EditScreen>,
) -> impl IntoElement {
    let cut = &this.folder.read(cx).project.cut;
    let removed_in = |at: shot_core::TakeAt| -> Vec<shot_core::TimeRange> {
        if this.view == model::View::Edited {
            return Vec::new();
        }
        cut.chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .map(|edit| {
                edit.ranges
                    .iter()
                    .filter_map(|r| match r {
                        shot_core::RangeEdit::Remove { range, .. } => Some(*range),
                        shot_core::RangeEdit::Keep(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let spans: Vec<VoiceSpan> = this
        .map
        .spans
        .iter()
        .filter(|s| s.output_frames().end > window.start && s.output_frame < window.end)
        .map(|s| VoiceSpan {
            output: s.output_frames(),
            source_start: s.source_frames.start,
            peaks: this.peaks.get(&s.take).cloned().flatten(),
            removed: removed_in(s.at),
        })
        .collect();
    let window = window.clone();
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), win, _| {
            let bar = px(3.);
            let columns = ((f32::from(bounds.size.width) / f32::from(bar)) as u64).max(1);
            let frames = window.end - window.start;
            for column in 0..columns {
                let from = window.start + column * frames / columns;
                let to = (window.start + (column + 1) * frames / columns).max(from + 1);
                let Some(span) = spans.iter().find(|s| s.output.contains(&from)) else {
                    continue;
                };
                let Some(peaks) = &span.peaks else {
                    continue;
                };
                let source = |f: u64| {
                    frame_time(
                        span.source_start + (f.min(span.output.end) - span.output.start),
                        FPS,
                    )
                };
                let time = source(from);
                let peak = peaks.over(time, source(to));
                let removed = span
                    .removed
                    .iter()
                    .any(|r| r.from() <= time && time < r.to());
                let height = (bounds.size.height - px(8.)) * peak.clamp(0.02, 1.0);
                let left = bounds.origin.x + bar * column as f32;
                let top = bounds.origin.y + (bounds.size.height - height) / 2.;
                win.paint_quad(
                    fill(
                        Bounds::new(point(left, top), size(bar - px(1.), height)),
                        if removed {
                            Hsla::from(CUT).opacity(0.35)
                        } else {
                            Hsla::from(WAVE)
                        },
                    )
                    .corner_radii(px(1.)),
                );
            }
        },
    )
    .size_full()
}

struct VoiceSpan {
    output: std::ops::Range<u64>,
    source_start: u64,
    peaks: Option<Arc<Peaks>>,
    /// Media the edit removes, shown red in Untrimmed.
    removed: Vec<shot_core::TimeRange>,
}

fn playhead_line(this: &EditScreen, window: &std::ops::Range<u64>) -> impl IntoElement {
    let at = model::fraction(
        this.playhead - window.start.min(this.playhead),
        window.end - window.start,
    );
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(NAME_WIDTH))
        .right(px(16.))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(gpui::relative(at))
                .w(px(2.))
                .rounded(px(1.))
                .bg(ACCENT),
        )
}
