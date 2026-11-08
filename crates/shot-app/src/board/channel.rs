//! Channel repos as the app uses them: find the channel of a video folder, start a channel, add
//! a video to one, get a card's folder ready to open, and move a video folder into a channel.
//!
//! The board reads videos from git (see `shot-board`), the studio edits a folder on disk at
//! `<repo>/videos/<slug>`. A new video gets both: its branch with the first commit and its
//! folder. A card whose folder is missing files, such as an idea that only has `video.kdl` on
//! the default branch, gets them from its branch tip before it opens.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use shot_board::Board;
use shot_core::{Cut, FormatKind, Lock, Status, Video};
use shot_ship::lock::LOCK_FILE;

/// The files of a video folder that live in git, apart from `elements/` and `sources/`.
const TRACKED: [&str; 5] = ["video.kdl", "script.md", "format.kdl", "cut.kdl", LOCK_FILE];

/// The channel repo and slug of a video folder at `<repo>/videos/<slug>`, if the repo opens as
/// a board.
pub fn locate(dir: &Path) -> Option<(PathBuf, String)> {
    let dir = std::path::absolute(dir).ok()?;
    let slug = dir.file_name()?.to_str()?.to_owned();
    let videos = dir.parent()?;
    if videos.file_name()? != "videos" {
        return None;
    }
    let repo = videos.parent()?.to_owned();
    Board::open(&repo).ok()?;
    Some((repo, slug))
}

/// True when `dir` is the root of a git repo, which opens as a channel board.
pub fn is_channel(dir: &Path) -> bool {
    Board::open(dir).is_ok()
}

/// The channel's name: its folder name.
pub fn name(repo: &Path) -> String {
    repo.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.display().to_string())
}

/// A slug from a title or folder name: `Why we ship, fast!` becomes `why-we-ship-fast`.
pub fn slug_from(text: &str) -> String {
    let mut slug = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "video".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Starts a channel repo in the new folder `dir`.
pub fn create(dir: &Path) -> Result<(), String> {
    Board::init(dir).map(drop).map_err(|e| e.to_string())
}

/// Adds an idea titled `title` to the channel, as a branch and a folder, and returns its slug.
/// A taken slug gets a number: `friday-deploys-2`.
pub fn new_video(repo: &Path, title: &str, format: FormatKind) -> Result<String, String> {
    let board = Board::open(repo).map_err(|e| e.to_string())?;
    let base = slug_from(title);
    let mut n = 1;
    let slug = loop {
        let slug = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let taken =
            board.has_video(&slug).map_err(|e| e.to_string())? || folder(repo, &slug).exists();
        if !taken {
            break slug;
        }
        n += 1;
    };
    let video = Video {
        title: title.trim().to_owned(),
        status: Status::Ideas,
        format,
        target_date: None,
    };
    let files = BTreeMap::from([
        ("video.kdl".to_owned(), video.to_kdl()),
        ("script.md".to_owned(), first_script(&video.title)),
        // A new video starts migrated, like `shot migrate` leaves one: nothing differs from the
        // channel's format yet and the lock of an empty cut is empty.
        ("format.kdl".to_owned(), String::new()),
        (LOCK_FILE.to_owned(), Lock::default().to_kdl()),
    ]);
    board.add_video(&slug, &files).map_err(|e| e.to_string())?;
    prepare(repo, &slug)?;
    Ok(slug)
}

/// Gets the folder of video `slug` ready to open in the studio and returns it. Files the folder
/// lacks come from the video's branch tip; an idea without a script gets one with its title,
/// and one without a format gets an empty layer and lock, as a new video does. Files already on
/// disk are never overwritten.
pub fn prepare(repo: &Path, slug: &str) -> Result<PathBuf, String> {
    let board = Board::open(repo).map_err(|e| e.to_string())?;
    let dir = folder(repo, slug);
    let write = |file: &str, text: &str| {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    };
    let from_branch = |file: &str| -> Result<(), String> {
        if dir.join(file).exists() {
            return Ok(());
        }
        // A folder nobody committed yet has no branch to fill it from.
        match board.read_file(slug, file) {
            Ok(Some(text)) => write(file, &text),
            Ok(None) | Err(shot_board::Error::UnknownVideo(_)) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    };
    for file in TRACKED {
        from_branch(file)?;
    }
    let read = |file: &str| fs::read_to_string(dir.join(file)).ok();
    if let Some(cut) = read("cut.kdl").and_then(|text| Cut::parse(&text).ok()) {
        for take in cut.chapters.iter().flat_map(|c| &c.takes) {
            from_branch(&format!("elements/{}.jsonl", take.take))?;
        }
    }
    let video = read("video.kdl")
        .ok_or_else(|| format!("videos/{slug}/video.kdl does not exist"))
        .and_then(|text| Video::parse(&text).map_err(|e| format!("videos/{slug}/video.kdl:{e}")))?;
    if !dir.join("script.md").exists() {
        write("script.md", &first_script(&video.title))?;
    }
    if !dir.join("format.kdl").exists() && !dir.join(LOCK_FILE).exists() {
        write("format.kdl", "")?;
        write(LOCK_FILE, &Lock::default().to_kdl())?;
    }
    Ok(dir)
}

/// Moves the video folder `dir` into the channel at `repo` as `videos/<slug>`, named after the
/// folder, and adds it to the board with its text files. Media and caches move along but stay
/// out of git. Returns the new folder.
pub fn move_into(dir: &Path, repo: &Path) -> Result<PathBuf, String> {
    let board = Board::open(repo).map_err(|_| {
        format!(
            "{} is not a channel: pick the root folder of a git repo",
            repo.display()
        )
    })?;
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let slug = slug_from(&name);
    let target = folder(repo, &slug);
    if board.has_video(&slug).map_err(|e| e.to_string())? || target.exists() {
        return Err(format!(
            "{} already has a video videos/{slug}. Rename this folder first.",
            self::name(repo)
        ));
    }
    let files = text_files(dir)?;
    fs::create_dir_all(repo.join("videos")).map_err(|e| e.to_string())?;
    fs::rename(dir, &target).map_err(|e| match e.kind() {
        std::io::ErrorKind::CrossesDevices => format!(
            "{} is on another disk than the channel. Move it into {} in Finder, then open it.",
            dir.display(),
            repo.join("videos").display()
        ),
        _ => format!("{}: {e}", dir.display()),
    })?;
    if let Err(error) = board.add_video(&slug, &files) {
        // Put the folder back, so a failed move leaves everything as it was.
        let _ = fs::rename(&target, dir);
        return Err(error.to_string());
    }
    Ok(target)
}

fn folder(repo: &Path, slug: &str) -> PathBuf {
    repo.join("videos").join(slug)
}

fn first_script(title: &str) -> String {
    format!("# {title}\n")
}

/// The text files of a video folder that belong in git, by path relative to the folder:
/// everything except `media/`, `.shot/` and hidden files. Files that are not UTF-8 are left out.
fn text_files(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) -> Result<(), String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || (dir == root && name == "media") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, files)?;
            } else if let Ok(text) = fs::read_to_string(&path) {
                let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
                let parts: Vec<_> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                files.insert(parts.join("/"), text);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files)?;
    if !files.contains_key("video.kdl") {
        return Err(format!("{}: not a video folder", dir.display()));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::VideoFolder;

    #[test]
    fn slugs_are_lowercase_words_joined_by_dashes() {
        assert_eq!(slug_from("Why we ship, fast!"), "why-we-ship-fast");
        assert_eq!(slug_from("  gpui in 10 minutes "), "gpui-in-10-minutes");
        assert_eq!(slug_from("Ünïcode"), "n-code");
        assert_eq!(slug_from("!!!"), "video");
    }

    #[test]
    fn a_new_video_is_a_card_and_a_folder_that_opens() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("cantina");
        create(&repo).unwrap();
        let slug = new_video(&repo, "Friday deploys", FormatKind::Opinion).unwrap();
        assert_eq!(slug, "friday-deploys");
        let again = new_video(&repo, "Friday deploys", FormatKind::Opinion).unwrap();
        assert_eq!(again, "friday-deploys-2");

        let cards = Board::open(&repo).unwrap().cards().unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].video.as_ref().unwrap().status, Status::Ideas);
        let folder = VideoFolder::open(&repo.join("videos/friday-deploys")).unwrap();
        assert_eq!(folder.project.video.title, "Friday deploys");
        assert_eq!(folder.project.video.format, FormatKind::Opinion);
        assert_eq!(
            locate(&repo.join("videos/friday-deploys")),
            Some((repo.clone(), slug))
        );
    }

    #[test]
    fn an_idea_with_only_its_card_gets_a_folder_that_opens() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("cantina");
        create(&repo).unwrap();
        let card = "title \"An idea\"\nstatus ideas\nformat tutorial\n";
        Board::open(&repo)
            .unwrap()
            .add_video(
                "idea",
                &BTreeMap::from([("video.kdl".to_owned(), card.to_owned())]),
            )
            .unwrap();

        let dir = prepare(&repo, "idea").unwrap();
        assert_eq!(fs::read_to_string(dir.join("video.kdl")).unwrap(), card);
        let folder = VideoFolder::open(&dir).unwrap();
        assert_eq!(folder.project.script.title, "An idea");
        assert!(prepare(&repo, "missing").is_err());

        let uncommitted = repo.join("videos/uncommitted");
        crate::welcome::create(&uncommitted, FormatKind::Opinion).unwrap();
        assert_eq!(prepare(&repo, "uncommitted").unwrap(), uncommitted);
    }

    #[test]
    fn moving_a_video_into_a_channel_puts_it_on_the_board() {
        let parent = tempfile::tempdir().unwrap();
        let repo = parent.path().join("cantina");
        create(&repo).unwrap();
        let loose = parent.path().join("Loose Video");
        crate::welcome::create(&loose, FormatKind::Tutorial).unwrap();
        fs::create_dir_all(loose.join("media/t01")).unwrap();
        fs::write(loose.join("media/t01/events.jsonl"), "{}\n").unwrap();

        let moved = move_into(&loose, &repo).unwrap();
        assert_eq!(moved, repo.join("videos/loose-video"));
        assert!(!loose.exists());
        assert!(moved.join("media/t01/events.jsonl").exists());
        let board = Board::open(&repo).unwrap();
        assert!(
            board
                .read_file("loose-video", "script.md")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            board
                .read_file("loose-video", "media/t01/events.jsonl")
                .unwrap(),
            None,
            "media stays out of git"
        );
        VideoFolder::open(&moved).unwrap();

        let other = parent.path().join("loose-video");
        crate::welcome::create(&other, FormatKind::Tutorial).unwrap();
        assert!(move_into(&other, &repo).is_err(), "a taken slug is refused");
        assert!(other.exists());
    }
}
