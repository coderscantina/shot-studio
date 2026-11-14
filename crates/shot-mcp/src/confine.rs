//! Keeps every path a tool takes inside the channel repo or the video folder.
//!
//! Git reads go through `shot-board`, which only accepts boring slugs and relative paths. The
//! working tree reads (the video's files, cached transcripts, thumbnail stills) and every path
//! an agent names go through here: no absolute paths, no `..`, and symlinks are resolved before
//! the check.

use std::io;
use std::path::{Component, Path, PathBuf};

/// A slug ends up in paths and refs: lowercase letters, digits and dashes, not starting with one.
pub fn slug(slug: &str) -> Result<(), String> {
    let valid = !slug.is_empty()
        && !slug.starts_with('-')
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!("`{slug}` is not a valid name"))
    }
}

/// A relative path that names something below its folder.
pub fn relative(path: &str) -> Result<&Path, String> {
    let plain = !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if plain {
        Ok(Path::new(path))
    } else {
        Err(format!(
            "`{path}` is not a relative path inside the video folder"
        ))
    }
}

/// The real location of `rel` (empty for the folder itself) in `folder`, the video folder of
/// `slug`: the main checkout's `videos/<slug>` or the one in the video's worktree. A path that
/// does not exist yet comes back as spelled, so the caller's read reports it missing. A path
/// that exists but resolves outside the video folder, or a video folder that resolves outside
/// the repo, is an error. A worktree's `media/` and `.shot/` link to the main checkout's video
/// folder, so paths there count as inside.
pub fn resolve(repo: &Path, slug_name: &str, folder: &Path, rel: &str) -> Result<PathBuf, String> {
    slug(slug_name)?;
    let mut path = folder.to_owned();
    if !rel.is_empty() {
        path.push(relative(rel)?);
    }
    let real = match path.canonicalize() {
        Ok(real) => real,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(path),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let outside = || format!("`{rel}` of {slug_name} resolves outside its video folder");
    let root = repo
        .canonicalize()
        .map_err(|e| format!("{}: {e}", repo.display()))?;
    let folder = folder
        .canonicalize()
        .map_err(|e| format!("{slug_name}: {e}"))?;
    let main = repo.join("videos").join(slug_name).canonicalize().ok();
    let inside = real.starts_with(&folder) || main.is_some_and(|main| real.starts_with(main));
    if !folder.starts_with(&root) || !inside {
        return Err(outside());
    }
    Ok(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_below_their_folder() {
        for ok in ["script.md", "sources/a.md", "a/b/c.txt"] {
            assert!(relative(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "./a",
            "a\\b",
            "a/..",
        ] {
            assert!(relative(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn slugs_are_boring() {
        assert!(slug("why-we-deploy").is_ok());
        for bad in ["", "-x", "../x", "a/b", "A", "/abs"] {
            assert!(slug(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn symlinks_cannot_leave_the_video_folder() {
        use std::os::unix::fs::symlink;
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        let video = repo.path().join("videos/alpha");
        std::fs::create_dir_all(video.join(".shot")).unwrap();
        std::fs::write(video.join("inside"), "x").unwrap();
        symlink(outside.path(), video.join(".shot/cache")).unwrap();
        symlink(outside.path().join("secret"), video.join("link")).unwrap();
        symlink(outside.path(), repo.path().join("videos/beta")).unwrap();

        let resolve = |slug: &str, rel: &str| {
            resolve(
                repo.path(),
                slug,
                &repo.path().join("videos").join(slug),
                rel,
            )
        };
        assert!(resolve("alpha", "inside").is_ok());
        assert!(resolve("alpha", "missing").is_ok());
        assert!(resolve("alpha", "").is_ok());
        assert!(resolve("alpha", "link").is_err());
        assert!(resolve("alpha", ".shot/cache/secret").is_err());
        assert!(resolve("beta", "secret").is_err());
        assert!(resolve("alpha", "../beta/secret").is_err());
    }
}
