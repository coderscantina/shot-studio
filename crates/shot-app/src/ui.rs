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
            on_click: Box::new(on_click),
        }
    }

    pub fn icon(mut self, name: &'static str) -> Self {
        self.icon = Some(name);
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
                .when(on, |this| this.bg(FLOAT).shadow_sm())
                .when(!on, |this| this.hover(|s| s.text_color(TEXT)))
                .when_some(segment.icon, |this, name| {
                    this.child(icon(name, ICON, color))
                })
                .child(segment.label)
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
        .child(div().flex_1().child(message.into()))
}
