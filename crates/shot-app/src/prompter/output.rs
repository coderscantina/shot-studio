//! The prompter output: a borderless window covering the prompter display, laid out for the
//! Elgato Prompter's 1024x600 panel. Black, so the beam splitter shows nothing but the text.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, DisplayId, Div, FontWeight, HighlightStyle, Hsla, Render, StyledText,
    Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions, div,
    prelude::*, px, rgb,
};
use shot_prompter::View;

use super::{Live, native};
use crate::theme::*;

/// Text colors on the prompter: said words white, the rest light enough to read through glass.
const SAID: u32 = 0xffffff;
const AHEAD: u32 = 0x9aa2ae;
const NEXT: u32 = 0x7b838f;
const PREVIOUS: u32 = 0x4f5560;

pub struct PrompterOutput {
    live: Arc<Live>,
    pub text_size: f32,
}

/// Opens the output window on `display`, without focus, and applies `mirror`.
pub fn open(
    live: Arc<Live>,
    display: u32,
    text_size: f32,
    mirror: bool,
    cx: &mut App,
) -> Option<WindowHandle<PrompterOutput>> {
    let id = DisplayId::new(u64::from(display));
    let bounds: Bounds<gpui::Pixels> = cx.find_display(id)?.bounds();
    let handle = cx
        .open_window(
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
                native::cover_screen(window);
                native::set_mirror(window, mirror);
                cx.new(|cx| PrompterOutput::new(live, text_size, cx))
            },
        )
        .ok()?;
    Some(handle)
}

impl PrompterOutput {
    fn new(live: Arc<Live>, text_size: f32, cx: &mut Context<Self>) -> Self {
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
        Self { live, text_size }
    }
}

impl Render for PrompterOutput {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let prompter = self.live.lock();
        let view = prompter.view();
        div()
            .size_full()
            .bg(rgb(0x000000))
            .font_family(SANS)
            .px(px(44.))
            .pt(px(28.))
            .pb(px(20.))
            .flex()
            .flex_col()
            .child(lines(&view, self.text_size))
            .child(status(&view, self.text_size * 0.34))
    }
}

/// Previous, current and next line, sized from the current line's `size` in points.
pub fn lines(view: &View, size: f32) -> Div {
    let current = view.current.map(|line| {
        let said: usize = line.words[..view.said]
            .iter()
            .map(|w| w.len() + 1)
            .sum::<usize>()
            .min(line.text().len());
        StyledText::new(line.text()).with_highlights([(
            0..said,
            HighlightStyle {
                color: Some(Hsla::from(rgb(SAID))),
                ..Default::default()
            },
        )])
    });
    let small = |text: Option<String>, scale: f32, color: u32| {
        div()
            .text_size(px(size * scale))
            .line_height(px(size * scale * 1.2))
            .text_color(rgb(color))
            .children(text)
    };
    // Centered: the camera sits behind the middle of the glass, so reading there keeps eye contact.
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(size * 0.35))
        .overflow_hidden()
        .child(
            small(view.previous.map(|l| l.text()), 0.5, PREVIOUS)
                .truncate()
                .flex_none(),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(size))
                .line_height(px(size * 1.18))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(AHEAD))
                .children(current),
        )
        .child(small(view.next.map(|l| l.text()), 0.62, NEXT).font_weight(FontWeight::MEDIUM))
}

/// Chapter and beat, retakes and time left along the bottom.
fn status(view: &View, size: f32) -> Div {
    let left = view.time_left.as_secs();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(size))
        .text_size(px(size))
        .text_color(rgb(NEXT))
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
                .text_color(rgb(AHEAD))
                .child(format!("~{}:{:02} left", left / 60, left % 60)),
        )
}
