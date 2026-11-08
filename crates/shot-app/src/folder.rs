//! A video folder on disk: its project files and the cached transcripts of its takes.
//!
//! The app holds no edit state of its own. Every edit writes `cut.kdl` through `shot-core`'s
//! canonical writer and reloads, so the screen always shows what the files say. A migrated
//! video (one with a `cut.lock`) has its `format.kdl` as a layer over the channel's format, and
//! every write rebuilds its lock from the files, unless it shipped and the lock is frozen.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_core::{
    Cut, Decision, Elements, Format, FormatLayer, Id, Lock, Micros, MigrateInput, Project, Script,
    TakeAt, TimeRange, Transcript, Video, ZoomBy,
};
use shot_mcp::style::{self, Disk, Repo};
use shot_render::theme::Theme;
use shot_ship::lock::LOCK_FILE;

pub struct VideoFolder {
    pub dir: PathBuf,
    /// The files: `cut.kdl` as written, the settings resolved through the layers.
    pub project: Project,
    /// The project as the screens show it: a migrated video's lock folded into its take
    /// blocks (see [`crate::style::shown`]). Equal to `project` before migration.
    pub shown: Project,
    /// Cached transcripts by take. A take without one is missing here.
    pub transcripts: HashMap<Id, Transcript>,
    /// The committed elements of each take that has an elements file.
    pub elements: HashMap<Id, Elements>,
    /// What the brand gives the templates, read again on every write like export reads it:
    /// the preview draws with it and the screens check overlays against it.
    pub theme: Result<Arc<Theme>, String>,
    style: Option<Styled>,
}

/// What a migrated video adds to its files.
struct Styled {
    /// The video's `format.kdl`.
    layer: FormatLayer,
    /// The settings under the video's layer: the preset with the channel's layer.
    base: Format,
    lock: Lock,
    /// The channel repo and the video's slug, which a build reads. A folder outside a channel
    /// repo keeps the lock it has, as `shot export` does.
    repo: Option<(Disk, String)>,
}

/// `cut` with `take` appended as a chapter of its own that keeps `range`, its layout and zoom
/// markers written as nodes. On a migrated video a marker zoom is a person's zoom.
pub fn appended(
    cut: &Cut,
    take: &Id,
    range: TimeRange,
    markers: &[shot_capture::events::Marker],
    format: &Format,
    migrated: bool,
) -> Cut {
    let mut cut = cut.append_take(take.clone(), format!("Take {take}"), range);
    if let Some(edit) = cut.chapters.last_mut().and_then(|c| c.takes.last_mut()) {
        shot_prompter::apply_markers(edit, markers, format);
        if migrated {
            for zoom in &mut edit.zooms {
                if zoom.by == Some(ZoomBy::Marker) {
                    zoom.by = None;
                }
            }
        }
    }
    cut
}

fn read(dir: &Path, name: &str) -> Result<Option<String>, String> {
    let path = dir.join(name);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

impl VideoFolder {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let required = |name: &str| {
            read(dir, name)?.ok_or_else(|| format!("{}: missing", dir.join(name).display()))
        };
        let parsed = |name: &str, error: shot_core::ParseError| format!("{name}:{error}");
        let video = Video::parse(&required("video.kdl")?).map_err(|e| parsed("video.kdl", e))?;
        let script = Script::parse(&required("script.md")?).map_err(|e| parsed("script.md", e))?;
        let cut = match read(dir, "cut.kdl")? {
            Some(text) => Cut::parse(&text).map_err(|e| parsed("cut.kdl", e))?,
            None => Cut::default(),
        };
        let (format, style) = match read(dir, LOCK_FILE)? {
            None => {
                let format =
                    Format::parse(&required("format.kdl")?).map_err(|e| parsed("format.kdl", e))?;
                (format, None)
            }
            Some(lock) => {
                let lock = Lock::parse(&lock).map_err(|e| parsed(LOCK_FILE, e))?;
                let layer = read(dir, "format.kdl")?
                    .map(|t| FormatLayer::parse(&t).map_err(|e| parsed("format.kdl", e)))
                    .transpose()?
                    .unwrap_or_default();
                let repo = Disk::locate(dir).ok();
                let channel = match &repo {
                    Some((disk, slug)) => {
                        let path = format!("formats/{}.kdl", video.format);
                        disk.read(slug, &path)?
                            .map(|bytes| {
                                let text = String::from_utf8(bytes)
                                    .map_err(|_| format!("{path} is not UTF-8"))?;
                                FormatLayer::parse(&text).map_err(|e| format!("{path}:{e}"))
                            })
                            .transpose()?
                    }
                    None => None,
                };
                let base = video.format.preset().resolve(channel.iter());
                let format = layer.apply(&base);
                let styled = Styled {
                    layer,
                    base,
                    lock,
                    repo,
                };
                (format, Some(styled))
            }
        };
        let project = Project {
            video,
            script,
            format,
            cut,
        };
        let mut folder = Self {
            dir: dir.to_owned(),
            shown: project.clone(),
            project,
            transcripts: HashMap::new(),
            elements: HashMap::new(),
            theme: Ok(Arc::default()),
            style,
        };
        folder.load_transcripts()?;
        folder.rebuild_lock()?;
        Ok(folder)
    }

    /// Whether the video's style comes from its lock.
    pub fn is_migrated(&self) -> bool {
        self.style.is_some()
    }

    /// A migrated video's lock as last built.
    pub fn lock(&self) -> Option<&Lock> {
        self.style.as_ref().map(|s| &s.lock)
    }

    /// Reads the cached transcript of every take the cut uses.
    fn load_transcripts(&mut self) -> Result<(), String> {
        for chapter in &self.project.cut.chapters {
            for edit in &chapter.takes {
                if self.transcripts.contains_key(&edit.take) {
                    continue;
                }
                if let Some(transcript) = shot_analyze::cache::read_cached(&self.dir, &edit.take)
                    .map_err(|e| e.to_string())?
                {
                    self.transcripts.insert(edit.take.clone(), transcript);
                }
            }
        }
        Ok(())
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

    /// Builds a migrated video's lock from the files and writes `cut.lock` when it changed,
    /// unless the video shipped, then reads the elements and the theme again. Called after
    /// every write, and when analysis added elements.
    pub fn rebuild_lock(&mut self) -> Result<(), String> {
        if let Some(styled) = &mut self.style
            && let Some((disk, slug)) = &styled.repo
        {
            let built = style::build_video(disk, slug)?;
            if !built.shipped() && built.lock != styled.lock {
                disk.write(&style::video_path(slug, LOCK_FILE), &built.lock.to_kdl())?;
                styled.lock = built.lock;
            }
        }
        self.shown = Project {
            cut: match &self.style {
                Some(styled) => crate::style::shown(&self.project.cut, &styled.lock),
                None => self.project.cut.clone(),
            },
            ..self.project.clone()
        };
        self.theme = Theme::load(&self.dir, self.project.video.format, &self.project.cut)
            .map(Arc::new)
            .map_err(|e| e.to_string());
        self.load_elements()
    }

    /// Writes `cut` as canonical `cut.kdl` and makes it the project's cut.
    pub fn write_cut(&mut self, cut: Cut) -> Result<(), String> {
        write(&self.dir.join("cut.kdl"), &cut.to_kdl())?;
        self.project.cut = cut;
        self.load_transcripts()?;
        self.rebuild_lock()
    }

    /// Writes `format.kdl` so the settings resolve to `format`: the whole format before
    /// migration, the settings that differ from the channel's after.
    pub fn write_format(&mut self, format: Format) -> Result<(), String> {
        let text = match &mut self.style {
            Some(styled) => {
                styled.layer = crate::style::layer(&format, &styled.base, &styled.layer);
                styled.layer.to_kdl()
            }
            None => format.to_kdl(),
        };
        write(&self.dir.join("format.kdl"), &text)?;
        self.project.format = format;
        self.rebuild_lock()
    }

    /// Answers the review item at `range` of the block at `at`: an override of its element
    /// in a migrated video, the old `ignore` or rule removal before.
    pub fn decide(
        &mut self,
        at: TakeAt,
        range: TimeRange,
        decision: Decision,
    ) -> Result<(), String> {
        let cut = match &self.style {
            Some(styled) => {
                crate::style::decide(&self.project.cut, &styled.lock, at, range, decision)
            }
            None => self.project.cut.decide(at, range, decision),
        };
        self.write_cut(cut.map_err(|e| e.to_string())?)
    }

    /// Turns removed media in `range` of the block at `at` back into kept media. In a
    /// migrated video the lock's cuts there get `keep` overrides.
    pub fn restore(&mut self, at: TakeAt, range: TimeRange) -> Result<(), String> {
        let cut = match &self.style {
            Some(styled) => crate::style::restore(&self.project.cut, &styled.lock, at, range),
            None => self.project.cut.restore(at, range),
        };
        self.write_cut(cut.map_err(|e| e.to_string())?)
    }

    /// Appends a chapter that keeps all of a new take, with its layout and zoom hotkey markers
    /// as nodes, and writes `cut.kdl`. A video's first take in a channel repo starts it on the
    /// lock; in a migrated video the zoom hotkeys are a person's zooms.
    pub fn append_take(&mut self, take: Id, duration: Micros) -> Result<(), String> {
        let Some(range) = TimeRange::new(Micros(0), duration) else {
            return Err(format!("take {take} is empty"));
        };
        if self.style.is_none() && self.project.cut.chapters.is_empty() {
            self.start_migrated()?;
        }
        let markers = shot_capture::events::read_markers(&self.media(&take, "events.jsonl"))
            .map_err(|e| format!("{take}/events.jsonl: {e}"))?;
        let cut = appended(
            &self.project.cut,
            &take,
            range,
            &markers,
            &self.project.format,
            self.style.is_some(),
        );
        self.write_cut(cut)
    }

    /// Migrates a video with nothing recorded yet, as `shot migrate` would: its `format.kdl`
    /// becomes the settings that differ from the channel's, and it gets a lock. Outside a
    /// channel repo nothing changes.
    fn start_migrated(&mut self) -> Result<(), String> {
        let Ok((disk, slug)) = Disk::locate(&self.dir) else {
            return Ok(());
        };
        let inputs = style::inputs(&disk, &slug)?;
        let migration = shot_core::migrate(&MigrateInput {
            project: &self.project,
            imported: &inputs.imported,
            brand: &inputs.brand,
            channel: &inputs.format,
            elements: &inputs.elements,
            assets: &inputs.assets,
        })
        .map_err(|e| e.to_string())?;
        if let Some(channel) = &migration.channel {
            disk.write(
                &format!("formats/{}.kdl", self.project.video.format),
                &channel.to_kdl(),
            )?;
        }
        let path = |file: &str| style::video_path(&slug, file);
        disk.write(&path("format.kdl"), &migration.video.to_kdl())?;
        disk.write(&path("cut.kdl"), &migration.cut.to_kdl())?;
        disk.write(&path(LOCK_FILE), &migration.lock.to_kdl())?;
        *self = Self::open(&self.dir)?;
        Ok(())
    }

    /// Adds the Lottie at `source` to the channel's brand when the import check passes, see
    /// [`crate::place::add_lottie`], and rebuilds the lock so its stickers and hashes are in.
    pub fn add_lottie(&mut self, source: &Path) -> Result<crate::place::LottieImport, String> {
        let repo = Disk::locate(&self.dir)
            .ok()
            .and(shot_render::theme::channel_root(&self.dir))
            .ok_or("Lottie templates live in a channel's brand, and this video is not in one")?
            .to_owned();
        let imported = crate::place::add_lottie(&repo, source)?;
        self.rebuild_lock()?;
        Ok(imported)
    }

    pub fn media(&self, take: &Id, file: &str) -> PathBuf {
        self.dir.join("media").join(take.as_str()).join(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_migrated_video_answers_its_lock_review_with_an_override() {
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
        assert!(folder.is_migrated());
        assert_eq!(folder.project.format.pacing, shot_core::Pacing::Natural);
        assert_eq!(
            folder.project.format.captions,
            shot_core::CaptionStyle::WordPop
        );
        let queue: Vec<_> = folder.shown.cut.review_queue().collect();
        assert_eq!(queue.len(), 1, "the unsure filler waits");
        assert!(
            fs::read_to_string(video.join(LOCK_FILE))
                .unwrap()
                .contains("cut from=10_050_000 to=11_950_000 on=silence"),
            "opening rebuilds the lock with the channel's rules"
        );

        let (at, review) = queue[0];
        folder.decide(at, review.range, Decision::Keep).unwrap();
        let cut = fs::read_to_string(video.join("cut.kdl")).unwrap();
        assert!(cut.ends_with("override t01 filler at=5_000_000 {\n    keep\n}\n"));
        assert!(
            !fs::read_to_string(video.join(LOCK_FILE))
                .unwrap()
                .contains("review")
        );
        assert_eq!(folder.shown.cut.review_queue().count(), 0);
        assert!(!shot_core::has_rule_nodes(&folder.project.cut));
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
