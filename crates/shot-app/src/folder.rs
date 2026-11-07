//! A video folder on disk: its four project files and the cached transcripts of its takes.
//!
//! The app holds no edit state of its own. Every edit writes `cut.kdl` through `shot-core`'s
//! canonical writer and reloads, so the screen always shows what the files say.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use shot_core::{Cut, Format, Id, Micros, Project, Script, TimeRange, Transcript, Video};

pub struct VideoFolder {
    pub dir: PathBuf,
    pub project: Project,
    /// Cached transcripts by take. A take without one is missing here.
    pub transcripts: HashMap<Id, Transcript>,
}

impl VideoFolder {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let read = |name: &str| {
            let path = dir.join(name);
            fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let parsed = |name: &str, error: shot_core::ParseError| format!("{name}:{error}");
        let cut = match fs::read_to_string(dir.join("cut.kdl")) {
            Ok(text) => Cut::parse(&text).map_err(|e| parsed("cut.kdl", e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Cut::default(),
            Err(e) => return Err(format!("{}: {e}", dir.join("cut.kdl").display())),
        };
        let project = Project {
            video: Video::parse(&read("video.kdl")?).map_err(|e| parsed("video.kdl", e))?,
            script: Script::parse(&read("script.md")?).map_err(|e| parsed("script.md", e))?,
            format: Format::parse(&read("format.kdl")?).map_err(|e| parsed("format.kdl", e))?,
            cut,
        };
        let mut folder = Self {
            dir: dir.to_owned(),
            project,
            transcripts: HashMap::new(),
        };
        folder.load_transcripts()?;
        Ok(folder)
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

    /// Writes `cut` as canonical `cut.kdl` and makes it the project's cut.
    pub fn write_cut(&mut self, cut: Cut) -> Result<(), String> {
        let path = self.dir.join("cut.kdl");
        let tmp = self.dir.join(".cut.kdl.tmp");
        fs::write(&tmp, cut.to_kdl())
            .and_then(|()| fs::rename(&tmp, &path))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.project.cut = cut;
        self.load_transcripts()
    }

    /// Writes `format` as canonical `format.kdl` and makes it the project's format.
    pub fn write_format(&mut self, format: Format) -> Result<(), String> {
        let path = self.dir.join("format.kdl");
        let tmp = self.dir.join(".format.kdl.tmp");
        fs::write(&tmp, format.to_kdl())
            .and_then(|()| fs::rename(&tmp, &path))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        self.project.format = format;
        Ok(())
    }

    /// Appends a chapter that keeps all of a new take, and writes `cut.kdl`.
    pub fn append_take(&mut self, take: Id, duration: Micros) -> Result<(), String> {
        let Some(range) = TimeRange::new(Micros(0), duration) else {
            return Err(format!("take {take} is empty"));
        };
        let cut = self
            .project
            .cut
            .append_take(take.clone(), format!("Take {take}"), range);
        self.write_cut(cut)
    }

    pub fn media(&self, take: &Id, file: &str) -> PathBuf {
        self.dir.join("media").join(take.as_str()).join(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
