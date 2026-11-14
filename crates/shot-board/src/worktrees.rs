//! A worktree per video branch ([0037]).
//!
//! Opening a video checks out `video/<slug>` at `.shot/worktrees/<slug>` of the channel repo.
//! Edits stay uncommitted there until someone commits them. The video folder's git-ignored
//! folders stay in the main checkout and the worktree links them, so removing a worktree never
//! loses a take.
//!
//! [0037]: ../../../docs/decisions/0037-a-worktree-per-video-branch.md

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use git2::{
    BranchType, Oid, Repository, Status, StatusOptions, Worktree, WorktreeAddOptions,
    WorktreePruneOptions, build::CheckoutBuilder, build::TreeUpdateBuilder,
};

use crate::{Board, Error, Result, branch_name, check_name, proposals, video_dir, video_path};

/// Where video worktrees live, relative to the main checkout.
const WORKTREES: &str = ".shot/worktrees";
/// The git-ignored folders of a video folder (see the contract's channel repo layout). They stay
/// in the main checkout's video folder and each worktree links them.
const LINKED: [&str; 2] = ["media", ".shot"];
/// Ignore rules the worktrees need even in a repo whose `.gitignore` lacks them. `media/` in a
/// `.gitignore` matches only folders, and the links are not folders to git.
const EXCLUDE: [&str; 3] = ["/.shot/worktrees/", "/videos/*/media", "/videos/*/.shot"];

impl Board {
    /// The video folder of `slug` in its worktree, checking out `video/<slug>` at
    /// `.shot/worktrees/<slug>` on first use and reusing it after. An idea without a branch gets
    /// one off the default branch. The video's ignored folders are linked from the main checkout.
    pub fn open_video(&self, slug: &str) -> Result<PathBuf> {
        let tip = self.tip(slug)?.id();
        let branch = branch_name(slug);
        if self.repo.find_branch(&branch, BranchType::Local).is_err() {
            self.set_branch(&branch, tip)?;
        }
        let main = self.workdir()?;
        let path = main.join(WORKTREES).join(slug);
        if self.video_worktree(slug).is_none() {
            if let Ok(gone) = self.repo.find_worktree(slug) {
                // Registered but its folder was deleted: forget it so it can be added again.
                gone.prune(None)?;
            }
            self.write_excludes()?;
            std::fs::create_dir_all(main.join(WORKTREES)).map_err(io)?;
            let reference = self.repo.find_reference(&format!("refs/heads/{branch}"))?;
            self.repo.worktree(
                slug,
                &path,
                Some(WorktreeAddOptions::new().reference(Some(&reference))),
            )?;
        }
        let folder = path.join(video_dir(slug));
        for name in LINKED {
            let link = folder.join(name);
            if link.symlink_metadata().is_ok() {
                continue;
            }
            let target = main.join(video_dir(slug)).join(name);
            std::fs::create_dir_all(&target).map_err(io)?;
            std::fs::create_dir_all(&folder).map_err(io)?;
            std::os::unix::fs::symlink(&target, &link).map_err(io)?;
        }
        Ok(folder)
    }

    /// Commits `files` (relative to the video folder) from the video's worktree on its branch,
    /// as they are on disk. A file missing on disk is deleted. Other uncommitted edits stay
    /// uncommitted. Returns the new commit sha, or `None` when the files are already committed.
    pub fn commit_video_files(
        &self,
        slug: &str,
        files: &[&str],
        subject: &str,
    ) -> Result<Option<String>> {
        check_name(slug)?;
        if let Some(file) = files.iter().find(|f| !proposals::is_inside(f)) {
            return Err(Error::InvalidPath((*file).to_owned()));
        }
        let worktree = self
            .video_worktree(slug)
            .ok_or_else(|| Error::NoWorktree(slug.to_owned()))?;
        let repo = Repository::open_from_worktree(&worktree)?;
        let work = repo.workdir().ok_or(Error::Bare)?.to_owned();
        let head = repo.head()?.peel_to_commit()?;
        let head_tree = head.tree()?;

        let mut update = TreeUpdateBuilder::new();
        let mut present = Vec::with_capacity(files.len());
        for file in files {
            let path = video_path(slug, file);
            let on_disk = work.join(&path);
            if on_disk.is_file() {
                update.upsert(&path, repo.blob_path(&on_disk)?, git2::FileMode::Blob);
                present.push((path, true));
            } else if head_tree.get_path(Path::new(&path)).is_ok() {
                update.remove(&path);
                present.push((path, false));
            }
        }
        let tree = update.create_updated(&repo, &head_tree)?;
        if tree == head.tree_id() {
            return Ok(None);
        }
        let sig = self.signature();
        let tree = repo.find_tree(tree)?;
        let sha = repo.commit(Some("HEAD"), &sig, &sig, subject, &tree, &[&head])?;

        let mut index = repo.index()?;
        for (path, exists) in &present {
            if *exists {
                index.add_path(Path::new(path))?;
            } else {
                index.remove_path(Path::new(path))?;
            }
        }
        index.write()?;
        Ok(Some(sha.to_string()))
    }

    /// Removes the worktree of every video whose branch is merged into the default branch or
    /// deleted, unless it has uncommitted changes. The media it links stays in the main
    /// checkout. Returns the slugs whose worktree was removed. [`Board::open`] runs this.
    pub fn remove_stale_worktrees(&self) -> Result<Vec<String>> {
        let names = self.repo.worktrees()?;
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let default = self.default_tip().ok().map(|c| c.id());
        let mut removed = Vec::new();
        for slug in names.iter().filter_map(|name| name.ok().flatten()) {
            let Some(worktree) = self.video_worktree(slug) else {
                continue;
            };
            let stale = match self.repo.find_branch(&branch_name(slug), BranchType::Local) {
                Err(_) => true,
                Ok(branch) => {
                    let tip = branch.get().peel_to_commit()?.id();
                    match default {
                        Some(default) => self.repo.graph_descendant_of(default, tip)?,
                        None => false,
                    }
                }
            };
            let locked = !matches!(worktree.is_locked()?, git2::WorktreeLockStatus::Unlocked);
            if !stale || locked {
                continue;
            }
            let repo = Repository::open_from_worktree(&worktree)?;
            if !uncommitted(&repo)?.is_empty() {
                continue;
            }
            drop(repo);
            // Unlink first, so removing the working tree can never reach into the media.
            let folder = worktree.path().join(video_dir(slug));
            for name in LINKED {
                let link = folder.join(name);
                if link.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
                    std::fs::remove_file(&link).map_err(io)?;
                }
            }
            worktree.prune(Some(
                WorktreePruneOptions::new().valid(true).working_tree(true),
            ))?;
            removed.push(slug.to_owned());
        }
        Ok(removed)
    }

    /// The video folder in the worktree of `slug`, if it has one.
    pub(crate) fn worktree_folder(&self, slug: &str) -> Option<PathBuf> {
        let worktree = self.video_worktree(slug)?;
        Some(worktree.path().join(video_dir(slug)))
    }

    /// Moves `video/<slug>` to `new`. When the video has a worktree, the files that change are
    /// checked out there first, and a change to a file with uncommitted edits is refused with
    /// [`Error::Uncommitted`] before anything moves.
    pub(crate) fn advance_video(&self, slug: &str, new: Oid) -> Result<()> {
        if let Some(worktree) = self.video_worktree(slug) {
            let repo = Repository::open_from_worktree(&worktree)?;
            let old = repo.head()?.peel_to_tree()?;
            let new_tree = repo.find_commit(new)?.tree()?;
            let diff = repo.diff_tree_to_tree(Some(&old), Some(&new_tree), None)?;
            let changed: BTreeSet<String> = diff
                .deltas()
                .flat_map(|d| [d.old_file().path(), d.new_file().path()])
                .flatten()
                .filter_map(|p| p.to_str().map(str::to_owned))
                .collect();
            let paths: Vec<String> = uncommitted(&repo)?
                .into_iter()
                .filter(|p| changed.contains(p))
                .collect();
            if !paths.is_empty() {
                return Err(Error::Uncommitted {
                    slug: slug.to_owned(),
                    paths,
                });
            }
            repo.checkout_tree(new_tree.as_object(), Some(CheckoutBuilder::new().safe()))?;
        }
        self.set_branch(&branch_name(slug), new)
    }

    /// The worktree of `slug`, only if it is valid and sits where this board puts it, so a
    /// person's own worktree of the same name is never touched.
    fn video_worktree(&self, slug: &str) -> Option<Worktree> {
        check_name(slug).ok()?;
        let worktree = self.repo.find_worktree(slug).ok()?;
        worktree.validate().ok()?;
        let ours = self.workdir().ok()?.join(WORKTREES).join(slug);
        (worktree.path().canonicalize().ok()? == ours.canonicalize().ok()?).then_some(worktree)
    }

    fn workdir(&self) -> Result<&Path> {
        self.repo.workdir().ok_or(Error::Bare)
    }

    /// Adds the [`EXCLUDE`] rules to the repo's local `info/exclude`, once.
    fn write_excludes(&self) -> Result<()> {
        let file = self.repo.path().join("info/exclude");
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        let missing: Vec<&str> = EXCLUDE
            .into_iter()
            .filter(|rule| !text.lines().any(|line| line == *rule))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for rule in missing {
            text.push_str(rule);
            text.push('\n');
        }
        std::fs::create_dir_all(file.parent().expect("info/exclude has a parent")).map_err(io)?;
        std::fs::write(&file, text).map_err(io)
    }
}

/// Repo paths with uncommitted changes in a worktree: staged, modified, deleted or new. When its
/// branch is gone there is no commit to compare with, so only changes against the index count.
fn uncommitted(repo: &Repository) -> Result<Vec<String>> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let mask = if repo.head().is_ok() {
        Status::all() - Status::IGNORED
    } else {
        Status::WT_NEW
            | Status::WT_MODIFIED
            | Status::WT_DELETED
            | Status::WT_TYPECHANGE
            | Status::WT_RENAMED
    };
    Ok(repo
        .statuses(Some(&mut options))?
        .iter()
        .filter(|entry| entry.status().intersects(mask))
        .filter_map(|entry| entry.path().ok().map(str::to_owned))
        .collect())
}

fn io(error: std::io::Error) -> Error {
    Error::Io(error.to_string())
}
