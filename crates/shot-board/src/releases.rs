//! Release tags for shipped videos.

use git2::BranchType;
use shot_core::{Date, Status};

use crate::{Board, Error, Result, branch_name, check_name};

const TAG_PREFIX: &str = "refs/tags/v";
const MESSAGE_PREFIX: &str = "🚀 ship ";

/// An ISO week, the unit of a release tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseWeek {
    pub year: u16,
    pub week: u8,
}

impl ReleaseWeek {
    /// The ISO week the date falls in. The year can differ from the date's near new year.
    pub fn of(date: Date) -> Self {
        let year = i32::from(date.year());
        let (m, d) = (usize::from(date.month()), i32::from(date.day()));
        const BEFORE_MONTH: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let leap = i32::from(is_leap(year) && m > 2);
        let ordinal = BEFORE_MONTH[m - 1] + d + leap;
        let weekday = weekday(year, m, d);
        let week = (ordinal - weekday + 10) / 7;
        let (year, week) = if week < 1 {
            (year - 1, weeks_in(year - 1))
        } else if week > weeks_in(year) {
            (year + 1, 1)
        } else {
            (year, week)
        };
        Self {
            year: u16::try_from(year).expect("year stays positive"),
            week: u8::try_from(week).expect("week is 1 to 53"),
        }
    }

    /// The tag name, for example `v2026.46`.
    pub fn tag_name(self) -> String {
        format!("v{}.{:02}", self.year, self.week)
    }
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
    /// Release tags, newest name first.
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
        tags.sort_by(|a, b| b.name.cmp(&a.name));
        Ok(tags)
    }

    /// Tags the tip of a shipped video's branch for the given week. Errors if the video is not
    /// in the `shipped` column or the tag exists.
    pub fn tag_release(&self, slug: &str, week: ReleaseWeek) -> Result<ReleaseTag> {
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
        let name = week.tag_name();
        if self
            .repo
            .find_reference(&format!("refs/tags/{name}"))
            .is_ok()
        {
            return Err(Error::TagExists(name));
        }
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

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// 1 for Monday to 7 for Sunday (Sakamoto's method).
fn weekday(year: i32, month: usize, day: i32) -> i32 {
    const OFFSET: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let sunday_first = (y + y / 4 - y / 100 + y / 400 + OFFSET[month - 1] + day) % 7;
    if sunday_first == 0 { 7 } else { sunday_first }
}

fn weeks_in(year: i32) -> i32 {
    let p = |y: i32| (y + y / 4 - y / 100 + y / 400) % 7;
    if p(year) == 4 || p(year - 1) == 3 {
        53
    } else {
        52
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn week(y: u16, m: u8, d: u8) -> (u16, u8) {
        let w = ReleaseWeek::of(Date::new(y, m, d).unwrap());
        (w.year, w.week)
    }

    #[test]
    fn iso_weeks() {
        assert_eq!(week(2026, 11, 9), (2026, 46));
        assert_eq!(week(2026, 11, 7), (2026, 45));
        assert_eq!(week(2027, 1, 1), (2026, 53));
        assert_eq!(week(2024, 12, 30), (2025, 1));
        assert_eq!(week(2024, 3, 1), (2024, 9));
        assert_eq!(
            ReleaseWeek {
                year: 2026,
                week: 5
            }
            .tag_name(),
            "v2026.05"
        );
    }
}
