//! A video folder on disk: its project files and the cached transcripts of its takes.
//!
//! The app holds no edit state of its own. Every edit writes `cut.kdl` through `shot-core`'s
//! canonical writer and reloads, so the screen always shows what the files say. The folder
//! opens through [`style::open`] like in `shot` and the MCP server, which migrates a video
//! written before the lock; its `format.kdl` is a layer over the channel's format, and every
//! write rebuilds its lock from the files, unless it shipped and the lock is frozen.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_core::{
    Brand, CustomLayout, Cut, Elements, Format, FormatLayer, Id, Layout, Lineup, Lock, Micros,
    Project, TakeAt, TakeWords, TimeRange, Transcript,
};
use shot_mcp::style::{self, Disk};
use shot_render::theme::Theme;
use shot_ship::lock::LOCK_FILE;

pub struct VideoFolder {
    pub dir: PathBuf,
    /// The files: `cut.kdl` as written, the settings resolved through the layers.
    pub project: Project,
    /// The cut as it plays, the lock's cuts as removed ranges (see [`crate::model::as_played`]),
    /// for the Takes view, which asks how much of each attempt plays.
    pub played: Cut,
    /// Cached transcripts by take. A take without one is missing here.
    pub transcripts: HashMap<Id, Transcript>,
    /// The committed elements of each take that has an elements file.
    pub elements: HashMap<Id, Elements>,
    /// What the brand gives the templates, read again on every write like export reads it:
    /// the preview draws with it and the screens check overlays against it.
    pub theme: Result<Arc<Theme>, String>,
    /// The video's `format.kdl`.
    layer: FormatLayer,
    /// The settings under the video's layer: the preset with the channel's layer.
    base: Format,
    lock: Lock,
    /// The channel repo, or the folder as a channel of one, and the video's slug.
    disk: Disk,
    slug: String,
    /// The files before each change, newest last, for undo.
    history: Vec<Snapshot>,
    /// Files that undo stepped back from, newest last, for redo.
    undone: Vec<Snapshot>,
    /// Inside [`VideoFolder::as_one_change`]: writes join the change instead of each being one.
    batching: bool,
    /// Inside [`VideoFolder::writing`]: this folder holds the video's writer lock.
    writing: bool,
}

/// The files undo and redo step through.
#[derive(Clone, PartialEq)]
struct Snapshot {
    cut: Cut,
    format: Format,
    /// The brand block of the video's `format.kdl`.
    brand: Option<Brand>,
    /// The channel's `brand/brand.kdl` as written, for channel layouts; `None` without one.
    channel_brand: Option<String>,
}

/// How many writes undo can step back through.
const HISTORY: usize = 200;

fn read(dir: &Path, name: &str) -> Result<Option<String>, String> {
    let path = dir.join(name);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    shot_fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

impl VideoFolder {
    /// Opens the video in `dir` as every front end does, see [`style::open`]: a video written
    /// before the lock is migrated first, and one whose migration would change its frames
    /// does not open.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let opened = {
            let _lock =
                shot_fs::WriteLock::acquire(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            style::open(dir)?
        };
        let mut folder = Self {
            dir: dir.to_owned(),
            played: opened.project.cut.clone(),
            project: opened.project,
            transcripts: HashMap::new(),
            elements: HashMap::new(),
            theme: Ok(Arc::default()),
            layer: opened.layer,
            base: opened.base,
            lock: opened.lock,
            disk: opened.disk,
            slug: opened.slug,
            history: Vec::new(),
            undone: Vec::new(),
            batching: false,
            writing: false,
        };
        folder.load_transcripts()?;
        folder.rebuild_lock()?;
        Ok(folder)
    }

    /// The video's lock as last built.
    pub fn lock(&self) -> &Lock {
        &self.lock
    }

    /// Reads the cached transcript of every take the cut uses that has none loaded, as after
    /// transcribing. A take recorded without a mic has no words, so its transcript is empty.
    pub fn load_transcripts(&mut self) -> Result<(), String> {
        for chapter in &self.project.cut.chapters {
            for edit in &chapter.takes {
                if self.transcripts.contains_key(&edit.take) {
                    continue;
                }
                if let Some(transcript) = shot_analyze::cache::read_cached(&self.dir, &edit.take)
                    .map_err(|e| e.to_string())?
                {
                    self.transcripts.insert(edit.take.clone(), transcript);
                } else if self.without_mic(&edit.take) {
                    let silent = shot_core::Transcript::new(edit.take.clone());
                    self.transcripts.insert(edit.take.clone(), silent);
                }
            }
        }
        Ok(())
    }

    /// Whether `take` was recorded with the mic off, so it has no words.
    pub fn without_mic(&self, take: &Id) -> bool {
        shot_capture::take::not_recorded(&self.dir, take, "mic.mp4")
    }

    /// Reads `elements/<take>.jsonl` of every take the cut uses.
    fn load_elements(&mut self) -> Result<(), String> {
        self.elements.clear();
        for edit in self.project.cut.chapters.iter().flat_map(|c| &c.takes) {
            let path = format!("elements/{}.jsonl", edit.take);
            if self.elements.contains_key(&edit.take) {
                continue;
            }
            if let Some(text) = read(&self.dir, &path)? {
                let elements =
                    Elements::parse(edit.take.clone(), &text).map_err(|e| format!("{path}:{e}"))?;
                self.elements.insert(edit.take.clone(), elements);
            }
        }
        Ok(())
    }

    /// Builds the video's lock from the files and writes `cut.lock` when it changed, unless the
    /// video shipped, then reads the elements and the theme again. Called after every write,
    /// and when analysis added elements.
    pub fn rebuild_lock(&mut self) -> Result<(), String> {
        self.writing(Self::store_lock)
    }

    fn store_lock(&mut self) -> Result<(), String> {
        let built = style::build_video(&self.disk, &self.slug)?;
        if !built.shipped() && built.lock != self.lock {
            self.disk.write(
                &style::video_path(&self.slug, LOCK_FILE),
                &built.lock.to_kdl(),
            )?;
            self.lock = built.lock;
        }
        self.played = crate::model::as_played(&self.project.cut, &self.lock);
        self.theme = Theme::load(&self.dir, self.project.video.format, &self.project.cut)
            .map(Arc::new)
            .map_err(|e| e.to_string());
        self.load_elements()
    }

    /// Writes `cut` as canonical `cut.kdl` and makes it the project's cut. A change can be
    /// undone.
    pub fn write_cut(&mut self, cut: Cut) -> Result<(), String> {
        self.as_one_change(|folder| folder.store(cut))
    }

    /// Runs `change`, whose writes of the cut and the format undo and redo as one step.
    pub fn as_one_change<T>(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.batching {
            return change(self);
        }
        self.writing(|folder| {
            let before = folder.snapshot();
            folder.batching = true;
            let result = change(folder);
            folder.batching = false;
            if folder.snapshot() != before {
                if folder.history.len() == HISTORY {
                    folder.history.remove(0);
                }
                folder.history.push(before);
                folder.undone.clear();
            }
            result
        })
    }

    /// Runs `change` holding the video's writer lock, so `shot` and the MCP server wait for it
    /// and it waits for them. Nested calls run inside the outer one's lock.
    fn writing<T>(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.writing {
            return change(self);
        }
        self.reopen_if_gone()?;
        let _lock = shot_fs::WriteLock::acquire(&self.dir)
            .map_err(|e| format!("{}: {e}", self.dir.display()))?;
        self.writing = true;
        let result = change(self);
        self.writing = false;
        result
    }

    /// Checks a channel video's worktree out again, at the same place, when it went: the board
    /// removes the worktree of a merged branch that has no edits when it opens.
    fn reopen_if_gone(&self) -> Result<(), String> {
        if self.dir.exists() {
            return Ok(());
        }
        match crate::board::channel::locate(&self.dir) {
            Some((repo, slug)) => crate::board::channel::prepare(&repo, &slug).map(drop),
            None => Ok(()),
        }
    }

    /// Writes the files before the last change back. False when there is nothing to undo.
    pub fn undo(&mut self) -> Result<bool, String> {
        let Some(snapshot) = self.history.pop() else {
            return Ok(false);
        };
        let current = self.snapshot();
        self.writing(|folder| folder.restore_snapshot(snapshot))?;
        self.undone.push(current);
        Ok(true)
    }

    /// Writes the files the last undo stepped back from. False when there is nothing to redo.
    pub fn redo(&mut self) -> Result<bool, String> {
        let Some(snapshot) = self.undone.pop() else {
            return Ok(false);
        };
        let current = self.snapshot();
        self.writing(|folder| folder.restore_snapshot(snapshot))?;
        self.history.push(current);
        Ok(true)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            cut: self.project.cut.clone(),
            format: self.project.format.clone(),
            brand: self.layer_brand(),
            channel_brand: self
                .brand_path()
                .and_then(|path| fs::read_to_string(path).ok()),
        }
    }

    fn restore_snapshot(&mut self, snapshot: Snapshot) -> Result<(), String> {
        if let Some(path) = self.brand_path()
            && snapshot.channel_brand != fs::read_to_string(&path).ok()
        {
            let repo = path.parent().and_then(Path::parent).unwrap_or(&path);
            let _channel = shot_fs::WriteLock::channel(repo).map_err(|e| e.to_string())?;
            match &snapshot.channel_brand {
                Some(text) => write(&path, text)?,
                None => fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            }
        }
        if snapshot.format != self.project.format {
            self.store_format(snapshot.format)?;
        }
        if snapshot.brand != self.layer_brand() {
            self.store_brand(snapshot.brand)?;
        }
        self.store(snapshot.cut)
    }

    fn store(&mut self, cut: Cut) -> Result<(), String> {
        write(&self.dir.join("cut.kdl"), &cut.to_kdl())?;
        self.project.cut = cut;
        self.load_transcripts()?;
        self.rebuild_lock()
    }

    /// Writes `format.kdl` so the settings resolve to `format`: the settings that differ from
    /// the channel's.
    pub fn write_format(&mut self, format: Format) -> Result<(), String> {
        self.as_one_change(|folder| folder.store_format(format))
    }

    fn store_format(&mut self, format: Format) -> Result<(), String> {
        self.layer = FormatLayer {
            theme: self.layer.theme.take(),
            brand: self.layer.brand.take(),
            rules: std::mem::take(&mut self.layer.rules),
            ..shot_core::settings_diff(&format, &self.base)
        };
        write(&self.dir.join("format.kdl"), &self.layer.to_kdl())?;
        self.project.format = format;
        self.rebuild_lock()
    }

    /// The brand block of the video's `format.kdl`, `None` without one.
    fn layer_brand(&self) -> Option<Brand> {
        self.layer.brand.clone()
    }

    /// The hand-held strength the video's own layer sets, `None` when it leaves it to the
    /// channel's brand.
    pub fn own_handheld(&self) -> Option<u8> {
        self.layer_brand().and_then(|b| b.handheld)
    }

    /// Sets the camera's hand-held strength in the video's layer, or with `None` leaves it to
    /// the channel's brand. A change can be undone.
    pub fn set_handheld(&mut self, strength: Option<u8>) -> Result<(), String> {
        let mut brand = self.layer_brand().unwrap_or_default();
        brand.handheld = strength.map(|s| s.min(100));
        let brand = (!brand.is_empty()).then_some(brand);
        self.as_one_change(|folder| folder.store_brand(brand))
    }

    /// Whether the video's own layer cuts the camera out, `None` when it leaves it to the
    /// channel's brand.
    pub fn own_cutout(&self) -> Option<bool> {
        self.layer_brand().and_then(|b| b.cutout)
    }

    /// Turns the camera's cut-out on or off in the video's layer, or with `None` leaves it to
    /// the channel's brand. A change can be undone.
    pub fn set_cutout(&mut self, on: Option<bool>) -> Result<(), String> {
        let mut brand = self.layer_brand().unwrap_or_default();
        brand.cutout = on;
        let brand = (!brand.is_empty()).then_some(brand);
        self.as_one_change(|folder| folder.store_brand(brand))
    }

    fn store_brand(&mut self, brand: Option<Brand>) -> Result<(), String> {
        self.layer.brand = brand;
        write(&self.dir.join("format.kdl"), &self.layer.to_kdl())?;
        self.rebuild_lock()
    }

    /// Removes the lock's overlay or sound `instance` of `take` through its element's override,
    /// see [`Cut::remove_instance`]. A change can be undone.
    pub fn remove_instance(
        &mut self,
        take: &Id,
        instance: &shot_core::Instance,
    ) -> Result<(), String> {
        let cut = self
            .project
            .cut
            .remove_instance(take, instance)
            .ok_or("Only the style rules' overlays and sounds can be removed here")?;
        self.write_cut(cut)
    }

    /// Turns removed media in `range` of the block at `at` back into kept media: a person's
    /// removals, and the lock's cuts there through `keep` overrides.
    pub fn restore(&mut self, at: TakeAt, range: TimeRange) -> Result<(), String> {
        let cut = self.project.cut.restore(at, range, &self.lock);
        self.write_cut(cut.map_err(|e| e.to_string())?)
    }

    /// Puts a finished take into the cut as one change: as a chapter of its own, with its
    /// layout and zoom hotkey markers as nodes, and then the cut composed again, see
    /// [`VideoFolder::propose`]. Returns how many beats the take plays now.
    pub fn append_take(&mut self, take: Id, duration: Micros) -> Result<usize, String> {
        let Some(range) = TimeRange::new(Micros(0), duration) else {
            return Err(format!("take {take} is empty"));
        };
        self.as_one_change(|folder| {
            folder.store_take(take.clone(), range)?;
            folder.propose(Some(&take))?;
            let lineup = folder.lineup();
            let cut = &folder.project.cut;
            Ok(lineup
                .rows()
                .filter(|(_, row)| {
                    row.attempts.iter().any(|a| a.take != take)
                        && cut.plays(&lineup, row, None).iter().any(|p| p.take == take)
                })
                .count())
        })
    }

    /// Every take of the video lined up against the script, see [`Lineup`]: the takes the cut
    /// uses and the ones in the media folder that left it, which a beat can go back to.
    pub fn lineup(&self) -> Lineup {
        let mut ids = crate::takes::media_takes(&self.dir);
        ids.extend(self.transcripts.keys().cloned());
        ids.sort();
        ids.dedup();
        let takes: Vec<_> = ids
            .iter()
            .filter_map(|id| crate::takes::read_take(self, id))
            .collect();
        let words: Vec<TakeWords<'_>> = takes
            .iter()
            .map(|(transcript, elements)| TakeWords {
                transcript,
                elements: elements.as_ref(),
            })
            .collect();
        Lineup::new(&self.project.script, &self.project.cut, &words)
    }

    /// Composes the cut from the takes as they line up now ([`Cut::compose`]): the best
    /// scored attempt plays each beat unless a person chose or edited there. `new` is a take
    /// just appended whole. A cut that cannot be composed, because a Short uses a take that
    /// would leave, stays as it is. Returns whether the cut changed.
    pub fn propose(&mut self, new: Option<&Id>) -> Result<bool, String> {
        let Ok(cut) = self.project.cut.compose(&self.lineup(), new) else {
            return Ok(false);
        };
        if cut == self.project.cut {
            return Ok(false);
        }
        self.write_cut(cut)?;
        Ok(true)
    }

    /// Takes `take` out of the video: out of the cut, its picks, overrides and Shorts, and
    /// moves its media and elements to the Trash, where Finder can put them back. Undo cannot
    /// bring the media back, so the history before this change is dropped.
    pub fn remove_take(&mut self, take: &Id) -> Result<(), String> {
        self.writing(|folder| {
            let mut cut = folder.project.cut.clone();
            for chapter in &mut cut.chapters {
                chapter.takes.retain(|edit| edit.take != *take);
            }
            cut.chapters.retain(|chapter| !chapter.takes.is_empty());
            cut.picks.retain(|p| p.take != *take);
            cut.left_cuts.retain(|l| l.take != *take);
            cut.overrides.retain(|o| o.take != *take);
            cut.caption_breaks.retain(|b| b.take != *take);
            for derived in &mut cut.derived {
                derived.clips.retain(|clip| clip.take != *take);
            }
            folder.store(cut)?;
            crate::trash::trash(&folder.dir.join("media").join(take.as_str()))?;
            let elements = folder.dir.join(format!("elements/{take}.jsonl"));
            if elements.exists() {
                crate::trash::trash(&elements)?;
            }
            folder.transcripts.remove(take);
            folder.history.clear();
            folder.undone.clear();
            folder.rebuild_lock()
        })
    }

    fn store_take(&mut self, take: Id, range: TimeRange) -> Result<(), String> {
        let markers = shot_capture::events::read_markers(&self.media(&take, "events.jsonl"))
            .map_err(|e| format!("{take}/events.jsonl: {e}"))?;
        let cut = shot_prompter::appended(
            &self.project.cut,
            &take,
            range,
            &markers,
            &self.project.format,
        );
        self.write_cut(cut)
    }

    /// The channel repo the video is in, `None` for a channel of one.
    fn channel_root(&self) -> Option<&Path> {
        self.disk
            .in_channel()
            .then(|| shot_render::theme::channel_root(&self.dir))
            .flatten()
    }

    /// Adds the Lottie at `source` to the channel's brand when the import check passes, see
    /// [`crate::place::add_lottie`], and rebuilds the lock so its stickers and hashes are in.
    pub fn add_lottie(&mut self, source: &Path) -> Result<crate::place::LottieImport, String> {
        let repo = self
            .channel_root()
            .ok_or("Lottie templates live in a channel's brand, and this video is not in one")?
            .to_owned();
        let imported = crate::place::add_lottie(&repo, source)?;
        self.rebuild_lock()?;
        Ok(imported)
    }

    /// Lays `look` over the caption look of the channel's theme and drops the video's own, so
    /// every video of the channel uses it. Brand files are not part of undo.
    pub fn set_brand_captions(&mut self, look: shot_core::CaptionLook) -> Result<(), String> {
        self.writing(|folder| folder.store_brand_captions(look))
    }

    fn store_brand_captions(&mut self, look: shot_core::CaptionLook) -> Result<(), String> {
        let repo = self
            .channel_root()
            .ok_or(
                "The caption look of a channel lives in its brand, and this video is not in one",
            )?
            .to_owned();
        shot_mcp::brand::change(&repo, |themes| {
            Ok(themes.with_active_look(|theme| theme.captions = theme.captions.merge(&look)))
        })?;
        let mut format = self.project.format.clone();
        format.caption_look = shot_core::CaptionLook::default();
        self.write_format(format)?;
        self.rebuild_lock()
    }

    /// The channel's `brand/brand.kdl`, where channel layouts and overlay presets live: `None`
    /// for a video outside a channel repo, whose brand is the default one.
    pub fn brand_path(&self) -> Option<PathBuf> {
        Some(self.channel_root()?.join("brand/brand.kdl"))
    }

    /// Saves `layout` in the channel's brand, replacing the one of its name, and offers it on
    /// the 1 to 4 keys through the video's format when `on_keys`, or stops offering it. One
    /// change with undo.
    pub fn save_layout(&mut self, layout: CustomLayout, on_keys: bool) -> Result<(), String> {
        let path = self.brand_path().ok_or(
            "Channel layouts live in a channel's brand, and this video is not in a channel",
        )?;
        let repo = path
            .parent()
            .and_then(Path::parent)
            .expect("brand/brand.kdl is in a repo")
            .to_owned();
        self.as_one_change(|folder| {
            let name = Layout::Custom(layout.name);
            shot_mcp::brand::change(&repo, |themes| Ok(themes.set_layout(layout)))?;
            let mut format = folder.project.format.clone();
            let listed = format.other_layouts.contains(&name);
            if on_keys && !listed && format.default_layout != name {
                format.other_layouts.push(name);
            } else if !on_keys && listed {
                format.other_layouts.retain(|l| *l != name);
            }
            if format != folder.project.format {
                folder.store_format(format)
            } else {
                folder.rebuild_lock()
            }
        })
    }

    pub fn media(&self, take: &Id, file: &str) -> PathBuf {
        self.dir.join("media").join(take.as_str()).join(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_change_and_the_cut_it_makes_undo_as_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
        file("video.kdl", "title \"A\"\nstatus editing\nformat opinion\n");
        file("script.md", "# A\n");
        file(
            "format.kdl",
            &shot_core::FormatKind::Opinion.preset().to_kdl(),
        );
        file(
            "cut.kdl",
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=20_000_000\n    }\n}\n",
        );
        let mut folder = VideoFolder::open(dir.path()).unwrap();
        let (cut, format) = (folder.project.cut.clone(), folder.project.format.clone());
        folder
            .as_one_change(|folder| {
                let mut changed = folder.project.format.clone();
                changed.prompter = shot_core::PrompterMode::FullScript;
                folder.write_format(changed)?;
                let shorter = folder
                    .project
                    .cut
                    .restore(
                        TakeAt {
                            chapter: 0,
                            take: 0,
                        },
                        TimeRange::new(Micros(0), Micros(1)).unwrap(),
                        folder.lock(),
                    )
                    .map_err(|e| e.to_string())?;
                folder.write_cut(shorter.append_take(
                    Id::new("t02").unwrap(),
                    "B",
                    TimeRange::new(Micros(0), Micros(5_000_000)).unwrap(),
                ))
            })
            .unwrap();
        assert!(folder.undo().unwrap());
        assert_eq!(folder.project.cut, cut);
        assert_eq!(folder.project.format, format);
        assert!(!folder.undo().unwrap(), "one step for both writes");
        assert_eq!(
            VideoFolder::open(dir.path()).unwrap().project.format,
            format
        );
    }

    #[test]
    fn a_take_recorded_without_a_mic_has_no_words_and_needs_no_transcription() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
        file("video.kdl", "title \"A\"\nstatus editing\nformat opinion\n");
        file("script.md", "# A\n");
        file(
            "format.kdl",
            &shot_core::FormatKind::Opinion.preset().to_kdl(),
        );
        file(
            "cut.kdl",
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=20_000_000\n    }\n}\n\
             chapter \"B\" {\n    take t02 {\n        keep from=0 to=20_000_000\n    }\n}\n",
        );
        fs::create_dir_all(dir.path().join("media/t01")).unwrap();
        fs::write(dir.path().join("media/t01/screen.mp4"), b"").unwrap();
        let folder = VideoFolder::open(dir.path()).unwrap();
        let (silent, cloned) = (Id::new("t01").unwrap(), Id::new("t02").unwrap());
        assert!(folder.without_mic(&silent));
        assert_eq!(folder.transcripts[&silent].words().len(), 0);
        // Without its take folder the take may have had a mic: it still wants transcribing.
        assert!(!folder.without_mic(&cloned));
        assert!(!folder.transcripts.contains_key(&cloned));
    }

    #[test]
    fn a_video_answers_its_lock_review_with_an_override() {
        let repo = tempfile::tempdir().unwrap();
        let video = repo.path().join("videos/demo");
        fs::create_dir_all(video.join("elements")).unwrap();
        fs::create_dir_all(repo.path().join("formats")).unwrap();
        let file = |path: &Path, text: &str| fs::write(path, text).unwrap();
        file(
            &repo.path().join("formats/opinion.kdl"),
            shot_core::FormatKind::Opinion.preset_source(),
        );
        file(
            &video.join("video.kdl"),
            "title \"A\"\nstatus editing\nformat opinion\n",
        );
        file(&video.join("script.md"), "# A\n");
        file(&video.join("format.kdl"), "pacing natural\n");
        file(
            &video.join("cut.kdl"),
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=20_000_000\n    }\n}\n",
        );
        file(&video.join(LOCK_FILE), "lock 1\n");
        file(
            &video.join("elements/t01.jsonl"),
            "{\"kind\":\"filler\",\"t\":5000000,\"to\":5500000,\"confidence\":60,\"by\":\"test\"}\n\
             {\"kind\":\"silence\",\"t\":10000000,\"to\":12000000,\"confidence\":99,\"by\":\"test\"}\n",
        );

        let mut folder = VideoFolder::open(&video).unwrap();
        assert_eq!(
            fs::read_to_string(video.join("format.kdl")).unwrap(),
            "",
            "opening drops the pacing, which never reached a video with a lock"
        );
        assert_eq!(
            folder.project.format.captions,
            shot_core::CaptionStyle::WordPop
        );
        let queue: Vec<_> = folder.project.cut.review_queue(folder.lock()).collect();
        assert_eq!(queue.len(), 1, "the unsure filler waits");
        assert!(
            fs::read_to_string(video.join(LOCK_FILE))
                .unwrap()
                .contains("cut from=10_050_000 to=11_950_000 on=silence"),
            "opening rebuilds the lock with the channel's rules"
        );

        let (at, review) = queue[0];
        let kept = folder
            .project
            .cut
            .decide(at, review.range, shot_core::Decision::Keep, folder.lock())
            .unwrap();
        folder.write_cut(kept).unwrap();
        let decided = folder.project.cut.clone();
        assert!(folder.undo().unwrap());
        assert_eq!(
            folder.project.cut.review_queue(folder.lock()).count(),
            1,
            "undo reopens the review"
        );
        assert!(folder.redo().unwrap());
        assert_eq!(folder.project.cut, decided);
        assert!(!folder.redo().unwrap(), "nothing left to redo");
        let cut = fs::read_to_string(video.join("cut.kdl")).unwrap();
        assert!(cut.ends_with("override t01 filler at=5_000_000 {\n    keep\n}\n"));
        assert!(
            !fs::read_to_string(video.join(LOCK_FILE))
                .unwrap()
                .contains("review")
        );
        assert_eq!(folder.project.cut.review_queue(folder.lock()).count(), 0);
    }

    #[test]
    fn a_removed_lock_overlay_and_placed_sound_stay_removed_until_undo() {
        let repo = tempfile::tempdir().unwrap();
        let video = repo.path().join("videos/demo");
        fs::create_dir_all(video.join("elements")).unwrap();
        fs::create_dir_all(repo.path().join("formats")).unwrap();
        let file = |path: &Path, text: &str| fs::write(path, text).unwrap();
        file(
            &repo.path().join("formats/opinion.kdl"),
            shot_core::FormatKind::Opinion.preset_source(),
        );
        file(
            &video.join("video.kdl"),
            "title \"A\"\nstatus editing\nformat opinion\n",
        );
        file(&video.join("script.md"), "# A\n");
        file(&video.join("format.kdl"), "");
        file(
            &video.join("cut.kdl"),
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=20_000_000\n    }\n}\n\n\
             override t01 word at=2_000_000 {\n    sfx pop\n}\n",
        );
        file(&video.join(LOCK_FILE), "lock 1\n");
        file(
            &video.join("elements/t01.jsonl"),
            "{\"kind\":\"word\",\"t\":2000000,\"to\":2400000,\"by\":\"test\",\"text\":\"ship\"}\n\
             {\"kind\":\"url\",\"t\":8000000,\"to\":9000000,\"by\":\"test\",\"url\":\"https://github.com/coderscantina/shot-studio\",\"host\":\"github\"}\n",
        );
        let take = Id::new("t01").unwrap();
        let mut folder = VideoFolder::open(&video).unwrap();
        let find = |folder: &VideoFolder, card: bool| {
            folder.lock().take(&take).and_then(|t| {
                t.instances
                    .iter()
                    .find(|i| match &i.applied {
                        shot_core::Applied::Overlay { template, .. } => {
                            card && template.as_str() == "link-card"
                        }
                        shot_core::Applied::Sfx { .. } => !card,
                        _ => false,
                    })
                    .cloned()
            })
        };
        let card = find(&folder, true).expect("the url rule places a card");
        let sound = find(&folder, false).expect("the placed sound");

        folder.remove_instance(&take, &card).unwrap();
        folder.remove_instance(&take, &sound).unwrap();
        assert_eq!(
            fs::read_to_string(video.join("cut.kdl")).unwrap(),
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=20_000_000\n    }\n}\n\n\
             override t01 url at=8_000_000 {\n    overlay link-card off\n}\n",
            "the card is off on its element and the sound's node is gone"
        );
        folder.rebuild_lock().unwrap();
        assert!(find(&folder, true).is_none(), "a rebuild keeps it off");
        assert!(find(&folder, false).is_none());

        assert!(folder.undo().unwrap());
        assert_eq!(
            find(&folder, false),
            Some(sound),
            "undo brings the sound back"
        );
        assert!(folder.undo().unwrap());
        assert_eq!(find(&folder, true), Some(card));
    }

    /// `scripts/fixtures.sh` writes the demo channel's videos as the app creates them: an empty
    /// `format.kdl` layer and the lock a build writes for the files of each stage.
    #[test]
    fn demo_channel_locks_are_what_a_build_writes() {
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/demo-channel");
        let repo = tempfile::tempdir().unwrap();
        let formats = repo.path().join("formats");
        fs::create_dir_all(&formats).unwrap();
        for &kind in shot_core::FormatKind::ALL {
            fs::write(formats.join(format!("{kind}.kdl")), kind.preset_source()).unwrap();
        }
        let mut checked = 0;
        for entry in fs::read_dir(fixtures.join("videos")).unwrap() {
            let source = entry.unwrap().path();
            let slug = source.file_name().unwrap().to_str().unwrap().to_owned();
            let video = repo.path().join("videos").join(&slug);
            fs::create_dir_all(&video).unwrap();
            fs::copy(source.join("video.kdl"), video.join("video.kdl")).unwrap();
            fs::write(video.join("format.kdl"), "").unwrap();
            let (lock, cut) = match read(&source, LOCK_FILE).unwrap() {
                Some(lock) => (lock, source.join("cut.kdl")),
                None => (
                    fs::read_to_string(fixtures.join("empty-cut.lock")).unwrap(),
                    PathBuf::new(),
                ),
            };
            if cut.exists() {
                fs::copy(&cut, video.join("cut.kdl")).unwrap();
            }
            let built = style::build_video(&Disk::new(repo.path()), &slug).unwrap();
            assert_eq!(built.lock.to_kdl(), lock, "{slug}");
            checked += 1;
        }
        assert!(checked >= 7, "checked {checked} videos");
    }

    #[test]
    fn a_retake_of_a_beat_plays_in_place_of_the_earlier_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, text: &str| fs::write(dir.path().join(name), text).unwrap();
        file(
            "video.kdl",
            "title \"A\"\nstatus recorded\nformat opinion\n",
        );
        file(
            "script.md",
            "# A\n\n## Hook\n\n- Hello there world.\n\n## Rule\n\n- Ship on fridays now.\n",
        );
        file(
            "format.kdl",
            &shot_core::FormatKind::Opinion.preset().to_kdl(),
        );
        file(
            "cut.kdl",
            "chapter \"Take t01\" {\n    take t01 {\n        keep from=0 to=8_000_000\n    }\n}\n",
        );
        let words = |take: &str, text: &str| {
            let mut transcript = Transcript::new(Id::new(take).unwrap());
            for (i, word) in text.split(' ').enumerate() {
                let from = i as u64 * 1_000_000;
                let range = TimeRange::new(Micros(from), Micros(from + 800_000)).unwrap();
                transcript
                    .push(shot_core::Word {
                        text: word.into(),
                        range,
                        confidence: 90,
                    })
                    .unwrap();
            }
            shot_analyze::cache::write_cached(dir.path(), &transcript).unwrap();
        };
        words("t01", "Hello there world. Ship on fridays now.");
        words("t02", "Hello there world.");
        let mut folder = VideoFolder::open(dir.path()).unwrap();
        let replaced = folder
            .append_take(Id::new("t02").unwrap(), Micros(3_000_000))
            .unwrap();
        assert_eq!(replaced, 1);
        let blocks: Vec<&str> = folder
            .project
            .cut
            .chapters
            .iter()
            .flat_map(|c| &c.takes)
            .map(|edit| edit.take.as_str())
            .collect();
        assert_eq!(
            blocks,
            ["t02", "t01"],
            "the retake opens the cut, t01 keeps the rest"
        );
        let titles: Vec<&str> = folder
            .project
            .cut
            .chapters
            .iter()
            .map(|c| c.title.as_str())
            .collect();
        assert_eq!(titles, ["Hook", "Rule"], "under the script's chapters");
        assert!(
            folder.undo().unwrap(),
            "one undo takes the whole take back out"
        );
        assert_eq!(folder.project.cut.chapters[0].takes.len(), 1);
    }

    #[test]
    fn appending_a_take_writes_a_canonical_chapter() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("video.kdl"),
            "title \"A\"\nstatus recorded\nformat opinion\n",
        )
        .unwrap();
        fs::write(dir.path().join("script.md"), "# A\n").unwrap();
        fs::write(
            dir.path().join("format.kdl"),
            shot_core::FormatKind::Opinion.preset().to_kdl(),
        )
        .unwrap();
        let mut folder = VideoFolder::open(dir.path()).unwrap();
        folder
            .append_take(Id::new("t02").unwrap(), Micros(12_500_000))
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("cut.kdl")).unwrap(),
            "chapter \"Take t02\" {\n    take t02 {\n        keep from=0 to=12_500_000\n    }\n}\n"
        );
        assert_eq!(
            VideoFolder::open(dir.path())
                .unwrap()
                .project
                .cut
                .chapters
                .len(),
            1
        );
    }
}
