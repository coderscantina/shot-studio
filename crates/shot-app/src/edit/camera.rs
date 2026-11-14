//! The Camera section of the Picture inspector: the hand-held strength the video's brand layer
//! gives the camera, 0 (off) to 100, and whether it cuts the person out. A channel brand editor
//! sets the channel's own.

use gpui::{Context, Div, Stateful, div, prelude::*, px};
use gpui_component::switch::Switch;
use gpui_component::tooltip::Tooltip;

use super::EditScreen;
use super::zoom::{Knob, knob_row, slider};
use crate::theme::*;
use crate::ui::{self, label};

pub(super) fn section(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let folder = this.folder.read(cx);
    let own = folder.own_handheld();
    let own_cutout = folder.own_cutout();
    let cutout = folder.theme.as_ref().is_ok_and(|theme| theme.cutout);
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
    let reset = own.map(|_| {
        reset_button("camera-handheld-reset")
            .on_click(cx.listener(|this, _, _, cx| this.set_handheld(None, cx)))
    });
    let cutout_reset = own_cutout.map(|_| {
        reset_button("camera-cutout-reset")
            .on_click(cx.listener(|this, _, _, cx| this.set_cutout(None, cx)))
    });
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(10.))
        .px(px(10.))
        .child(label("Camera"))
        .child(knob_row(
                "Hand-held",
                slider(this, Knob::Handheld, "camera-handheld", strength, cx),
                value,
                reset,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(14.))
                    .child(div().text_color(MUTED).child("Cut out"))
                    .child(div().flex_1())
                    .children(cutout_reset)
                    .child(
                        Switch::new("camera-cutout")
                            .checked(cutout)
                            .accessibility_label("Cut the person out of the camera")
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.set_cutout(Some(*on), cx)
                            })),
                    ),
            )
            .when(cutout, |section| {
                section.child(
                    div()
                        .text_size(TEXT_XS)
                        .text_color(FAINT)
                        .child("A take shows the person once analysis found them. For takes recorded before, run shot analyze"),
                )
            })
}

fn reset_button(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(6.))
        .rounded(px(6.))
        .text_size(TEXT_XS)
        .text_color(MUTED)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT).text_color(TEXT))
        .map(ui::keyboard_reachable)
        .child("Reset")
        .tooltip(|window, cx| Tooltip::new("Back to the channel's brand").build(window, cx))
}

impl EditScreen {
    /// Turns the camera's cut-out on or off in the video's brand layer, or with `None` leaves it
    /// to the channel's brand.
    pub(super) fn set_cutout(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, cx| {
            let result = folder.set_cutout(on);
            cx.notify();
            result
        });
        self.wrote(result, cx);
    }
}
