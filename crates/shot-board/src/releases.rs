//! Release tags for shipped videos.

use git2::BranchType;
use shot_core::{Date, Status};

use crate::{Board, Error, Result, branch_name, check_name};

const TAG_PREFIX: &str = "refs/tags/v";
const MESSAGE_PREFIX: &str = "🚀 ship ";

/// The CalVer name for the `n`th release of `date`: `v26.11.7`, then `v26.11.7.2`.
fn tag_name(date: Date, n: u32) -> String {
    let base = format!("v{}.{}.{}", date.year() % 100, date.month(), date.day());
    if n == 1 { base } else { format!("{base}.{n}") }
}

/// The numeric parts of a CalVer tag name, so `v26.11.10` sorts after `v26.11.9`.
fn sort_key(name: &str) -> Vec<u32> {
    name.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// A release tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTag {
    pub name: String,
    /// The tagged (shipped) commit.
    pub commit: String,
    /// The video the tag was made for, read from the tag message.
    pub slug: Option<String>,
}

impl Board {
    /// Release tags, newest first.
    pub fn releases(&self) -> Result<Vec<ReleaseTag>> {
        let mut tags = Vec::new();
        for reference in self.repo.references_glob(&format!("{TAG_PREFIX}*"))? {
            let reference = reference?;
            let Some(name) = reference
                .name()
                .ok()
                .and_then(|n| n.strip_prefix("refs/tags/"))
            else {
                continue;
            };
            let slug = reference
                .peel(git2::ObjectType::Tag)
                .ok()
                .and_then(|o| o.into_tag().ok())
                .and_then(|t| {
                    t.message()
                        .ok()
                        .flatten()?
                        .trim()
                        .strip_prefix(MESSAGE_PREFIX)
                        .map(str::to_owned)
                });
            tags.push(ReleaseTag {
                name: name.to_owned(),
                commit: reference.peel_to_commit()?.id().to_string(),
                slug,
            });
        }
        tags.sort_by_key(|tag| std::cmp::Reverse(sort_key(&tag.name)));
        Ok(tags)
    }

    /// Tags the tip of a shipped video's branch as released on `date`. A later release on the
    /// same day gets the next `.N` suffix. Errors if the video is not in the `shipped` column.
    pub fn tag_release(&self, slug: &str, date: Date) -> Result<ReleaseTag> {
        check_name(slug)?;
        let tip = self
            .repo
            .find_branch(&branch_name(slug), BranchType::Local)
            .map_err(|_| Error::UnknownVideo(slug.to_owned()))?
            .get()
            .peel_to_commit()?;
        if self.read_video(&tip, slug)?.status != Status::Shipped {
            return Err(Error::NotShipped(slug.to_owned()));
        }
        let name = (1..)
            .map(|n| tag_name(date, n))
            .find(|name| {
                self.repo
                    .find_reference(&format!("refs/tags/{name}"))
                    .is_err()
            })
            .expect("an unused suffix exists");
        self.repo.tag(
            &name,
            tip.as_object(),
            &self.signature(),
            &format!("{MESSAGE_PREFIX}{slug}"),
            false,
        )?;
        Ok(ReleaseTag {
            name,
            commit: tip.id().to_string(),
            slug: Some(slug.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calver_names_sort_numerically() {
        let date = Date::new(2026, 11, 7).unwrap();
        assert_eq!(tag_name(date, 1), "v26.11.7");
        assert_eq!(tag_name(date, 2), "v26.11.7.2");
        assert!(sort_key("v26.11.10") > sort_key("v26.11.9"));
        assert!(sort_key("v26.11.7.2") > sort_key("v26.11.7"));
    }
}
