//! The Camera section of the Picture inspector: the hand-held strength the video's brand layer
//! gives the camera, 0 (off) to 100. A channel brand editor sets the channel's own.

use gpui::{Context, Div, div, prelude::*, px};
use gpui_component::tooltip::Tooltip;

use super::EditScreen;
use super::zoom::{Knob, knob_row, slider};
use crate::theme::*;
use crate::ui::{self, label};

pub(super) fn section(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let folder = this.folder.read(cx);
    let migrated = folder.is_migrated();
    let own = folder.own_handheld();
    let strength = match this.knobs_dragging(Knob::Handheld) {
        Some(value) => value,
        None => folder
            .theme
            .as_ref()
            .map_or(0, |theme| u64::from(theme.handheld)),
    };
    let value = match strength {
        0 => "Off".to_owned(),
        n => n.to_string(),
    };
    let reset = own.filter(|_| migrated).map(|_| {
        div()
            .id("camera-handheld-reset")
            .px(px(6.))
            .rounded(px(6.))
            .text_size(TEXT_XS)
            .text_color(MUTED)
            .cursor_pointer()
            .hover(|s| s.bg(FLOAT).text_color(TEXT))
            .map(ui::keyboard_reachable)
            .child("Reset")
            .tooltip(|window, cx| Tooltip::new("Back to the channel's brand").build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.set_handheld(None, cx)))
    });
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(10.))
        .px(px(10.))
        .child(label("Camera"))
        .map(|section| {
            if migrated {
                section.child(knob_row(
                    "Hand-held",
                    slider(this, Knob::Handheld, "camera-handheld", strength, cx),
                    value,
                    reset,
                ))
            } else {
                section.child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(FAINT)
                        .child("The camera is styled by the channel's brand. Migrate the video to give it a hand-held drift"),
                )
            }
        })
}
