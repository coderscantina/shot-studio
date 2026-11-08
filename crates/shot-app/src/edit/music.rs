//! The music bed under the main cut, `music` in `cut.kdl`: add one of the bundled beds or an
//! audio file, replace it, change its gain, remove it. The Music lane's menu and toolbar call
//! these.
//!
//! Export reads the bed as a file relative to the video folder, so the bed has to be in it. A
//! bundled bed is written once to `sources/<name>.wav`, and a file from outside the folder is
//! copied into `sources/`. A file already in the folder is used where it is.

use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Context, Entity, PathPromptOptions, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use shot_audio::sound::{BUNDLED, EXTENSIONS, Kind};
use shot_core::Music;
use shot_media::audio::SAMPLE_RATE;

use super::EditScreen;
use super::timeline::Pick;

/// A new bed's gain in dB, and how far below it the bed drops under speech.
const GAIN: i16 = -18;
const DUCK_DEPTH: i16 = 12;
/// The gains the menu offers, in dB.
const GAINS: [i16; 5] = [-12, -15, -18, -21, -24];
/// One step of the toolbar's gain buttons, in dB, and the quietest gain they reach.
pub const STEP: i16 = 3;
const QUIETEST: i16 = -48;

/// The bundled beds by name.
pub fn beds() -> impl Iterator<Item = &'static str> {
    BUNDLED
        .iter()
        .filter(|b| b.kind == Kind::Music)
        .map(|b| b.name)
}

/// A gain as the app shows it, with a real minus: "−18 dB".
pub fn db(gain: i16) -> String {
    format!("{} dB", gain.to_string().replace('-', "−"))
}

/// The bed's name and gains in one line.
pub fn summary(music: &Music) -> String {
    format!(
        "{}, {}, {} under voice",
        bed_name(&music.file),
        db(music.gain),
        db(music.duck)
    )
}

/// The file a bundled bed is written to, relative to the video folder.
pub fn bed_file(name: &str) -> String {
    format!("sources/{name}.wav")
}

/// A short name for a bed's file: the bundled bed's name, else the file name.
pub fn bed_name(file: &str) -> &str {
    beds()
        .find(|name| bed_file(name) == file)
        .unwrap_or_else(|| file.rsplit('/').next().unwrap_or(file))
}

/// Where a new bed comes from.
pub(super) enum Source {
    Bundled(&'static str),
    File(PathBuf),
}

impl EditScreen {
    fn music(&self, cx: &App) -> Option<Music> {
        self.folder.read(cx).project.cut.music.clone()
    }

    /// Puts the bundled bed `name` under the cut, keeping the gain of the bed it replaces.
    pub(super) fn add_bed(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.load_music(Source::Bundled(name), cx);
    }

    /// Asks for an audio file and puts it under the cut.
    pub(super) fn choose_music(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use as music".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.load_music(Source::File(path), cx))
                .ok();
        })
        .detach();
    }

    /// Brings the bed into the video folder off the UI thread, then writes it to `cut.kdl`.
    pub(super) fn load_music(&mut self, source: Source, cx: &mut Context<Self>) {
        let dir = self.folder.read(cx).dir.clone();
        cx.spawn(async move |this, cx| {
            let file = cx
                .background_spawn(async move { into_folder(source, &dir) })
                .await;
            this.update(cx, |this, cx| match file {
                Ok(file) => {
                    let music = match this.music(cx) {
                        Some(music) => Music { file, ..music },
                        None => Music {
                            file,
                            gain: GAIN,
                            duck: GAIN - DUCK_DEPTH,
                        },
                    };
                    this.selection = None;
                    this.lanes.picked = Some(Pick::Music);
                    this.edit(cx, |cut| Ok(cut.set_music(Some(music))));
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Sets the bed's gain, moving the gain under speech along so the bed ducks as deep.
    pub(super) fn set_music_gain(&mut self, gain: i16, cx: &mut Context<Self>) {
        let Some(music) = self.music(cx) else {
            return;
        };
        let gain = gain.clamp(QUIETEST, 0);
        let duck = (music.duck + gain - music.gain).min(0);
        self.edit(cx, |cut| {
            Ok(cut.set_music(Some(Music {
                gain,
                duck,
                ..music
            })))
        });
    }

    pub(super) fn remove_music(&mut self, cx: &mut Context<Self>) {
        self.edit(cx, |cut| Ok(cut.set_music(None)));
    }
}

/// The bed's file relative to `dir`: where it already is in the folder, else copied or
/// written into `sources/`.
fn into_folder(source: Source, dir: &Path) -> Result<String, String> {
    let sources = dir.join("sources");
    let failed = |e: std::io::Error| format!("could not add the music to sources/: {e}");
    match source {
        Source::Bundled(name) => {
            let file = bed_file(name);
            let path = dir.join(&file);
            if !path.exists() {
                let bed = shot_audio::sound::bundled(name)
                    .ok_or_else(|| format!("no bundled bed `{name}`"))?;
                std::fs::create_dir_all(&sources).map_err(failed)?;
                std::fs::write(&path, wav(bed.decode().samples())).map_err(failed)?;
            }
            Ok(file)
        }
        Source::File(path) => {
            let audio = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
            if !audio {
                return Err(format!(
                    "pick an audio file for the music: {}",
                    EXTENSIONS.join(", ")
                ));
            }
            if let Some(inside) = relative(dir, &path) {
                return Ok(inside);
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("the music's file name is not readable text")?;
            let copy = copy_target(&sources, name, &path).map_err(failed)?;
            if !copy.exists() {
                std::fs::create_dir_all(&sources).map_err(failed)?;
                std::fs::copy(&path, &copy).map_err(failed)?;
            }
            relative(dir, &copy).ok_or_else(|| "the copied music is outside the folder".into())
        }
    }
}

/// `path` relative to `dir` with `/` between folders, when it lies inside.
fn relative(dir: &Path, path: &Path) -> Option<String> {
    let dir = dir.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    let inside = path.strip_prefix(&dir).ok()?;
    let parts: Option<Vec<&str>> = inside.iter().map(|p| p.to_str()).collect();
    Some(parts?.join("/"))
}

/// Where `sources/` keeps a copy of `original`: `name`, or the copy made before when one of
/// that name and size is there, else `name` with the first free number.
fn copy_target(sources: &Path, name: &str, original: &Path) -> std::io::Result<PathBuf> {
    let size = std::fs::metadata(original)?.len();
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    for n in 1.. {
        let candidate = sources.join(match n {
            1 => name.to_owned(),
            n => format!("{stem}-{n}.{extension}"),
        });
        match std::fs::metadata(&candidate) {
            Err(_) => return Ok(candidate),
            Ok(existing) if existing.len() == size => return Ok(candidate),
            Ok(_) => continue,
        }
    }
    unreachable!("some number is free")
}

/// Mono samples as a 16-bit WAV file at the mixer's rate.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(SAMPLE_RATE.to_le_bytes());
    out.extend((SAMPLE_RATE * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for sample in samples {
        out.extend(((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// The bundled beds and a file, to add a bed or replace the one there.
pub fn sources(menu: PopupMenu, screen: &Entity<EditScreen>, current: Option<&str>) -> PopupMenu {
    let menu = beds().fold(menu, |menu, name| {
        let screen = screen.clone();
        menu.item(
            PopupMenuItem::new(name)
                .checked(current == Some(bed_file(name).as_str()))
                .on_click(move |_, _, cx| screen.update(cx, |this, cx| this.add_bed(name, cx))),
        )
    });
    let screen = screen.clone();
    menu.separator().item(
        PopupMenuItem::new("Choose a file…")
            .on_click(move |_, _, cx| screen.update(cx, |this, cx| this.choose_music(cx))),
    )
}

/// The Music lane's menu: add a bed, or replace it, set its gain and remove it.
pub fn menu(
    menu: PopupMenu,
    screen: &Entity<EditScreen>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(music) = screen.read(cx).music(cx) else {
        let screen = screen.clone();
        return menu.submenu("Add music", window, cx, move |menu, _, _| {
            sources(menu, &screen, None)
        });
    };
    let file = music.file.clone();
    let menu = menu
        .label(summary(&music))
        .submenu("Replace", window, cx, {
            let screen = screen.clone();
            move |menu, _, _| sources(menu, &screen, Some(&file))
        })
        .submenu("Gain", window, cx, {
            let screen = screen.clone();
            move |menu, _, _| {
                GAINS.into_iter().fold(menu, |menu, gain| {
                    let screen = screen.clone();
                    menu.item(
                        PopupMenuItem::new(db(gain))
                            .checked(gain == music.gain)
                            .on_click(move |_, _, cx| {
                                screen.update(cx, |this, cx| this.set_music_gain(gain, cx))
                            }),
                    )
                })
            }
        });
    menu.separator()
        .item(PopupMenuItem::new("Remove music").action(Box::new(super::DeleteSelection)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundled_bed_is_written_once_and_files_outside_are_copied_in() {
        let root = tempfile::tempdir().unwrap();
        let video = root.path().join("video");
        std::fs::create_dir_all(video.join("sources")).unwrap();
        let name = beds().next().unwrap();
        let file = into_folder(Source::Bundled(name), &video).unwrap();
        assert_eq!(file, bed_file(name));
        assert_eq!(bed_name(&file), name);
        let written = std::fs::read(video.join(&file)).unwrap();
        assert_eq!(&written[..4], b"RIFF");

        let inside = video.join("sources/own.mp3");
        std::fs::write(&inside, b"mp3").unwrap();
        assert_eq!(
            into_folder(Source::File(inside), &video).unwrap(),
            "sources/own.mp3"
        );

        let outside = root.path().join("own.mp3");
        std::fs::write(&outside, b"other").unwrap();
        let copied = into_folder(Source::File(outside.clone()), &video).unwrap();
        assert_eq!(copied, "sources/own-2.mp3");
        // The same file again reuses its copy.
        assert_eq!(into_folder(Source::File(outside), &video).unwrap(), copied);

        let text = root.path().join("notes.txt");
        std::fs::write(&text, b"").unwrap();
        assert!(into_folder(Source::File(text), &video).is_err());
    }
}
