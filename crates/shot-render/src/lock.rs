//! A video's `cut.lock`: the renderer reads its cuts, zooms and overlays, and never resolves the
//! style cascade itself ([0025]). Opening a video writes it, so a video without one was not
//! opened yet.
//!
//! [0025]: ../../../docs/decisions/0025-lock-carries-the-resolved-style.md

use std::io;
use std::path::{Path, PathBuf};

use shot_core::{Lock, ParseError, Project, Timeline};

/// The lock's file name in the video folder.
pub const LOCK_FILE: &str = "cut.lock";

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{0}: missing, open the video in Shot Studio or run `shot style build` to write it")]
    Missing(PathBuf),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Parse(#[from] ParseError),
}

/// The video's lock.
pub fn read(video_dir: &Path) -> Result<Lock, LockError> {
    let path = video_dir.join(LOCK_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Lock::parse(&text)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(LockError::Missing(path)),
        Err(source) => Err(LockError::Io { path, source }),
    }
}

/// The timeline the export shows, styled by the video's lock.
pub fn timeline(video_dir: &Path, project: &Project) -> Result<Timeline, LockError> {
    Ok(Timeline::evaluate(project, &read(video_dir)?))
}
