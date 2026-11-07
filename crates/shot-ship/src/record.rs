//! The upload record, `.shot/ship/record.json` in the video folder: what `shot ship` already
//! published, so a second run skips it and an interrupted upload resumes.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::youtube::Privacy;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// By cut name: `main` and the Shorts.
    #[serde(default)]
    pub uploads: BTreeMap<String, Upload>,
    /// The release tag, once the video was tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upload {
    /// Set once the upload finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
    /// The resumable session of an unfinished upload, and the size of the file it carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub size: u64,
    pub privacy: Privacy,
    /// The custom thumbnail was set.
    #[serde(default)]
    pub thumbnail: bool,
}

impl Record {
    pub fn path(video_dir: &Path) -> PathBuf {
        video_dir.join(".shot/ship/record.json")
    }

    /// The record, empty when nothing shipped yet.
    pub fn load(video_dir: &Path) -> io::Result<Self> {
        match fs::read_to_string(Self::path(video_dir)) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes the record through a temporary file, so a crash never leaves half a record.
    pub fn save(&self, video_dir: &Path) -> io::Result<()> {
        let path = Self::path(video_dir);
        fs::create_dir_all(path.parent().expect("has a parent"))?;
        let part = path.with_extension("json.part");
        let text = serde_json::to_string_pretty(self).expect("the record serializes");
        fs::write(&part, text + "\n")?;
        fs::rename(part, path)
    }

    /// The video id of a finished upload of `cut`.
    pub fn video_id(&self, cut: &str) -> Option<&str> {
        self.uploads.get(cut)?.video_id.as_deref()
    }
}
