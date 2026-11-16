//! The inputs bar over the stage: a chip per track that says whether it records and whether it
//! needs attention, and under it the open chip's switch, devices and privacy access.

use std::sync::atomic::Ordering;

use gpui::{Context, Div, FontWeight, Hsla, div, prelude::*, px};
use gpui_component::Disableable;
use gpui_component::Sizable;
use gpui_component::switch::Switch;
use shot_capture::{Access, Permission};

use super::{Picker, RecordScreen, Track, access, free_space, meter};
use crate::theme::*;
use crate::ui::{icon, mono};

/// What a chip needs from its track.
struct Chip {
    picker: Option<Picker>,
    track: Track,
    icon: &'static str,
    name: &'static str,
    /// The track records: it is on and has a device.
    records: bool,
    /// Access is missing or the camera sends black.
    warn: bool,
}

impl RecordScreen {
    fn chips(&self) -> [Chip; 4] {
        let inputs = self.choice.inputs;
        let denied = |a: Access| a == Access::Denied;
        [
            Chip {
                picker: Some(Picker::Display),
                track: Track::Screen,
                icon: "display",
                name: "Screen",
                records: inputs.screen && self.choice.display.is_some(),
                warn: inputs.screen && denied(self.permissions.screen),
            },
            Chip {
                picker: Some(Picker::Camera),
                track: Track::Camera,
                icon: "camera",
                name: "Camera",
                records: self.choice.camera().is_some(),
                warn: self.choice.camera().is_some()
                    && (denied(self.permissions.camera) || self.camera_black()),
            },
            Chip {
                picker: Some(Picker::Mic),
                track: Track::Mic,
                icon: "mic",
                name: "Mic",
                records: self.choice.mic().is_some(),
                warn: self.choice.mic().is_some() && denied(self.permissions.mic),
            },
            Chip {
                picker: None,
                track: Track::SystemAudio,
                icon: "system-audio",
                name: "System audio",
                records: inputs.system_audio,
                warn: false,
            },
        ]
    }

    fn camera_black(&self) -> bool {
        self.shared
            .monitor
            .lock()
            .expect("monitor")
            .as_ref()
            .is_some_and(|view| view.camera_black())
    }

    /// The chips, and the open one's settings under them.
    pub(super) fn inputs_bar(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        let level = f32::from_bits(self.shared.level.load(Ordering::Relaxed));
        let events_denied = self.permissions.events == Access::Denied;
        let open = self.chips().into_iter().find(|c| self.opened(c.track));
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .children(self.chips().into_iter().map(|chip| {
                        let on = self.opened(chip.track);
                        let track = chip.track;
                        let color: Hsla = if chip.warn {
                            WARN.into()
                        } else if chip.records {
                            TEXT.into()
                        } else {
                            FAINT.into()
                        };
                        div()
                            .id(chip.name)
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(32.))
                            .px(px(10.))
                            .rounded(RADIUS_SM)
                            .bg(if on { FLOAT } else { RAISED })
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT))
                            .child(icon(chip.icon, ICON_SM, color))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color)
                                    .when(!chip.records, |name| name.line_through())
                                    .child(chip.name),
                            )
                            .when(track == Track::Mic && chip.records, |chip| {
                                chip.child(div().w(px(36.)).child(meter(level)))
                            })
                            .when(chip.warn, |chip| chip.child(icon("alert", ICON_SM, WARN)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open = if this.opened(track) {
                                    None
                                } else {
                                    Some(track)
                                };
                                cx.notify();
                            }))
                    }))
                    .when(events_denied, |row| {
                        row.child(
                            div()
                                .id("events")
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(RADIUS_SM)
                                .bg(WARN_TINT)
                                .cursor_pointer()
                                .text_size(TEXT_SM)
                                .text_color(WARN)
                                .child(icon("cursor", ICON_SM, WARN))
                                .child("No cursor or keys")
                                .on_click(|_, _, cx| {
                                    cx.open_url(super::settings_url(Permission::InputMonitoring))
                                }),
                        )
                    }),
            )
            .children(open.map(|chip| self.chip_panel(chip, busy, cx)))
    }

    fn opened(&self, track: Track) -> bool {
        self.open == Some(track)
    }

    /// The open chip: the switch for the next takes, the devices to pick and privacy access.
    fn chip_panel(&self, chip: Chip, busy: bool, cx: &mut Context<Self>) -> Div {
        let track = chip.track;
        let on = track.get(self.choice.inputs);
        let options: Vec<(String, String)> = match chip.picker {
            Some(Picker::Display) => self
                .devices
                .displays
                .iter()
                .map(|d| {
                    (
                        d.id.to_string(),
                        format!("{}, {}x{}", self.screen_name(d), d.width, d.height),
                    )
                })
                .collect(),
            Some(Picker::Camera) => self
                .devices
                .cameras
                .iter()
                .map(|d| (d.unique_id.clone(), d.name.clone()))
                .collect(),
            Some(Picker::Mic) => self
                .devices
                .mics
                .iter()
                .map(|d| (d.unique_id.clone(), d.name.clone()))
                .collect(),
            None => Vec::new(),
        };
        let selected = match chip.picker {
            Some(Picker::Display) => self.choice.display.map(|d| d.to_string()),
            Some(Picker::Camera) => self.choice.camera.clone(),
            Some(Picker::Mic) => self.choice.mic.clone(),
            None => None,
        };
        let permission = match track {
            Track::Screen => Some((Permission::ScreenRecording, self.permissions.screen)),
            Track::Camera => Some((Permission::Camera, self.permissions.camera)),
            Track::Mic => Some((Permission::Microphone, self.permissions.mic)),
            Track::SystemAudio => None,
        };
        let empty = match chip.picker {
            Some(Picker::Display) => "No display found.",
            Some(Picker::Camera) => "No camera connected.",
            Some(Picker::Mic) => "No mic connected.",
            None => "Records the sound the Mac plays.",
        };
        let free = free_space(&self.folder.read(cx).dir)
            .map(|bytes| format!("{} GB free", bytes / 1_000_000_000));
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(10.))
            .rounded(RADIUS_MD)
            .bg(CARD)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Switch::new(("track", track as usize))
                            .small()
                            .checked(on)
                            .disabled(busy)
                            .when(busy, |switch| {
                                switch.tooltip("Tracks stay as they are while a take records")
                            })
                            .accessibility_label(track.label())
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                this.switch(track, *on, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(TEXT_SM)
                            .text_color(if on { TEXT } else { MUTED })
                            .child(if on {
                                track.label().to_owned()
                            } else {
                                track.off().to_owned()
                            }),
                    )
                    .when(track == Track::Screen, |row| {
                        row.children(free.map(|free| {
                            mono(format!("Crash safe, {free}"))
                                .text_size(TEXT_XS)
                                .text_color(MUTED)
                        }))
                    }),
            )
            .when(on, |panel| {
                panel
                    .when(options.is_empty(), |panel| {
                        panel.child(div().text_size(TEXT_SM).text_color(MUTED).child(empty))
                    })
                    .children(options.into_iter().enumerate().map(|(index, (id, text))| {
                        let picked = Some(&id) == selected.as_ref();
                        let picker = chip.picker;
                        div()
                            .id(("device", index))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(30.))
                            .px(px(8.))
                            .rounded(RADIUS_SM)
                            .text_size(TEXT_SM)
                            .when(!busy, |row| row.cursor_pointer().hover(|s| s.bg(FLOAT)))
                            .when(picked, |row| row.bg(FLOAT).text_color(STRONG))
                            .child(div().flex_1().truncate().child(text))
                            .when(picked, |row| row.child(icon("check", ICON_SM, STRONG)))
                            .when(!busy, |row| {
                                row.on_click(cx.listener(move |this, _, _, cx| {
                                    match picker {
                                        Some(Picker::Display) => {
                                            this.choice.display = id.parse().ok()
                                        }
                                        Some(Picker::Camera) => {
                                            this.choice.camera = Some(id.clone())
                                        }
                                        Some(Picker::Mic) => this.choice.mic = Some(id.clone()),
                                        None => {}
                                    }
                                    this.restart(cx);
                                    cx.notify();
                                }))
                            })
                    }))
                    .when(track == Track::Camera && self.camera_black(), |panel| {
                        panel.child(
                            div().text_size(TEXT_SM).text_color(WARN).child(
                                "Black frames. Check the lens cover. The take still records.",
                            ),
                        )
                    })
                    .children(permission.map(|(permission, state)| access(permission, state)))
            })
    }
}
