//! A migrated video's `cut.lock`: the renderer reads its cuts, zooms and overlays, and never
//! resolves the style cascade itself ([0025]). A video without one is read from `cut.kdl` alone.
//!
//! [0025]: ../../../docs/decisions/0025-lock-carries-the-resolved-style.md

use std::io;
use std::path::{Path, PathBuf};

use shot_core::{Lock, ParseError, Project, Timeline};

/// The lock's file name in the video folder.
pub const LOCK_FILE: &str = "cut.lock";

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Parse(#[from] ParseError),
}

/// The video's lock, `None` when it has none yet.
pub fn read(video_dir: &Path) -> Result<Option<Lock>, LockError> {
    let path = video_dir.join(LOCK_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(Lock::parse(&text)?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LockError::Io { path, source }),
    }
}

/// The timeline the export shows: styled by the lock when the video has one.
pub fn timeline(video_dir: &Path, project: &Project) -> Result<Timeline, LockError> {
    Ok(match read(video_dir)? {
        Some(lock) => Timeline::styled(project, &lock),
        None => Timeline::evaluate(project),
    })
}
