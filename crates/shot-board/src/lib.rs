//! Git-backed board: video.kdl, status commits, tags and proposals.
//!
//! The board is a channel repo (see `docs/contracts/project-files.md`) and nothing else. There is
//! no database and no cache: [`Board::open`] on a fresh process rebuilds the same board.
//!
//! # Branches and refs
//!
//! - `video/<slug>`: the branch of one video. Every change to the video lands here as a commit,
//!   so `git log video/<slug>` is that video's activity. An idea without a branch yet lives only
//!   on the default branch (`main`, then `master`, then the checked out branch); the first move or
//!   accepted proposal creates its branch off the default branch tip.
//! - `proposal/<slug>/<name>`: an agent proposal off the video's branch ([0004]).
//! - `vYY.M.D`: annotated CalVer release tag on a shipped video's commit, for example `v26.11.7`.
//!   A second release on the same day is `v26.11.7.2`.
//!
//! `<slug>` and `<name>` use lowercase ASCII letters, digits and dashes, not starting with a dash.
//!
//! # Working tree
//!
//! All reads and writes go through refs, trees and blobs. The working tree and the index of the
//! checked out branch are never touched, so a user editing in the repo is not disturbed. The
//! flip side: if the branch being updated is the checked out one, its working tree becomes stale
//! until the user runs `git reset --hard` or similar. The app keeps the working tree on the
//! default branch.
//!
//! [0004]: ../../docs/decisions/0004-proposals-as-branches.md

mod error;
mod proposals;
mod releases;

use std::collections::BTreeSet;
use std::path::Path;

use git2::{BranchType, Commit, Oid, Repository, Signature, build::TreeUpdateBuilder};
use shot_core::{Status, Video};

pub use error::{CardError, Error, Result};
pub use proposals::{Accepted, FileDiff, Proposal};
pub use releases::ReleaseTag;

const VIDEO_PREFIX: &str = "video/";
const FALLBACK_NAME: &str = "Shot Studio";
const FALLBACK_EMAIL: &str = "shot@localhost";

/// One card on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub slug: String,
    /// False for an idea that still only exists on the default branch.
    pub has_branch: bool,
    /// The parsed `video.kdl`, or why it could not be read. One bad card never hides the others.
    pub video: Result<Video, CardError>,
}

/// One commit on a video branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub slug: String,
    pub sha: String,
    /// Commit time in seconds since the Unix epoch.
    pub time: i64,
    pub subject: String,
    pub author: String,
}

/// A channel repo opened as a board.
pub struct Board {
    repo: Repository,
}

impl Board {
    /// Opens the repo at `path`. It must be the repo root, parents are not searched.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            repo: Repository::open(path)?,
        })
    }

    /// All videos, sorted by slug: every folder under `videos/` on the default branch plus every
    /// `video/<slug>` branch. Each card is read from its branch tip, else from the default branch.
    pub fn cards(&self) -> Result<Vec<Card>> {
        let default = self.default_tip()?;
        let mut slugs = BTreeSet::new();
        if let Ok(videos) = default
            .tree()?
            .get_path("videos".as_ref())
            .and_then(|e| e.to_object(&self.repo))
            .and_then(|o| o.peel_to_tree())
        {
            slugs.extend(
                videos
                    .iter()
                    .filter_map(|e| Some(e.name().ok()?.to_owned())),
            );
        }
        slugs.extend(self.branch_slugs()?);

        slugs
            .into_iter()
            .map(|slug| {
                let branch = self
                    .repo
                    .find_branch(&branch_name(&slug), BranchType::Local);
                let has_branch = branch.is_ok();
                let tip = match branch {
                    Ok(branch) => branch.get().peel_to_commit()?,
                    Err(_) => default.clone(),
                };
                let video = self.read_video(&tip, &slug);
                Ok(Card {
                    slug,
                    has_branch,
                    video,
                })
            })
            .collect()
    }

    /// Moves a card by rewriting `status` through shot-core and committing on the video's
    /// branch, creating the branch for an idea that has none. Returns the new commit sha, or
    /// `None` when the card already has that status (no empty commits).
    pub fn move_card(&self, slug: &str, status: Status) -> Result<Option<String>> {
        let tip = self.tip(slug)?;
        let mut video = self.read_video(&tip, slug)?;
        if video.status == status {
            return Ok(None);
        }
        video.status = status;
        let tree = self.write_files(&tip, [(video_path(slug, "video.kdl"), video.to_kdl())])?;
        let sha = self.commit(&tip, tree, &format!("🚚 move {slug} to {status}"))?;
        self.set_branch(&branch_name(slug), sha)?;
        Ok(Some(sha.to_string()))
    }

    /// The newest commits across all video branches, newest first, at most `limit`. Commits that
    /// are also on the default branch are history of the channel, not of a video, and left out.
    pub fn activity(&self, limit: usize) -> Result<Vec<Activity>> {
        let default = self.default_tip()?.id();
        let mut entries = Vec::new();
        for slug in self.branch_slugs()? {
            let branch = self
                .repo
                .find_branch(&branch_name(&slug), BranchType::Local)?;
            let mut walk = self.repo.revwalk()?;
            walk.set_sorting(git2::Sort::TIME)?;
            walk.push(branch.get().peel_to_commit()?.id())?;
            walk.hide(default)?;
            for oid in walk.take(limit) {
                let commit = self.repo.find_commit(oid?)?;
                entries.push(Activity {
                    slug: slug.clone(),
                    sha: commit.id().to_string(),
                    time: commit.time().seconds(),
                    subject: commit
                        .summary()
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                        .to_owned(),
                    author: commit.author().name().unwrap_or_default().to_owned(),
                });
            }
        }
        entries.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| a.sha.cmp(&b.sha)));
        entries.truncate(limit);
        Ok(entries)
    }

    fn signature(&self) -> Signature<'static> {
        self.repo.signature().unwrap_or_else(|_| {
            Signature::now(FALLBACK_NAME, FALLBACK_EMAIL).expect("fallback identity is valid")
        })
    }

    fn default_tip(&self) -> Result<Commit<'_>> {
        for name in ["main", "master"] {
            if let Ok(branch) = self.repo.find_branch(name, BranchType::Local) {
                return Ok(branch.get().peel_to_commit()?);
            }
        }
        let head = self.repo.head().map_err(|_| Error::NoDefaultBranch)?;
        let managed = head
            .name()
            .is_ok_and(|n| n.contains("/video/") || n.contains("/proposal/"));
        match head.peel_to_commit() {
            Ok(commit) if !managed => Ok(commit),
            _ => Err(Error::NoDefaultBranch),
        }
    }

    /// The text of `file` (relative to the video folder) at the video's tip, `None` if it has no
    /// such file yet.
    pub fn read_file(&self, slug: &str, file: &str) -> Result<Option<String>> {
        if !proposals::is_inside(file) {
            return Err(Error::InvalidPath(file.to_owned()));
        }
        let tip = self.tip(slug)?;
        let Ok(entry) = tip.tree()?.get_path(Path::new(&video_path(slug, file))) else {
            return Ok(None);
        };
        let blob = self.repo.find_blob(entry.id())?;
        Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
    }

    /// The bytes of `path` (relative to the repo root, for example `brand/brand.kdl`) in the
    /// tree the video is read from, `None` if there is no such file.
    pub fn read_repo_file(&self, slug: &str, path: &str) -> Result<Option<Vec<u8>>> {
        if !proposals::is_inside(path) {
            return Err(Error::InvalidPath(path.to_owned()));
        }
        let tip = self.tip(slug)?;
        let Ok(entry) = tip.tree()?.get_path(Path::new(path)) else {
            return Ok(None);
        };
        Ok(Some(self.repo.find_blob(entry.id())?.content().to_vec()))
    }

    /// The commit a video is read from: its branch tip, else the default branch if it has the
    /// video folder.
    fn tip(&self, slug: &str) -> Result<Commit<'_>> {
        check_name(slug)?;
        if let Ok(branch) = self.repo.find_branch(&branch_name(slug), BranchType::Local) {
            return Ok(branch.get().peel_to_commit()?);
        }
        let default = self.default_tip()?;
        if default
            .tree()?
            .get_path(Path::new(&video_dir(slug)))
            .is_ok()
        {
            Ok(default)
        } else {
            Err(Error::UnknownVideo(slug.to_owned()))
        }
    }

    fn branch_slugs(&self) -> Result<Vec<String>> {
        let mut slugs = Vec::new();
        for branch in self.repo.branches(Some(BranchType::Local))? {
            let (branch, _) = branch?;
            if let Some(slug) = branch.name()?.and_then(|n| n.strip_prefix(VIDEO_PREFIX)) {
                slugs.push(slug.to_owned());
            }
        }
        Ok(slugs)
    }

    fn read_video(&self, commit: &Commit<'_>, slug: &str) -> Result<Video, CardError> {
        let missing = || CardError::Missing {
            slug: slug.to_owned(),
        };
        let tree = commit.tree().map_err(|_| missing())?;
        let entry = tree
            .get_path(Path::new(&video_path(slug, "video.kdl")))
            .map_err(|_| missing())?;
        let blob = self.repo.find_blob(entry.id()).map_err(|_| missing())?;
        Video::parse(&String::from_utf8_lossy(blob.content())).map_err(|error| CardError::Parse {
            slug: slug.to_owned(),
            error,
        })
    }

    /// Builds the tree of `base` with the given repo-relative files replaced or added.
    fn write_files(
        &self,
        base: &Commit<'_>,
        files: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Oid> {
        let mut update = TreeUpdateBuilder::new();
        for (path, text) in files {
            let blob = self.repo.blob(text.as_bytes())?;
            update.upsert(path, blob, git2::FileMode::Blob);
        }
        Ok(update.create_updated(&self.repo, &base.tree()?)?)
    }

    fn commit(&self, parent: &Commit<'_>, tree: Oid, subject: &str) -> Result<Oid> {
        self.commit_with_parents(&[parent], tree, subject)
    }

    fn commit_with_parents(
        &self,
        parents: &[&Commit<'_>],
        tree: Oid,
        subject: &str,
    ) -> Result<Oid> {
        let sig = self.signature();
        let tree = self.repo.find_tree(tree)?;
        Ok(self
            .repo
            .commit(None, &sig, &sig, subject, &tree, parents)?)
    }

    fn set_branch(&self, name: &str, commit: Oid) -> Result<()> {
        self.repo
            .reference(&format!("refs/heads/{name}"), commit, true, "shot-board")?;
        Ok(())
    }
}

fn branch_name(slug: &str) -> String {
    format!("{VIDEO_PREFIX}{slug}")
}

fn video_dir(slug: &str) -> String {
    format!("videos/{slug}")
}

fn video_path(slug: &str, file: &str) -> String {
    format!("videos/{slug}/{file}")
}

/// Slugs and proposal names end up in ref names and paths, so they stay boring.
fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_owned()))
    }
}
