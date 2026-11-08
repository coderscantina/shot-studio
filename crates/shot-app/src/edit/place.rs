//! Placing one-off effects from the Edit screen: the sound picker (selection toolbar, Sound
//! lane, Picture), the pop picker with the brand's stickers and the Lottie import, and the
//! callout tool on the Picture preview. Every placement writes `cut.kdl` through
//! [`crate::place`], and the brand's files for a Lottie.

use gpui::{Context, Div, FontWeight, PathPromptOptions, SharedString, div, prelude::*, px};
use shot_core::{Id, Micros, TakeAt};
use shot_render::overlay::pop::EMOJI;

use super::EditScreen;
use crate::place::{self, LottieImport, Pop, SoundOrigin};
use crate::theme::*;
use crate::ui::{self, kbd, label};

/// Which picker is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picker {
    Sound,
    Pop,
}

/// What the pickers show and the last placement said.
#[derive(Default)]
pub struct PlaceState {
    pub picker: Option<Picker>,
    /// Read when the sound picker opens.
    sounds: Vec<(Id, SoundOrigin)>,
    /// The next click on the Picture preview places a callout.
    pub callout: bool,
    note: Option<String>,
    /// The import check's findings for the last Lottie refused.
    findings: Vec<String>,
}

impl EditScreen {
    pub(super) fn toggle_picker(&mut self, picker: Picker, cx: &mut Context<Self>) {
        let place = &mut self.place;
        place.picker = (place.picker != Some(picker)).then_some(picker);
        place.callout = false;
        place.note = None;
        place.findings.clear();
        if place.picker == Some(Picker::Sound) {
            place.sounds = place::sounds(&self.folder.read(cx).dir);
        }
        cx.notify();
    }

    pub(super) fn toggle_callout(&mut self, cx: &mut Context<Self>) {
        self.place.callout = !self.place.callout;
        self.place.picker = None;
        self.place.note = self
            .place
            .callout
            .then(|| "Click the screen in the preview to place a callout".into());
        cx.notify();
    }

    /// Closes an open picker or the callout tool. Returns whether one was open.
    pub(super) fn close_picker(&mut self) -> bool {
        let open = self.place.picker.is_some() || self.place.callout;
        self.place.picker = None;
        self.place.callout = false;
        open
    }

    /// Where a placement goes: the start of the selected words, else the playhead.
    fn moment(&self) -> Option<(TakeAt, Micros)> {
        match self.selected() {
            Some((at, range, ..)) => Some((at, range.from())),
            None => self.map.source_at(self.playhead),
        }
    }

    fn take_of(&self, at: TakeAt, cx: &Context<Self>) -> Option<Id> {
        let cut = &self.folder.read(cx).project.cut;
        Some(
            cut.chapters
                .get(at.chapter)?
                .takes
                .get(at.take)?
                .take
                .clone(),
        )
    }

    pub(super) fn place_sound(&mut self, sound: Id, cx: &mut Context<Self>) {
        let Some((at, t)) = self.moment() else {
            return;
        };
        let Some(take) = self.take_of(at, cx) else {
            return;
        };
        let folder = self.folder.read(cx);
        if !folder.is_migrated() {
            self.place.note =
                Some("Sounds are style: run shot migrate on this video to place them".into());
            cx.notify();
            return;
        }
        match place::with_sound(
            &folder.project.cut,
            &take,
            folder.elements.get(&take),
            t,
            sound,
        ) {
            Ok(cut) => {
                self.place.picker = None;
                self.edit(cx, |_| Ok(cut));
            }
            Err(note) => {
                self.place.note = Some(note);
                cx.notify();
            }
        }
    }

    pub(super) fn place_pop(&mut self, pop: Pop, cx: &mut Context<Self>) {
        let Some((at, t)) = self.map.source_at(self.playhead) else {
            return;
        };
        self.place.picker = None;
        self.edit(cx, |cut| place::with_overlay(cut, at, place::pop(t, pop)));
    }

    /// Places a callout where the preview was clicked, `at` a fraction of its width and height,
    /// on the frame on show.
    pub(super) fn place_callout(&mut self, at: (f32, f32), cx: &mut Context<Self>) {
        let Some(shown) = &self.shown else {
            return;
        };
        let Some(point) = shown.screen.and_then(|s| s.screen_point(at)) else {
            self.place.note = Some("Callouts point at the screen recording; click on it".into());
            cx.notify();
            return;
        };
        let Some((block, t)) = self.map.source_at(shown.frame) else {
            return;
        };
        let Some(take) = self.take_of(block, cx) else {
            return;
        };
        let overlay = place::callout(self.folder.read(cx).elements.get(&take), t, point);
        self.place.callout = false;
        self.place.note = None;
        self.edit(cx, |cut| place::with_overlay(cut, block, overlay));
    }

    fn add_lottie(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add to the brand".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                let result = this.folder.update(cx, |folder, cx| {
                    let result = folder.add_lottie(&path);
                    cx.notify();
                    result
                });
                let place = &mut this.place;
                place.findings.clear();
                match result {
                    Ok(LottieImport::Added { name, file }) => {
                        place.note = Some(format!("Added sticker {name} as {file}"));
                    }
                    Ok(LottieImport::Refused(findings)) => {
                        place.note = Some(
                            "Not added: the renderer cannot draw everything in this file".into(),
                        );
                        place.findings = findings.iter().map(ToString::to_string).collect();
                    }
                    Err(error) => place.note = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(10.))
        .rounded(px(11.))
        .bg(FLOAT)
        .shadow_lg()
        .text_size(TEXT_SM)
}

fn chip(id: impl Into<SharedString>, text: impl Into<SharedString>) -> gpui::Stateful<Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .bg(RAISED)
        .hover(|s| s.bg(FLOAT_ON))
        .cursor_pointer()
        .child(text.into())
}

fn chips() -> Div {
    div().flex().flex_wrap().gap(px(4.))
}

/// The note of the last placement and the findings of a refused Lottie.
pub fn note(this: &EditScreen) -> Option<Div> {
    let place = &this.place;
    place.note.clone().map(|note| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .text_size(TEXT_XS)
            .text_color(MUTED)
            .child(note)
            .children(
                place
                    .findings
                    .iter()
                    .map(|f| div().font_family(MONO).text_color(BODY).child(f.clone())),
            )
    })
}

/// The sounds a video can place, by where they come from. A click places the sound on the
/// selected words, else at the playhead.
pub fn sound_picker(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let mut panel = panel().child(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .font_weight(FontWeight::MEDIUM)
            .child("Sound")
            .child(div().flex_1())
            .child(kbd("esc")),
    );
    for origin in [SoundOrigin::Video, SoundOrigin::Brand, SoundOrigin::Bundled] {
        let names: Vec<&Id> = this
            .place
            .sounds
            .iter()
            .filter(|(_, o)| *o == origin)
            .map(|(name, _)| name)
            .collect();
        if names.is_empty() {
            continue;
        }
        panel = panel
            .child(label(origin.label()).text_size(TEXT_XS))
            .child(chips().children(names.into_iter().map(|name| {
                let sound = name.clone();
                chip(format!("sound-{name}"), name.to_string()).on_click(
                    cx.listener(move |this, _, _, cx| this.place_sound(sound.clone(), cx)),
                )
            })));
    }
    panel.children(note(this))
}

/// The bundled animated emoji and the brand's stickers, placed at the playhead, and the
/// Lottie import.
pub fn pop_picker(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let stickers: Vec<Id> = this
        .folder
        .read(cx)
        .lock()
        .map(|l| place::stickers(&l.brand).cloned().collect())
        .unwrap_or_default();
    panel()
        .child(div().font_weight(FontWeight::MEDIUM).child("Pop"))
        .child(chips().children(EMOJI.iter().map(|(emoji, _)| {
            let emoji: &'static str = emoji;
            chip(format!("pop-{emoji}"), emoji)
                .text_size(px(18.))
                .on_click(cx.listener(move |this, _, _, cx| this.place_pop(Pop::Emoji(emoji), cx)))
        })))
        .child(label("Brand stickers").text_size(TEXT_XS))
        .child(
            chips()
                .children(stickers.into_iter().map(|name| {
                    let sticker = name.clone();
                    chip(format!("sticker-{name}"), name.to_string()).on_click(cx.listener(
                        move |this, _, _, cx| this.place_pop(Pop::Sticker(sticker.clone()), cx),
                    ))
                }))
                .child(
                    chip("add-lottie", "Add Lottie…")
                        .text_color(MUTED)
                        .on_click(cx.listener(|this, _, _, cx| this.add_lottie(cx))),
                ),
        )
        .children(note(this))
}

/// A button that opens a picker or a tool, lit while it is open.
pub fn tool_button(
    id: &'static str,
    icon: &'static str,
    text: &'static str,
    on: bool,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(30.))
        .px(px(10.))
        .rounded(RADIUS_SM)
        .cursor_pointer()
        .bg(if on { FLOAT_ON } else { FLOAT })
        .hover(|s| s.bg(FLOAT_ON))
        .text_size(TEXT_SM)
        .child(ui::icon(icon, ICON_SM, if on { STRONG } else { MUTED }))
        .child(text)
}
