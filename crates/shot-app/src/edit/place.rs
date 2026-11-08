//! Placing one-off effects from the Edit screen: the sound picker (selection toolbar, Sound
//! lane, Picture), the pop picker with the brand's stickers and the Lottie import, and the
//! callout tool on the Picture preview. Every placement writes `cut.kdl` through
//! [`crate::place`], and the brand's files for a Lottie.
//!
//! Each sound in the picker has a play button that plays it on its own ([`Audition`]), so a
//! sound can be heard before it is placed. The picker stays open after a placement, so another
//! sound can replace it on the same word.

use std::time::Duration;

use gpui::{Context, Div, FontWeight, PathPromptOptions, SharedString, div, prelude::*, px};
use shot_core::{Id, Micros, TakeAt};
use shot_render::overlay::pop::EMOJI;

use super::EditScreen;
use crate::place::{self, LottieImport, Pop, SoundOrigin};
use crate::sound::Audition;
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
    /// Why sounds cannot be placed on this video, read when the sound picker opens.
    blocked: Option<String>,
    /// Plays a sound on its own, made with the first one played.
    audition: Option<Audition>,
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
        place.stop_audition();
        if place.picker == Some(Picker::Sound) {
            let folder = self.folder.read(cx);
            place.sounds = place::sounds(&folder.dir);
            // A placed sound plays from the lock, which only a channel repo rebuilds.
            place.blocked = match (&self.channel, folder.is_migrated()) {
                (Some(_), true) => None,
                (Some(_), false) => Some(format!(
                    "Sounds are part of the style: run shot migrate --video {} once to place \
                     them. You can listen to them now",
                    folder.dir.display()
                )),
                (None, _) => Some(
                    "Sounds are placed on a migrated video in a channel repo. You can listen \
                     to them here"
                        .into(),
                ),
            };
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
        self.place.stop_audition();
        open
    }

    /// Plays `sound` on its own, or stops it when it is the one playing. Pauses the preview.
    fn audition(&mut self, sound: Id, cx: &mut Context<Self>) {
        let place = &mut self.place;
        let audition = place.audition.get_or_insert_with(Audition::spawn);
        if audition.playing().as_ref() == Some(&sound) {
            audition.stop();
            cx.notify();
            return;
        }
        let Some(source) = place::library(&self.folder.read(cx).dir).find(&sound) else {
            place.note = Some(format!("{sound} is not in the sound folders any more"));
            cx.notify();
            return;
        };
        audition.play(sound, source);
        if self.playing {
            self.set_playing(false);
        }
        // Redraws when the sound ends, so its play button turns back.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let Ok(playing) = this.update(cx, |this, _| {
                    this.place
                        .audition
                        .as_ref()
                        .is_some_and(|a| a.playing().is_some())
                }) else {
                    return;
                };
                if !playing {
                    this.update(cx, |_, cx| cx.notify()).ok();
                    return;
                }
            }
        })
        .detach();
        cx.notify();
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
        if self.place.blocked.is_some() {
            return;
        }
        let Some((take, t)) = self
            .moment()
            .and_then(|(at, t)| Some((self.take_of(at, cx)?, t)))
        else {
            self.place.note =
                Some("The playhead is outside the takes. Move it onto a spoken word".into());
            cx.notify();
            return;
        };
        let folder = self.folder.read(cx);
        match place::with_sound(
            &folder.project.cut,
            &take,
            folder.elements.get(&take),
            t,
            sound.clone(),
        ) {
            Ok((cut, word)) => {
                self.place.stop_audition();
                self.place.note = Some(format!(
                    "Placed {sound} on \"{word}\". Press space to hear it in the \
                     cut, or pick another sound to replace it"
                ));
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

impl PlaceState {
    fn stop_audition(&self) {
        if let Some(audition) = &self.audition {
            audition.stop();
        }
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

/// The sounds a video can place, by where they come from. A click on a name places the sound
/// on the selected words, else at the playhead; the play button next to it plays it alone.
pub fn sound_picker(this: &EditScreen, cx: &mut Context<EditScreen>) -> Div {
    let place = &this.place;
    let playing = place.audition.as_ref().and_then(Audition::playing);
    let hint = match &place.blocked {
        Some(blocked) => div().text_color(WARN).child(blocked.clone()),
        None => div().text_color(MUTED).child(match this.selection {
            Some(_) => "Click a sound to place it on the selected words",
            None => "Click a sound to place it on the word at the playhead",
        }),
    };
    let mut panel = panel()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .font_weight(FontWeight::MEDIUM)
                .child("Sound")
                .child(div().flex_1())
                .child(kbd("esc")),
        )
        .child(hint.text_size(TEXT_XS));
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
                sound_chip(
                    name,
                    playing.as_ref() == Some(name),
                    place.blocked.is_none(),
                    cx,
                )
            })));
    }
    let warnings = [
        this.sound.warning(),
        place.audition.as_ref().and_then(Audition::error),
    ];
    panel.children(note(this)).children(
        warnings
            .into_iter()
            .flatten()
            .map(|w| div().text_size(TEXT_XS).text_color(WARN).child(w)),
    )
}

/// A sound's chip: a play button that plays it alone, lit while it plays, and its name, which
/// places it when `placeable`.
fn sound_chip(name: &Id, playing: bool, placeable: bool, cx: &mut Context<EditScreen>) -> Div {
    let (listen, place) = (name.clone(), name.clone());
    let play = div()
        .id(gpui::ElementId::Name(format!("play-{name}").into()))
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded_l(RADIUS_SM)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT_ON))
        .child(ui::icon(
            if playing { "pause" } else { "play" },
            ICON_SM,
            if playing { ACCENT } else { MUTED },
        ))
        .on_click(cx.listener(move |this, _, _, cx| this.audition(listen.clone(), cx)));
    let label = div()
        .id(gpui::ElementId::Name(format!("sound-{name}").into()))
        .flex()
        .items_center()
        .h_full()
        .pl(px(2.))
        .pr(px(10.))
        .rounded_r(RADIUS_SM)
        .child(name.to_string())
        .when(placeable, |label| {
            label
                .cursor_pointer()
                .hover(|s| s.bg(FLOAT_ON))
                .on_click(cx.listener(move |this, _, _, cx| this.place_sound(place.clone(), cx)))
        })
        .when(!placeable, |label| label.text_color(MUTED));
    div()
        .flex()
        .items_center()
        .h(px(28.))
        .rounded(RADIUS_SM)
        .bg(if playing { FLOAT_ON } else { RAISED })
        .border_1()
        .border_color(if playing { ACCENT } else { RAISED })
        .child(play)
        .child(label)
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
