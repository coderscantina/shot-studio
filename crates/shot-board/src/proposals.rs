//! Agent proposals as branches `proposal/<slug>/<name>` ([0004]).
//!
//! File paths in and out are relative to the video folder (`cut.kdl`, `sources/note.md`).
//!
//! [0004]: ../../../docs/decisions/0004-proposals-as-branches.md

use std::collections::BTreeMap;
use std::path::{Component, Path};

use git2::{BranchType, Commit, Delta, Diff, Patch};

use crate::{Board, Error, Result, branch_name, check_name, video_dir, video_path};

const PROPOSAL_PREFIX: &str = "proposal/";
/// Where export renders a proposal's preview, inside the video folder, one folder per name.
const PROPOSAL_CACHE: &str = ".shot/cache/proposals";

/// A proposal waiting for review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub slug: String,
    pub name: String,
    /// The commit the proposal branch points at.
    pub head: String,
    /// The first line of the head commit.
    pub subject: String,
}

/// The unified diff of one file of a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub patch: String,
}

/// The result of accepting a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// The new tip of the video branch.
    pub head: String,
    /// False when the video branch had moved on and a merge commit was needed.
    pub fast_forward: bool,
}

impl Board {
    /// Creates `proposal/<slug>/<name>` off the video's branch (or the default branch for an idea
    /// without one) with one commit that writes `files`, a map of path to new contents.
    pub fn create_proposal(
        &self,
        slug: &str,
        name: &str,
        subject: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Proposal> {
        let mut changes = Vec::with_capacity(files.len());
        for (path, text) in files {
            if !is_inside(path) {
                return Err(Error::InvalidPath(path.clone()));
            }
            changes.push((video_path(slug, path), text.clone()));
        }
        self.propose(slug, name, subject, self.tip(slug)?, changes)
    }

    /// Like [`Board::create_proposal`], with paths relative to the repo root, so a proposal can
    /// change the channel's `brand/` and `formats/`. Channel files live on the default branch:
    /// a proposal of only those branches off it and accepting it merges into it, so every
    /// video sees the change. A proposal of the video's files branches off the video. One
    /// proposal does not mix the two.
    pub fn create_repo_proposal(
        &self,
        slug: &str,
        name: &str,
        subject: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Proposal> {
        if let Some(path) = files.keys().find(|p| !is_inside(p)) {
            return Err(Error::InvalidPath(path.clone()));
        }
        let channel = files.keys().filter(|p| is_channel_path(p)).count();
        let base = match channel {
            0 => self.tip(slug)?,
            n if n == files.len() => {
                self.tip(slug)?;
                self.default_tip()?
            }
            _ => return Err(Error::MixedProposal),
        };
        let changes = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
        self.propose(slug, name, subject, base, changes)
    }

    fn propose(
        &self,
        slug: &str,
        name: &str,
        subject: &str,
        base: Commit<'_>,
        changes: Vec<(String, String)>,
    ) -> Result<Proposal> {
        check_name(name)?;
        let branch = proposal_branch(slug, name);
        if self.repo.find_branch(&branch, BranchType::Local).is_ok() {
            return Err(Error::ProposalExists {
                slug: slug.to_owned(),
                name: name.to_owned(),
            });
        }
        let tree = self.write_files(&base, changes)?;
        if tree == base.tree_id() {
            return Err(Error::EmptyProposal);
        }
        let head = self.commit(&base, tree, subject)?;
        self.set_branch(&branch, head)?;
        Ok(Proposal {
            slug: slug.to_owned(),
            name: name.to_owned(),
            head: head.to_string(),
            subject: subject.lines().next().unwrap_or_default().to_owned(),
        })
    }

    /// Open proposals, optionally of one video only, sorted by slug and name.
    pub fn proposals(&self, slug: Option<&str>) -> Result<Vec<Proposal>> {
        let mut found = Vec::new();
        for branch in self.repo.branches(Some(BranchType::Local))? {
            let (branch, _) = branch?;
            let Some((video, name)) = branch
                .name()?
                .and_then(|n| n.strip_prefix(PROPOSAL_PREFIX))
                .and_then(|n| n.split_once('/'))
            else {
                continue;
            };
            if slug.is_some_and(|s| s != video) {
                continue;
            }
            let head = branch.get().peel_to_commit()?;
            found.push(Proposal {
                slug: video.to_owned(),
                name: name.to_owned(),
                head: head.id().to_string(),
                subject: head.summary().ok().flatten().unwrap_or_default().to_owned(),
            });
        }
        found.sort_by(|a, b| (&a.slug, &a.name).cmp(&(&b.slug, &b.name)));
        Ok(found)
    }

    /// What the proposal changes compared to where it branched off the video, as one unified
    /// diff per file. Changes made on the video branch since then are not shown.
    pub fn proposal_diff(&self, slug: &str, name: &str) -> Result<Vec<FileDiff>> {
        let head = self.proposal_head(slug, name)?;
        let (_, tip) = self.target(slug, &head)?;
        let base = self
            .repo
            .find_commit(self.repo.merge_base(tip.id(), head.id())?)?;
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&base.tree()?), Some(&head.tree()?), None)?;
        file_diffs(&diff, &video_dir(slug))
    }

    /// The text of `file` (relative to the video folder) where the proposal branched off the
    /// video and at its head, each `None` where the file does not exist. Read together they
    /// say what the proposal changes, independent of what the video branch did since.
    pub fn proposal_file(
        &self,
        slug: &str,
        name: &str,
        file: &str,
    ) -> Result<(Option<String>, Option<String>)> {
        if !is_inside(file) {
            return Err(Error::InvalidPath(file.to_owned()));
        }
        let head = self.proposal_head(slug, name)?;
        let (_, tip) = self.target(slug, &head)?;
        let base = self
            .repo
            .find_commit(self.repo.merge_base(tip.id(), head.id())?)?;
        let path = video_path(slug, file);
        let read = |commit: &Commit<'_>| -> Result<Option<String>> {
            let Ok(entry) = commit.tree()?.get_path(Path::new(&path)) else {
                return Ok(None);
            };
            let blob = self.repo.find_blob(entry.id())?;
            Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
        };
        Ok((read(&base)?, read(&head)?))
    }

    /// Every file the proposal writes since it branched, by path from the repo root, with its
    /// text at the proposal's head. Proposals only write files, so these on top of the video's
    /// files are the video as the proposal has it, which is what its preview builds.
    pub fn proposal_files(&self, slug: &str, name: &str) -> Result<BTreeMap<String, String>> {
        let head = self.proposal_head(slug, name)?;
        let (_, tip) = self.target(slug, &head)?;
        let base = self
            .repo
            .find_commit(self.repo.merge_base(tip.id(), head.id())?)?;
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&base.tree()?), Some(&head.tree()?), None)?;
        let mut files = BTreeMap::new();
        for delta in diff.deltas().filter(|d| d.status() != Delta::Deleted) {
            let file = delta.new_file();
            let Some(path) = file.path().and_then(|p| p.to_str()) else {
                continue;
            };
            let blob = self.repo.find_blob(file.id())?;
            files.insert(
                path.to_owned(),
                String::from_utf8_lossy(blob.content()).into_owned(),
            );
        }
        Ok(files)
    }

    /// Merges the proposal into the video branch, or into the default branch for a proposal of
    /// channel files, fast-forward when that branch has not moved, else with a merge commit,
    /// then deletes the proposal branch. On a conflict nothing changes and [`Error::Conflict`]
    /// lists the paths.
    pub fn accept_proposal(&self, slug: &str, name: &str) -> Result<Accepted> {
        let head = self.proposal_head(slug, name)?;
        let (branch, tip) = self.target(slug, &head)?;

        let (new_tip, fast_forward) = if self.repo.graph_descendant_of(head.id(), tip.id())? {
            (head.id(), true)
        } else if tip.id() == head.id() || self.repo.graph_descendant_of(tip.id(), head.id())? {
            (tip.id(), true)
        } else {
            let mut merged = self.repo.merge_commits(&tip, &head, None)?;
            if merged.has_conflicts() {
                return Err(Error::Conflict {
                    paths: conflict_paths(&merged)?,
                });
            }
            let tree = merged.write_tree_to(&self.repo)?;
            let subject = format!("🔀 accept {name} into {slug}");
            (
                self.commit_with_parents(&[&tip, &head], tree, &subject)?,
                false,
            )
        };
        self.set_branch(&branch, new_tip)?;
        self.delete_proposal(slug, name)?;
        Ok(Accepted {
            head: new_tip.to_string(),
            fast_forward,
        })
    }

    /// Deletes the proposal branch.
    pub fn reject_proposal(&self, slug: &str, name: &str) -> Result<()> {
        self.proposal_head(slug, name)?;
        self.delete_proposal(slug, name)
    }

    /// The branch a proposal merges into and its tip: the default branch when everything the
    /// proposal changes since it left the default branch is a channel file, else the video's.
    fn target(&self, slug: &str, head: &Commit<'_>) -> Result<(String, Commit<'_>)> {
        let (default, default_tip) = self.default_branch()?;
        let base = self
            .repo
            .find_commit(self.repo.merge_base(default_tip.id(), head.id())?)?;
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&base.tree()?), Some(&head.tree()?), None)?;
        let channel_only = diff.deltas().all(|d| {
            let file = d.new_file().path().or(d.old_file().path());
            file.and_then(|p| p.to_str()).is_some_and(is_channel_path)
        });
        if channel_only && diff.deltas().len() > 0 {
            Ok((default, default_tip))
        } else {
            Ok((branch_name(slug), self.tip(slug)?))
        }
    }

    fn proposal_head(&self, slug: &str, name: &str) -> Result<Commit<'_>> {
        check_name(slug)?;
        check_name(name)?;
        self.repo
            .find_branch(&proposal_branch(slug, name), BranchType::Local)
            .and_then(|b| b.get().peel_to_commit())
            .map_err(|_| Error::UnknownProposal {
                slug: slug.to_owned(),
                name: name.to_owned(),
            })
    }

    /// Deletes the branch and the proposal's render cache in the checked-out video folder.
    fn delete_proposal(&self, slug: &str, name: &str) -> Result<()> {
        self.repo
            .find_branch(&proposal_branch(slug, name), BranchType::Local)?
            .delete()?;
        if let Some(work) = self.repo.workdir() {
            // The cache is derived and safe to delete, so a failure to remove it is not one.
            let _ =
                std::fs::remove_dir_all(work.join(video_dir(slug)).join(PROPOSAL_CACHE).join(name));
        }
        Ok(())
    }
}

fn proposal_branch(slug: &str, name: &str) -> String {
    format!("{PROPOSAL_PREFIX}{slug}/{name}")
}

/// A repo path of the channel rather than of a video: `brand/`, `formats/`, `styles/`.
pub(crate) fn is_channel_path(path: &str) -> bool {
    !path.starts_with("videos/")
}

/// A relative path that stays inside the folder.
pub(crate) fn is_inside(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn conflict_paths(index: &git2::Index) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for conflict in index.conflicts()? {
        let conflict = conflict?;
        if let Some(entry) = conflict.our.or(conflict.their).or(conflict.ancestor) {
            paths.push(String::from_utf8_lossy(&entry.path).into_owned());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn file_diffs(diff: &Diff<'_>, folder: &str) -> Result<Vec<FileDiff>> {
    let mut files = Vec::new();
    for index in 0..diff.deltas().len() {
        let Some(mut patch) = Patch::from_diff(diff, index)? else {
            continue;
        };
        let delta = patch.delta();
        let file = if delta.status() == Delta::Deleted {
            delta.old_file()
        } else {
            delta.new_file()
        };
        let path = file.path().unwrap_or(Path::new("")).to_string_lossy();
        let path = path
            .strip_prefix(folder)
            .and_then(|p| p.strip_prefix('/'))
            .unwrap_or(&path)
            .to_owned();
        let patch = String::from_utf8_lossy(&patch.to_buf()?).into_owned();
        files.push(FileDiff { path, patch });
    }
    Ok(files)
}
