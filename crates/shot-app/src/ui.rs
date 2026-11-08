//! Shared elements on raw gpui, built from the tokens in [`crate::theme`].

use gpui::{
    App, ClickEvent, Div, FontWeight, Hsla, InteractiveElement, ParentElement, Pixels,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Svg, Window, div,
    prelude::FluentBuilder, px, svg,
};

use crate::theme::{self, *};

/// A Hugeicons icon at `size`, colored like the text beside it unless told otherwise.
pub fn icon(name: &str, size: Pixels, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(theme::icon_path(name))
        .size(size)
        .flex_none()
        .text_color(color)
}

/// A keyboard hint such as `H` or `⌥1`.
pub fn kbd(keys: impl Into<SharedString>) -> Div {
    let keys = keys.into();
    // Geist Mono has no key symbols such as ⌫ and ⌥; the system font draws them at full size.
    let family = if keys.is_ascii() {
        MONO
    } else {
        ".SystemUIFont"
    };
    div()
        .font_family(family)
        .text_size(TEXT_XS)
        .px(px(6.))
        .py(px(1.))
        .rounded(px(5.))
        .bg(KBD)
        .text_color(KBD_TEXT)
        .child(keys)
}

/// A pane: a surface separated from its neighbours by shade.
pub fn pane() -> Div {
    div().bg(SURFACE).rounded(RADIUS_LG).min_h_0().min_w_0()
}

/// A quiet label above or beside a value.
pub fn label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(TEXT_SM)
        .font_weight(FontWeight::MEDIUM)
        .text_color(MUTED)
        .child(text.into())
}

/// Text in Geist Mono with tabular figures, for timecodes, durations and file names.
pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).child(text.into())
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One option of a [`segmented`] control.
pub struct Segment {
    pub label: SharedString,
    pub icon: Option<&'static str>,
    pub on: bool,
    /// Shows the icon alone, with the label as a tooltip, unless the segment is on.
    pub compact: bool,
    pub on_click: ClickHandler,
}

impl Segment {
    pub fn new(
        label: impl Into<SharedString>,
        on: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon: None,
            on,
            compact: false,
            on_click: Box::new(on_click),
        }
    }

    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
        self
    }

    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact && self.icon.is_some();
        self
    }
}

/// A segmented control in a well, the active segment raised with a shadow.
pub fn segmented(id: &'static str, well: impl Into<Hsla>, segments: Vec<Segment>) -> Stateful<Div> {
    let well: Hsla = well.into();
    div()
        .id(id)
        .flex()
        .gap(px(2.))
        .p(px(3.))
        .bg(well)
        .rounded(RADIUS_MD)
        .children(segments.into_iter().enumerate().map(|(index, segment)| {
            let on = segment.on;
            let color = if on { STRONG } else { MUTED };
            div()
                .id((id, index))
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(7.))
                .text_size(TEXT_SM)
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .cursor_pointer()
                .map(keyboard_reachable)
                .when(on, |this| this.bg(FLOAT).shadow_sm())
                .when(!on, |this| this.hover(|s| s.text_color(TEXT)))
                .when_some(segment.icon, |this, name| {
                    this.child(icon(name, ICON, color))
                })
                .map(|this| {
                    if segment.compact && !on {
                        let label = segment.label.clone();
                        this.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                        })
                    } else {
                        this.child(segment.label)
                    }
                })
                .on_click(segment.on_click)
        }))
}

/// A row of a key hint and what it does, like the mockup's `H Highlight`.
pub fn hint(keys: impl Into<SharedString>, what: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(px(14.))
        .text_color(KBD_TEXT)
        .child(kbd(keys))
        .child(what.into())
}

/// An alert block: an 8% tint of the tone with its icon and message.
pub fn alert(message: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .p(px(12.))
        .rounded(RADIUS_MD)
        .bg(Hsla::from(RECORD).opacity(0.08))
        .border_1()
        .border_color(Hsla::from(RECORD).opacity(0.3))
        .text_color(TEXT)
        .child(icon("alert", ICON, RECORD_TEXT))
        .child(div().flex_1().min_w_0().child(message.into()))
}

/// Makes a clickable element a tab stop that Enter or Space clicks, with the accent ring while
/// the keyboard focuses it. A mouse click leaves focus where it was, so the screen's keys keep
/// working, and Tab or Shift-Tab moves on to the next stop.
pub fn keyboard_reachable(element: Stateful<Div>) -> Stateful<Div> {
    element
        .tab_index(0)
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_visible(|s| s.border_color(ACCENT))
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .on_key_down(|event, window, cx| {
            let stroke = &event.keystroke;
            if stroke.key == "tab" && !stroke.modifiers.control && !stroke.modifiers.platform {
                if stroke.modifiers.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                cx.stop_propagation();
            }
        })
}

/// What a proposal action looks like: Accept in lime, Reject with its key, the rest quiet.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionLook {
    Accent,
    Plain,
    Quiet,
}

/// A button of an agent proposal card, with an optional icon and key hint.
pub fn proposal_action(
    id: impl Into<SharedString>,
    text: &'static str,
    icon_name: Option<&'static str>,
    keys: Option<&'static str>,
    look: ActionLook,
) -> Stateful<Div> {
    let accent = look == ActionLook::Accent;
    let color = match look {
        ActionLook::Accent => Hsla::from(ON_ACCENT),
        ActionLook::Plain => TEXT.into(),
        ActionLook::Quiet => MUTED.into(),
    };
    div()
        .id(id.into())
        .h(px(30.))
        .px(px(if icon_name.is_some() { 12. } else { 14. }))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(9.))
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(14.))
        .text_color(color)
        .cursor_pointer()
        .map(keyboard_reachable)
        .when(accent, |this| this.bg(ACCENT))
        .when(look == ActionLook::Plain, |this| {
            this.bg(RAISED).hover(|s| s.bg(FLOAT))
        })
        .when(look == ActionLook::Quiet, |this| {
            this.hover(|s| s.text_color(TEXT))
        })
        .when_some(icon_name, |this, name| {
            this.child(icon(name, ICON_SM, color))
        })
        .child(text)
        .when_some(keys, |this, keys| {
            this.child(if accent {
                kbd(keys)
                    .bg(Hsla::from(ON_ACCENT).opacity(0.14))
                    .text_color(ON_ACCENT)
            } else {
                kbd(keys)
            })
        })
}

/// An invisible canvas filling its parent that records where the parent was painted. A new
/// size draws one more frame, so layout that depends on it is never a frame stale.
pub fn measure(bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<Pixels>>>) -> gpui::Canvas<()> {
    gpui::canvas(
        move |b, window, _| {
            if bounds.get().size != b.size {
                // A refresh during this frame's paint is dropped, so ask for the next one.
                window.on_next_frame(|window, _| window.refresh());
            }
            bounds.set(b);
        },
        |_, _, _, _| {},
    )
}

/// A large button of an empty or finished state, lime when it is the way on, with its key.
pub fn big_button(
    id: &'static str,
    icon_name: &'static str,
    text: impl Into<SharedString>,
    keys: &'static str,
    accent: bool,
) -> Stateful<Div> {
    let color: Hsla = if accent {
        ON_ACCENT.into()
    } else {
        TEXT.into()
    };
    div()
        .id(id)
        .flex_none()
        .h(px(42.))
        .px(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(9.))
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .text_color(color)
        .font_weight(if accent {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .when(accent, |this| this.bg(ACCENT).hover(|s| s.opacity(0.9)))
        .when(!accent, |this| this.bg(RAISED).hover(|s| s.bg(FLOAT)))
        .child(icon(icon_name, ICON, color))
        .child(text.into())
        .child(if accent {
            kbd(keys)
                .bg(Hsla::from(ON_ACCENT).opacity(0.14))
                .text_color(ON_ACCENT)
        } else {
            kbd(keys)
        })
}

/// A centered empty state: an icon in a circle, a title, a line of text and its actions.
pub fn empty_state(
    icon_name: &'static str,
    title: impl Into<SharedString>,
    text: impl Into<SharedString>,
) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(
            div()
                .size(px(44.))
                .mb(px(6.))
                .rounded_full()
                .bg(RAISED)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(icon_name, px(22.), MUTED)),
        )
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .max_w(px(460.))
                .text_center()
                .line_height(px(22.))
                .text_color(BODY)
                .child(text.into()),
        )
}

/// A list in a pane that scrolls, with a fade over its bottom edge while more of it is below.
/// The list takes the room its parent gives it.
pub fn scrolling_list(id: &'static str, scroll: &gpui::ScrollHandle, list: Div) -> Div {
    let fade = scroll.clone();
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            list.id(id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(scroll),
        )
        // Painted after the list, so it reads this frame's scroll position.
        .child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let below = fade.max_offset().y + fade.offset().y;
                    if below <= px(1.) {
                        return;
                    }
                    let height = px(FADE).min(bounds.size.height);
                    let strip = gpui::Bounds::new(
                        gpui::point(bounds.origin.x, bounds.bottom() - height),
                        gpui::size(bounds.size.width, height),
                    );
                    let pane = Hsla::from(SURFACE);
                    window.paint_quad(gpui::fill(
                        strip,
                        gpui::linear_gradient(
                            180.,
                            gpui::linear_color_stop(pane.opacity(0.), 0.),
                            gpui::linear_color_stop(pane, 1.),
                        ),
                    ));
                },
            )
            .absolute()
            .size_full(),
        )
}

/// The height of the fade at the bottom of a [`scrolling_list`].
const FADE: f32 = 48.;
