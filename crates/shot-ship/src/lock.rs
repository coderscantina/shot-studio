//! A migrated video's `cut.lock` as the ship tools read it, so chapter times, thumbnails and the
//! ship plan follow the frames export renders. The lock is read, never rebuilt here: `shot`
//! rebuilds it before shipping, the app on every edit.

use std::io;
use std::path::{Path, PathBuf};

use shot_core::{Lock, ParseError, Project, Timeline};

use crate::spoken::Bookends;

/// The lock's file name in the video folder. A video with one is migrated.
pub const LOCK_FILE: &str = "cut.lock";

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("cut.lock: {0}")]
    Parse(#[from] ParseError),
    /// Placing the brand's intro and outro failed, for example on a missing bumper.
    #[error("{0}")]
    Bookends(String),
}

/// The video's lock, `None` when it is not migrated.
pub fn read(video_dir: &Path) -> Result<Option<Lock>, LockError> {
    let path = video_dir.join(LOCK_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(Lock::parse(&text)?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LockError::Io { path, source }),
    }
}

/// The timeline export renders: styled by the lock when the video has one.
pub fn timeline(video_dir: &Path, project: &Project) -> Result<Timeline, LockError> {
    Ok(match read(video_dir)? {
        Some(lock) => Timeline::styled(project, &lock),
        None => Timeline::evaluate(project),
    })
}

/// The main cut's intro and outro where export places them, for chapter times: none before
/// the video is migrated or when its brand has neither.
#[cfg(target_os = "macos")]
pub fn bookends(video_dir: &Path, project: &Project) -> Result<Bookends, LockError> {
    use shot_render::join::Piece;
    let has_bookends =
        read(video_dir)?.is_some_and(|l| l.brand.intro.is_some() || l.brand.outro.is_some());
    if !has_bookends {
        return Ok(Bookends::default());
    }
    let plan = shot_render::export::preview_plan(video_dir, project)
        .map_err(|e| LockError::Bookends(e.to_string()))?;
    let placed = plan
        .pieces
        .iter()
        .enumerate()
        .filter_map(|(index, piece)| {
            let Piece::Bookend(b) = *piece else {
                return None;
            };
            let before = match plan.pieces.get(index + 1) {
                Some(Piece::Segment(s)) => {
                    let segment = &plan.segments[*s].segment;
                    Some((segment.take.clone(), segment.source.from()))
                }
                _ => None,
            };
            Some(crate::spoken::Bookend {
                before,
                length: shot_render::plan::frame_time(plan.bookends[b].frames, plan.fps),
            })
        })
        .collect();
    Ok(Bookends(placed))
}

/// Export renders only on macOS, so elsewhere nothing is placed around the cut.
#[cfg(not(target_os = "macos"))]
pub fn bookends(_video_dir: &Path, _project: &Project) -> Result<Bookends, LockError> {
    Ok(Bookends::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spoken::tests::project;

    #[test]
    fn the_lock_cuts_the_timeline_of_a_migrated_video() {
        let dir = tempfile::tempdir().unwrap();
        let project = project(
            "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=10_000_000\n    }\n}\n",
        );
        let length = |t: &Timeline| crate::spoken::duration(t.main());
        assert_eq!(
            length(&timeline(dir.path(), &project).unwrap()).0,
            10_000_000
        );
        std::fs::write(
            dir.path().join(LOCK_FILE),
            "lock 1\n\ntake t01 {\n    cut from=2_000_000 to=3_000_000 on=silence t=2_000_000 layer=format rule=\"silence\"\n}\n",
        )
        .unwrap();
        assert_eq!(
            length(&timeline(dir.path(), &project).unwrap()).0,
            9_000_000
        );
    }
}
