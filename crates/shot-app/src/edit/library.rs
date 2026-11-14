//! What the Library window does to the Edit screen it places into: a sound, a pop, a cutaway,
//! a music bed or a zoom at the selected words or the playhead. Each placement is a change to
//! `cut.kdl` through [`crate::place`] and [`EditScreen::edit`], so it undoes like the Edit
//! screen's own pickers.

use std::path::PathBuf;

use gpui::{App, Context};
use shot_core::{Id, Micros, Overlay, ParamValue, TakeAt, TimeRange};
use shot_render::overlay::{broll, pop};

use super::{EditScreen, music};
use crate::model;
use crate::place::{self, Pop};

/// How tall a meme pops in, in 4K output pixels: a third of the frame, twice an emoji.
const MEME_SIZE: i64 = 720;
/// How long a meme cutaway shows, as long as a B marker's slot.
const CUTAWAY_LENGTH: Micros = broll::MARKER_HOLD;

/// Something the Library places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// A sound by the name the lookup finds it by, on the word spoken then.
    Sound(Id),
    /// A bundled bed under the cut.
    Bed(&'static str),
    /// An audio file under the cut, copied into `sources/` when it is outside the folder.
    MusicFile(PathBuf),
    /// An emoji or a brand sticker popping in.
    Pop(Pop),
    /// A still popping in, its file relative to the video folder.
    ImagePop(String),
    /// A still as a b-roll cutaway over the frame, its file relative to the video folder.
    Cutaway(String),
    /// A punch-in over the selected words.
    Zoom,
    /// One of the channel's overlay presets at the playhead.
    Preset(shot_core::OverlayPreset),
}

/// Where the Library places things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub video: PathBuf,
    pub title: String,
    /// Where a placement lands: the selected words or the playhead, as a timecode.
    pub at: String,
    pub selected: bool,
}

impl EditScreen {
    /// Where the Library places things in this screen.
    pub(crate) fn library_target(&self, cx: &App) -> Target {
        let folder = self.folder.read(cx);
        let selected = self.selected();
        let frame = selected
            .and_then(|(at, range, ..)| self.map.output_frame(at, range.from()))
            .unwrap_or(self.playhead);
        Target {
            video: folder.dir.clone(),
            title: folder.project.video.title.clone(),
            at: model::timecode(frame),
            selected: selected.is_some(),
        }
    }

    /// The video's resolved brand, for its stickers.
    pub(crate) fn library_brand(&self, cx: &App) -> shot_core::Brand {
        self.folder.read(cx).lock().brand.clone()
    }

    /// The credits of the downloads the cut uses, for the description. Reads the credit files.
    pub(crate) fn library_credits(&self, cx: &App) -> Vec<shot_core::Credit> {
        let folder = self.folder.read(cx);
        shot_library::Folders::of_video(&folder.dir).credits(&folder.project.cut, folder.lock())
    }

    /// Stops the preview so a sound in the Library can be heard alone.
    pub(crate) fn pause_for_audition(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.set_playing(false);
            cx.notify();
        }
    }

    /// Places `placement` at the selected words, else at the playhead. Returns what it did, in
    /// a few words, or why it could not.
    pub(crate) fn place_from_library(
        &mut self,
        placement: Placement,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let moment = match self.selected() {
            Some((at, range, ..)) => Some((at, range.from())),
            None => self.map.source_at(self.playhead),
        };
        let outside = || "The playhead is outside the takes. Move it onto the cut".to_owned();
        match placement {
            Placement::Sound(sound) => {
                let (at, t) = moment.ok_or_else(outside)?;
                let take = self.take_at(at, cx).ok_or_else(outside)?;
                let folder = self.folder.read(cx);
                let (cut, word) = place::with_sound(
                    &folder.project.cut,
                    &take,
                    folder.elements.get(&take),
                    t,
                    sound.clone(),
                )?;
                self.edit(cx, |_| Ok(cut));
                Ok(format!("Placed {sound} on \"{word}\""))
            }
            Placement::Bed(name) => {
                self.add_bed(name, cx);
                Ok(format!("Put {name} under the cut"))
            }
            Placement::MusicFile(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.load_music(music::Source::File(path), cx);
                Ok(format!("Put {name} under the cut"))
            }
            Placement::Pop(what) => {
                let (at, t) = moment.ok_or_else(outside)?;
                self.place_overlay(at, place::pop(t, what), cx);
                Ok("Placed a pop".into())
            }
            Placement::ImagePop(file) => {
                let (at, t) = moment.ok_or_else(outside)?;
                let overlay = shot_render::overlay::overlay(
                    pop::TEMPLATE,
                    shown_for(t, place::POP_LENGTH),
                    vec![
                        ("image", ParamValue::Text(file)),
                        ("size", ParamValue::Integer(MEME_SIZE)),
                    ],
                );
                self.place_overlay(at, overlay, cx);
                Ok("Placed the image as a pop".into())
            }
            Placement::Cutaway(file) => {
                let (at, t) = moment.ok_or_else(outside)?;
                let overlay = shot_render::overlay::overlay(
                    broll::TEMPLATE,
                    shown_for(t, CUTAWAY_LENGTH),
                    vec![("file", ParamValue::Text(file))],
                );
                self.place_overlay(at, overlay, cx);
                Ok("Placed the image as a cutaway".into())
            }
            Placement::Zoom => {
                if self.selected().is_none() {
                    return Err("Select the words to zoom in on first".into());
                }
                self.add_zoom(cx);
                Ok("Zoomed in on the selected words".into())
            }
            Placement::Preset(preset) => {
                let name = sentence_case(preset.name.as_str());
                self.place_preset(preset, cx);
                Ok(format!("Placed {name}"))
            }
        }
    }

    fn take_at(&self, at: TakeAt, cx: &App) -> Option<Id> {
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

    fn place_overlay(&mut self, at: TakeAt, overlay: Overlay, cx: &mut Context<Self>) {
        self.edit(cx, |cut| place::with_overlay(cut, at, overlay));
    }
}

fn shown_for(t: Micros, length: Micros) -> TimeRange {
    TimeRange::new(t, Micros(t.0 + length.0)).expect("a length is positive")
}

/// `lower-third-guest` as "Lower third guest".
fn sentence_case(id: &str) -> String {
    let text = id.replace('-', " ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
