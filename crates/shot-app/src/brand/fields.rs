//! Controls of the theme and overlay editors: text inputs and sliders made on first render and
//! kept showing the files' values, so undo and a switch of theme or preset show up in them
//! without bookkeeping. An input that has focus keeps what is typed into it.

use std::collections::HashMap;
use std::hash::Hash;

use gpui::{
    Context, Div, Entity, Focusable, FontWeight, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::slider::{Slider, SliderEvent, SliderState};

use crate::theme::*;

/// What an input reports: the text, and whether editing ended (Enter or focus left).
pub type OnInput<V, K> = fn(&mut V, &K, &str, bool, &mut Window, &mut Context<V>);
/// What a slider reports: the value, and whether the drag ended.
pub type OnSlide<V, K> = fn(&mut V, &K, f32, bool, &mut Context<V>);

/// Text inputs by key.
pub struct Inputs<K> {
    map: HashMap<K, Entity<InputState>>,
    subscriptions: Vec<Subscription>,
}

impl<K: Clone + Eq + Hash + 'static> Default for Inputs<K> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            subscriptions: Vec::new(),
        }
    }
}

impl<K: Clone + Eq + Hash + 'static> Inputs<K> {
    /// The input for `key`, showing `value` unless it has focus.
    pub fn get<V: 'static>(
        &mut self,
        key: K,
        value: &str,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<V>,
        on: OnInput<V, K>,
    ) -> Entity<InputState> {
        let input = match self.map.get(&key) {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value.to_owned())
                });
                let k = key.clone();
                self.subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, input, event: &InputEvent, window, cx| {
                        let done = match event {
                            InputEvent::Change => false,
                            InputEvent::PressEnter { .. } | InputEvent::Blur => true,
                            InputEvent::Focus => return,
                        };
                        let text = input.read(cx).value();
                        on(this, &k, &text, done, window, cx);
                    },
                ));
                self.map.insert(key, input.clone());
                input
            }
        };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        if !focused && input.read(cx).value().as_ref() != value {
            let value = value.to_owned();
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        input
    }

    /// Drops every input, as when the editor switches to another preset.
    pub fn clear(&mut self) {
        self.map.clear();
        self.subscriptions.clear();
    }
}

/// Sliders by key.
pub struct Sliders<K> {
    map: HashMap<K, Entity<SliderState>>,
    subscriptions: Vec<Subscription>,
}

impl<K: Clone + Eq + Hash + 'static> Default for Sliders<K> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            subscriptions: Vec::new(),
        }
    }
}

impl<K: Clone + Eq + Hash + 'static> Sliders<K> {
    /// The slider for `key` from `min` to `max`, showing `value`.
    #[allow(clippy::too_many_arguments)]
    pub fn get<V: 'static>(
        &mut self,
        key: K,
        value: f32,
        (min, max, step): (f32, f32, f32),
        window: &mut Window,
        cx: &mut Context<V>,
        on: OnSlide<V, K>,
    ) -> Entity<SliderState> {
        let slider = match self.map.get(&key) {
            Some(slider) => slider.clone(),
            None => {
                let slider = cx.new(|_| {
                    SliderState::new()
                        .min(min)
                        .max(max)
                        .step(step)
                        .default_value(value)
                });
                let k = key.clone();
                self.subscriptions.push(cx.subscribe(
                    &slider,
                    move |this, _, event: &SliderEvent, cx| match *event {
                        SliderEvent::Change(v) => on(this, &k, v.start(), false, cx),
                        SliderEvent::Release(v) => on(this, &k, v.start(), true, cx),
                    },
                ));
                self.map.insert(key, slider.clone());
                slider
            }
        };
        if (slider.read(cx).value().start() - value).abs() > f32::EPSILON {
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        slider
    }
}

/// A labelled row: the label at the left, the control, and an error or hint under it.
pub fn row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(32.))
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A text input in a well, outlined in the cut color when its value is refused.
pub fn input(state: &Entity<InputState>, refused: bool) -> Div {
    div()
        .px(px(6.))
        .rounded(RADIUS_SM)
        .bg(WELL)
        .border_1()
        .border_color(if refused { CUT } else { WELL })
        .text_size(TEXT_SM)
        .child(Input::new(state).appearance(false))
}

/// A slider with its value shown at the right.
pub fn slider(state: &Entity<SliderState>, label: &'static str, shown: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(Slider::new(state).accessibility_label(label).flex_1())
        .child(
            div()
                .w(px(52.))
                .flex_none()
                .flex()
                .justify_end()
                .text_size(TEXT_SM)
                .child(shown),
        )
}

/// A section heading with a line of what it changes.
pub fn heading(title: &'static str, about: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pb(px(6.))
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(div().text_size(TEXT_SM).text_color(MUTED).child(about))
}

/// A small group title inside a section.
pub fn group(title: impl Into<SharedString>) -> Div {
    div()
        .pt(px(10.))
        .text_size(TEXT_XS)
        .font_weight(FontWeight::MEDIUM)
        .text_color(MUTED)
        .child(title.into())
}

/// A quiet button in the editors.
pub fn button(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
) -> gpui::Stateful<Div> {
    crate::ui::keyboard_reachable(
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(10.))
            .rounded(RADIUS_SM)
            .bg(FLOAT)
            .hover(|s| s.bg(FLOAT_ON))
            .cursor_pointer()
            .text_size(TEXT_SM)
            .child(text.into()),
    )
}

/// A choice among a few values, lit when `on`.
pub fn chip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    on: bool,
) -> gpui::Stateful<Div> {
    crate::ui::keyboard_reachable(
        div()
            .id(id)
            .flex()
            .items_center()
            .h(px(28.))
            .px(px(10.))
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(if on { ACCENT } else { RAISED })
            .bg(if on { FLOAT_ON } else { RAISED })
            .text_color(if on { STRONG } else { BODY })
            .hover(|s| s.bg(FLOAT_ON))
            .cursor_pointer()
            .text_size(TEXT_SM)
            .child(text.into()),
    )
}

/// A seconds value as typed, `0.4` for 400 ms.
pub fn seconds(micros: u64) -> String {
    let text = format!("{:.2}", micros as f64 / 1e6);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Seconds as typed into microseconds: `0.4`, `0,4` or `1.25 s`.
pub fn parse_seconds(text: &str) -> Option<u64> {
    let text = text.trim().trim_end_matches('s').trim().replace(',', ".");
    let value: f64 = text.parse().ok()?;
    ((0.0..=3600.0).contains(&value)).then(|| (value * 1e6).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_read_and_write_back() {
        assert_eq!(seconds(400_000), "0.4");
        assert_eq!(seconds(5_000_000), "5");
        assert_eq!(parse_seconds("0,4"), Some(400_000));
        assert_eq!(parse_seconds("1.25 s"), Some(1_250_000));
        assert_eq!(parse_seconds("-1"), None);
        assert_eq!(parse_seconds("soon"), None);
    }
}
