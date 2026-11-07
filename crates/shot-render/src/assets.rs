//! The files frames draw from besides the video tracks, read once per video folder: each take's
//! event log and the code-diff snapshots in `sources/`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_core::{Effects, Id, Overlay};

use crate::code_diff::{CodeDiff, CodeDiffOverlay};
use crate::events::{EventLog, EventsError};
use crate::frame::Look;

/// The event log of a take, by file name inside `media/<take>/`.
pub const EVENTS_FILE: &str = "events.jsonl";

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{path}: {source}")]
    Events { path: PathBuf, source: EventsError },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

/// Loads and keeps what [`Look`]s need from one video folder.
#[derive(Debug)]
pub struct Assets {
    dir: PathBuf,
    events: HashMap<Id, Option<Arc<EventLog>>>,
    diffs: HashMap<String, Arc<CodeDiff>>,
}

impl Assets {
    pub fn new(video_dir: &Path) -> Self {
        Self {
            dir: video_dir.to_owned(),
            events: HashMap::new(),
            diffs: HashMap::new(),
        }
    }

    /// What a segment of `take` with `overlays` draws with `effects`. A take without an event
    /// log draws no cursor or keystrokes.
    pub fn look(
        &mut self,
        take: &Id,
        overlays: &[Overlay],
        effects: Effects,
    ) -> Result<Look, AssetError> {
        let mut diffs = HashMap::new();
        for code in overlays.iter().filter_map(CodeDiffOverlay::of) {
            let diff = self.diff(&code.diff)?;
            diffs.insert(code.diff, diff);
        }
        Ok(Look {
            effects,
            events: self.events(take)?,
            diffs,
        })
    }

    pub fn events(&mut self, take: &Id) -> Result<Option<Arc<EventLog>>, AssetError> {
        if let Some(events) = self.events.get(take) {
            return Ok(events.clone());
        }
        let path = self.dir.join("media").join(take.as_str()).join(EVENTS_FILE);
        let events = match fs::read_to_string(&path) {
            Ok(text) => Some(Arc::new(EventLog::parse(&text).map_err(|source| {
                AssetError::Events {
                    path: path.clone(),
                    source,
                }
            })?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(AssetError::Io { path, source }),
        };
        self.events.insert(take.clone(), events.clone());
        Ok(events)
    }

    fn diff(&mut self, file: &str) -> Result<Arc<CodeDiff>, AssetError> {
        if let Some(diff) = self.diffs.get(file) {
            return Ok(diff.clone());
        }
        let path = self.dir.join(file);
        let text = fs::read_to_string(&path).map_err(|source| AssetError::Io {
            path: path.clone(),
            source,
        })?;
        let diff = Arc::new(CodeDiff::parse(&text));
        self.diffs.insert(file.to_owned(), diff.clone());
        Ok(diff)
    }
}
