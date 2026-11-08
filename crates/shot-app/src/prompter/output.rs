//! The prompter output: a borderless window covering the prompter display, laid out for the
//! Elgato Prompter's 1024x600 panel. Black, so the beam splitter shows nothing but the text.
//!
//! Every way out works: Esc on the output, its close control, ⌘⇧P and the Prompter menu. The
//! window also closes itself when it lands on another display, which macOS does when the
//! prompter display goes away, so it never ends up covering the main display.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, DisplayId, Div, FocusHandle, FontWeight, HighlightStyle, Hsla, Render,
    StyledText, Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, prelude::*, px, relative, rgb,
};
use shot_prompter::View;

use super::settings::Settings;
use super::{Live, OUTPUT_CONTEXT, native};
use crate::theme::*;
use crate::ui::kbd;

/// The size the output is laid out for when the display is not known.
pub const ELGATO: (f32, f32) = (1024., 600.);

pub struct PrompterOutput {
    live: Arc<Live>,
    pub settings: Settings,
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
            focus,
            _display: watch,
        }
    }
}

impl Render for PrompterOutput {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let prompter = self.live.lock();
        let view = prompter.view();
        div()
            .key_context(OUTPUT_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .child(page(&view, &self.settings, 1.))
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

/// The whole output for `view` with the look of `settings`, at `scale` of the output's size: 1
/// on the prompter, smaller in the pane's preview.
pub fn page(view: &View, settings: &Settings, scale: f32) -> Div {
    let size = settings.text_size * scale;
    let margin = px(settings.margin * scale);
    let colors = settings.colors.palette();
    let reading = settings.reading_line / 100.;
    let column = || {
        div()
            .w(relative(settings.text_width / 100.))
            .flex()
            .flex_col()
    };
    let line = |text: Option<String>, scale: f32, color: u32| {
        div()
            .text_size(px(size * scale))
            .line_height(px(size * scale * settings.line_height))
            .text_color(rgb(color))
            .children(text)
    };
    let gap = px(size * 0.35);
    let text = match view.countdown {
        Some(seconds) => div().child(
            div()
                .absolute()
                .top(relative(reading))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .text_size(px(size * 2.))
                .line_height(px(size * 2.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(colors.said))
                .child(seconds.to_string()),
        ),
        None => div()
            // The previous line ends just above the reading line.
            .child(
                div()
                    .absolute()
                    .bottom(relative(1. - reading))
                    .left_0()
                    .right_0()
                    .pb(gap)
                    .px(margin)
                    .flex()
                    .justify_center()
                    .child(column().child(
                        line(view.previous.map(|l| l.text()), 0.5, colors.previous).truncate(),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .top(relative(reading))
                    .left_0()
                    .right_0()
                    .px(margin)
                    .flex()
                    .justify_center()
                    .child(
                        column()
                            .gap(gap)
                            .child(current(view, size, settings.line_height, &colors))
                            .child(
                                line(view.next.map(|l| l.text()), 0.62, colors.next)
                                    .font_weight(FontWeight::MEDIUM),
                            ),
                    ),
            ),
    };
    div()
        .size_full()
        .bg(rgb(0x000000))
        .font_family(SANS)
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(text.relative().flex_1().min_h_0().overflow_hidden())
        .child(
            status(view, size * 0.34, colors.next, colors.ahead)
                .px(margin)
                .pb(px(20. * scale)),
        )
}

/// The current line, its said words in the said color.
fn current(view: &View, size: f32, line_height: f32, colors: &super::settings::Palette) -> Div {
    let text = view.current.map(|line| {
        let said: usize = line.words[..view.said]
            .iter()
            .map(|w| w.len() + 1)
            .sum::<usize>()
            .min(line.text().len());
        StyledText::new(line.text()).with_highlights([(
            0..said,
            HighlightStyle {
                color: Some(Hsla::from(rgb(colors.said))),
                ..Default::default()
            },
        )])
    });
    div()
        .text_size(px(size))
        .line_height(px(size * line_height))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(colors.ahead))
        .children(text)
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
            view.chapter, view.beat, view.beats
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
