//! The prompter output: a borderless window covering the prompter display, laid out for the
//! Elgato Prompter's 1024x600 panel. Black, so the beam splitter shows nothing but the text,
//! unless the settings put the camera behind it. Scrolling over it moves the text by lines.
//!
//! Every way out works: Esc on the output, its close control, ⌘⇧P and the Prompter menu. The
//! window also closes itself when it lands on another display, which macOS does when the
//! prompter display goes away, so it never ends up covering the main display.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, DisplayId, Div, FocusHandle, FontWeight, HighlightStyle, Hsla, ObjectFit,
    Render, ScrollEvent, StyledText, Subscription, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, div, prelude::*, px, rgb, surface,
};
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_prompter::{Line, View};

use super::settings::Settings;
use super::{Live, OUTPUT_CONTEXT, native};
use crate::theme::*;
use crate::ui::kbd;

/// The size the output is laid out for when the display is not known.
pub const ELGATO: (f32, f32) = (1024., 600.);

pub struct PrompterOutput {
    live: Arc<Live>,
    pub settings: Settings,
    flow: Flow,
    wheel: Wheel,
    focus: FocusHandle,
    _display: Subscription,
}

/// Opens the output window on `display`, without taking focus from the app. `over_menu_bar`
/// keeps it above the menu bar and the Dock, for a display that only shows the prompter.
pub fn open(
    live: Arc<Live>,
    display: u32,
    settings: Settings,
    over_menu_bar: bool,
    cx: &mut App,
) -> Option<WindowHandle<PrompterOutput>> {
    let id = DisplayId::new(u64::from(display));
    let bounds: Bounds<gpui::Pixels> = cx.find_display(id)?.bounds();
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            display_id: Some(id),
            window_background: WindowBackgroundAppearance::Opaque,
            inactive_frame_interval: None,
            low_power_frame_interval: None,
            thermal_frame_interval: None,
            ..Default::default()
        },
        |window, cx| {
            native::cover_screen(window, over_menu_bar);
            native::set_mirror(window, settings.mirror);
            cx.new(|cx| PrompterOutput::new(live, settings, id, window, cx))
        },
    )
    .ok()
}

impl PrompterOutput {
    fn new(
        live: Arc<Live>,
        settings: Settings,
        display: DisplayId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Voice, steady scroll and hotkeys move the prompter from other threads.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Focused, so Esc reaches the output once a click makes it the key window.
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let watch = cx.observe_window_display(window, move |_, window, cx| {
            if window.display(cx).map(|d| d.id()) != Some(display) {
                window.remove_window();
            }
        });
        Self {
            live,
            settings,
            flow: Flow::default(),
            wheel: Wheel::default(),
            focus,
            _display: watch,
        }
    }
}

impl Render for PrompterOutput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let live = self.live.clone();
        let camera = self.settings.camera.then(|| live.camera()).flatten();
        let prompter = live.lock();
        let view = prompter.view();
        let viewport = window.viewport_size();
        let size = (f32::from(viewport.width), f32::from(viewport.height));
        let row = self.settings.text_size * self.settings.line_height;
        div()
            .key_context(OUTPUT_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                let lines = this.wheel.lines(event, row);
                if lines != 0 {
                    this.live.lock().step(lines);
                    cx.notify();
                }
            }))
            .child(page(
                &view,
                prompter.lines(),
                &self.settings,
                camera,
                1.,
                size,
                &mut self.flow,
                window,
            ))
            // Centered, so it stays in place and under the pointer when the output is mirrored.
            .child(
                div()
                    .absolute()
                    .top(px(10.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("close-prompter")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(RADIUS_SM)
                            .text_size(TEXT_SM)
                            .text_color(FAINT)
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT).text_color(TEXT))
                            .child("Close prompter")
                            .child(kbd("Esc"))
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            )
    }
}

/// Scrolling over a prompter page, turned into whole lines of the page's rows. What is left of a
/// line waits for the next event, so slow and fast swipes both move by lines.
#[derive(Default)]
pub struct Wheel(f32);

impl Wheel {
    /// Lines `event` moves the text on a page whose rows are `row` points tall: forward when
    /// the swipe pushes the text up, back when it pulls it down.
    pub fn lines(&mut self, event: &ScrollEvent, row: f32) -> isize {
        let row = row.max(1.);
        self.0 -= f32::from(event.delta.pixel_delta(px(row)).y);
        let lines = (self.0 / row).trunc();
        self.0 -= lines * row;
        lines as isize
    }
}

/// How long the text takes to glide to a new reading position.
const GLIDE: Duration = Duration::from_millis(320);
/// Lines drawn before and after the current one; the rest is off the page.
const BEFORE: usize = 3;
const AFTER: usize = 8;

/// The text as one column that scrolls, so the row being read always sits on the reading line.
/// Keeps where each line starts for the look it was measured with, and the glide between two
/// reading positions.
#[derive(Default)]
pub struct Flow {
    /// What the tops were measured for, and the top of each line and of the end.
    measured: Option<(FlowKey, Vec<f32>)>,
    glide: Option<Glide>,
}

#[derive(Clone, Copy, PartialEq)]
struct FlowKey {
    lines: usize,
    chars: usize,
    size: f32,
    width: f32,
    line_height: f32,
}

#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    since: Instant,
}

impl Glide {
    fn at(&self, now: Instant) -> f32 {
        let t = (now.duration_since(self.since).as_secs_f32() / GLIDE.as_secs_f32()).min(1.);
        let eased = 1. - (1. - t).powi(3);
        self.from + (self.to - self.from) * eased
    }
}

/// The look of one page: sizes in points at the page's scale.
struct Look {
    size: f32,
    row: f32,
    /// Space above a beat, and above a chapter's label.
    beat_gap: f32,
    sentence_gap: f32,
    label: f32,
    width: f32,
}

impl Look {
    fn font() -> gpui::Font {
        gpui::Font {
            weight: FontWeight::SEMIBOLD,
            ..gpui::font(SANS)
        }
    }

    /// Space above line `line`: a chapter's label, a beat's gap or a sentence's.
    fn above(&self, line: &Line, first: bool) -> f32 {
        if first {
            return if line.section.is_some() {
                self.label
            } else {
                0.
            };
        }
        match (&line.section, line.opens_beat) {
            (Some(_), _) => self.beat_gap + self.label,
            (None, true) => self.beat_gap,
            (None, false) => self.sentence_gap,
        }
    }

    fn shape(&self, text: &str, window: &Window) -> Option<gpui::WrappedLine> {
        let run = gpui::TextRun {
            len: text.len(),
            font: Self::font(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_text(
                text.to_owned().into(),
                px(self.size),
                &[run],
                Some(px(self.width)),
                None,
            )
            .ok()?
            .into_iter()
            .next()
    }
}

impl Flow {
    /// The top of every line and of the end, measured again when the lines or the look change.
    fn tops(&mut self, lines: &[Line], look: &Look, window: &Window) -> &[f32] {
        let key = FlowKey {
            lines: lines.len(),
            chars: lines
                .iter()
                .map(|l| l.words.iter().map(String::len).sum::<usize>())
                .sum(),
            size: look.size,
            width: look.width,
            line_height: look.row,
        };
        if self.measured.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut tops = Vec::with_capacity(lines.len() + 1);
            let mut y = 0.;
            for (index, line) in lines.iter().enumerate() {
                y += look.above(line, index == 0);
                tops.push(y);
                let rows = look
                    .shape(&line.text(), window)
                    .map_or(1, |shaped| shaped.wrap_boundaries.len() + 1);
                y += rows as f32 * look.row;
            }
            tops.push(y);
            self.measured = Some((key, tops));
        }
        &self.measured.as_ref().expect("measured above").1
    }

    /// Glides towards `to` and returns where the text is now.
    fn scroll(&mut self, to: f32, now: Instant) -> f32 {
        match self.glide {
            Some(glide) if glide.to == to => glide.at(now),
            Some(glide) => {
                let from = glide.at(now);
                self.glide = Some(Glide {
                    from,
                    to,
                    since: now,
                });
                from
            }
            None => {
                self.glide = Some(Glide {
                    from: to,
                    to,
                    since: now,
                });
                to
            }
        }
    }
}

/// The whole output for `view` with the look of `settings`, at `scale` of the output's size: 1
/// on the prompter, smaller in the pane's preview. `size` is the page's size in points. With a
/// `camera` frame the camera fills the page behind the text, as visible as the settings say for
/// the countdown or for reading.
#[allow(clippy::too_many_arguments)]
pub fn page(
    view: &View,
    lines: &[Line],
    settings: &Settings,
    camera: Option<CFRetained<CVPixelBuffer>>,
    scale: f32,
    size: (f32, f32),
    flow: &mut Flow,
    window: &Window,
) -> Div {
    let text_size = settings.text_size * scale;
    let margin = settings.margin * scale;
    let colors = settings.colors.palette();
    let status_size = text_size * 0.34;
    let status_height = status_size * 1.4 + 20. * scale;
    let area = (size.1 - status_height).max(0.);
    let reading = area * settings.reading_line / 100.;
    let look = Look {
        size: text_size,
        row: text_size * settings.line_height,
        beat_gap: text_size * 0.6,
        sentence_gap: text_size * 0.12,
        label: text_size * 0.75,
        width: ((size.0 - 2. * margin) * settings.text_width / 100.).max(1.),
    };
    let text = match (view.countdown, view.current) {
        (Some(seconds), _) => div().child(
            div()
                .absolute()
                .top(px(reading))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .text_size(px(text_size * 2.))
                .line_height(px(text_size * 2.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(colors.said))
                .child(seconds.to_string()),
        ),
        (None, None) => div(),
        (None, Some(current)) => {
            let tops = flow.tops(lines, &look, window).to_vec();
            // The row holding the next word to read sits on the reading line.
            let said = said_bytes(current, view.said);
            let row = look
                .shape(&current.text(), window)
                .and_then(|shaped| shaped.position_for_index(said, px(look.row)))
                .map_or(0., |at| f32::from(at.y));
            let target = tops[view.index] + row;
            let scroll = flow.scroll(target, Instant::now());
            let last = (view.index + 1 + view.ahead.len()).min(view.index + AFTER);
            let first = view.index.saturating_sub(BEFORE);
            div().children((first..last).map(|index| {
                let line = &lines[index];
                let top = reading + tops[index] - scroll;
                let (color, opacity) = match index.cmp(&view.index) {
                    std::cmp::Ordering::Less => (colors.previous, 0.7),
                    std::cmp::Ordering::Equal => (colors.ahead, 1.),
                    std::cmp::Ordering::Greater => (colors.ahead, 0.55),
                };
                let body = if index == view.index {
                    StyledText::new(line.text()).with_highlights([(
                        0..said,
                        HighlightStyle {
                            color: Some(Hsla::from(rgb(colors.said))),
                            ..Default::default()
                        },
                    )])
                } else {
                    StyledText::new(line.text())
                };
                div()
                    .absolute()
                    .top(px(top))
                    .left(px(margin + (size.0 - 2. * margin - look.width) / 2.))
                    .w(px(look.width))
                    .opacity(opacity)
                    .children(line.section.as_ref().map(|title| {
                        div()
                            .absolute()
                            .bottom_full()
                            .left_0()
                            .pb(px(text_size * 0.2))
                            .text_size(px(text_size * 0.36))
                            .line_height(px(text_size * 0.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(colors.next))
                            .child(title.clone())
                    }))
                    .child(
                        div()
                            .text_size(px(text_size))
                            .line_height(px(look.row))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(color))
                            .child(body),
                    )
            }))
        }
    };
    // Text fades out towards the edges, so lines enter and leave softly as it scrolls.
    let fade = |angle: f32| {
        gpui::linear_gradient(
            angle,
            gpui::linear_color_stop(gpui::black(), 0.),
            gpui::linear_color_stop(gpui::black().opacity(0.), 1.),
        )
    };
    div()
        .size_full()
        .relative()
        .bg(rgb(0x000000))
        .font_family(SANS)
        .flex()
        .flex_col()
        .overflow_hidden()
        .children(camera.map(|frame| {
            let shown = if view.countdown.is_some() {
                settings.camera_countdown
            } else {
                settings.camera_prompter
            };
            div()
                .absolute()
                .inset_0()
                .child(surface(frame).size_full().object_fit(ObjectFit::Cover))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(gpui::black().opacity(1. - shown / 100.)),
                )
        }))
        .child(
            text.relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(reading * 0.6))
                        .bg(fade(180.)),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(look.row))
                        .bg(fade(0.)),
                ),
        )
        .child(
            status(view, status_size, colors.next, colors.ahead)
                .px(px(margin))
                .pb(px(20. * scale)),
        )
}

/// Bytes of the current line's text up to the next word to read.
fn said_bytes(line: &Line, said: usize) -> usize {
    line.words[..said.min(line.words.len())]
        .iter()
        .map(|w| w.len() + 1)
        .sum::<usize>()
        .min(line.text().len())
}

/// Chapter and beat, retakes and time left along the bottom.
fn status(view: &View, size: f32, color: u32, strong: u32) -> Div {
    let left = view.time_left.as_secs();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(size))
        .text_size(px(size))
        .text_color(rgb(color))
        .child(div().flex_1().truncate().child(format!(
            "{} · beat {} of {}",
            view.chapter, view.part_beat, view.part_beats
        )))
        .child(format!(
            "{} {}",
            view.retakes,
            if view.retakes == 1 {
                "retake"
            } else {
                "retakes"
            }
        ))
        .child(
            div()
                .font_family(MONO)
                .text_color(rgb(strong))
                .child(format!("~{}:{:02} left", left / 60, left % 60)),
        )
}
