//! A video's `cut.lock` as the ship tools read it, so chapter times, thumbnails and the
//! ship plan follow the frames export renders. The lock is read, never rebuilt here: `shot`
//! rebuilds it before shipping, the app on every edit.

use std::io;
use std::path::{Path, PathBuf};

use shot_core::{Lock, ParseError, Project, Timeline};

use crate::spoken::Bookends;

/// The lock's file name in the video folder.
pub const LOCK_FILE: &str = "cut.lock";

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("{}: missing, open the video in Shot Studio or run `shot style build` to write it", .0.display())]
    Missing(PathBuf),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("cut.lock: {0}")]
    Parse(#[from] ParseError),
    /// Placing the brand's intro and outro failed, for example on a missing bumper.
    #[error("{0}")]
    Bookends(String),
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

/// The timeline export renders, styled by the video's lock.
pub fn timeline(video_dir: &Path, project: &Project) -> Result<Timeline, LockError> {
    Ok(Timeline::evaluate(project, &read(video_dir)?))
}

/// The main cut's intro and outro where export places them, for chapter times: none when its
/// brand has neither.
pub fn bookends(video_dir: &Path, project: &Project) -> Result<Bookends, LockError> {
    bookends_with(video_dir, project, &read(video_dir)?)
}

/// [`bookends`] of the cut styled by `lock` instead of the folder's `cut.lock` as it is now.
#[cfg(target_os = "macos")]
pub fn bookends_with(
    video_dir: &Path,
    project: &Project,
    lock: &Lock,
) -> Result<Bookends, LockError> {
    use shot_render::join::Piece;
    let has_bookends = lock.brand.intro.is_some() || lock.brand.outro.is_some();
    if !has_bookends {
        return Ok(Bookends::default());
    }
    let plan = shot_render::export::preview_plan_with(video_dir, project, lock)
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
pub fn bookends_with(
    _video_dir: &Path,
    _project: &Project,
    _lock: &Lock,
) -> Result<Bookends, LockError> {
    Ok(Bookends::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spoken::tests::project;

    #[test]
    fn the_lock_cuts_the_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let project = project(
            "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=10_000_000\n    }\n}\n",
        );
        let length = |t: &Timeline| crate::spoken::duration(t.main());
        assert!(matches!(
            timeline(dir.path(), &project),
            Err(LockError::Missing(_))
        ));
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

    #[test]
    fn a_plan_follows_the_lock_read_with_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let project = project(
            "chapter \"Hook\" {\n    take t01 {\n        keep from=0 to=10_000_000\n    }\n}\n",
        );
        let with_project = Lock::parse("lock 1\n").unwrap();
        // `cut.lock` is written again after the run read its files: it cuts a second.
        std::fs::write(
            dir.path().join(LOCK_FILE),
            "lock 1\n\ntake t01 {\n    cut from=2_000_000 to=3_000_000 on=silence t=2_000_000 layer=format rule=\"silence\"\n}\n",
        )
        .unwrap();
        let transcripts = std::collections::HashMap::new();
        let options = crate::ship::Options::default();
        let cuts =
            crate::ship::plan(dir.path(), &project, &with_project, &transcripts, options).unwrap();
        assert_eq!(cuts[0].length.0, 10_000_000);
    }
}
