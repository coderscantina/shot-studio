//! Plain-text rendering shared by the `shot` CLI and the MCP tools, so both show the same words.

use std::fmt::Write;

use shot_board::{Activity, Card, FileDiff, Proposal};
use shot_core::Status;

/// Parses a board column name, listing the valid ones on a typo.
pub fn parse_status(text: &str) -> Result<Status, String> {
    Status::ALL
        .iter()
        .copied()
        .find(|status| status.as_str() == text)
        .ok_or_else(|| {
            let valid: Vec<_> = Status::ALL.iter().map(|s| s.as_str()).collect();
            format!(
                "unknown status `{text}`, expected one of: {}",
                valid.join(", ")
            )
        })
}

/// One line per card, grouped by column in board order. Cards that cannot be read are listed
/// under `unreadable` with the file:line:column of the parse error.
pub fn board(cards: &[Card]) -> String {
    let mut out = String::new();
    for &status in Status::ALL {
        let column: Vec<_> = cards
            .iter()
            .filter_map(|card| Some((card, card.video.as_ref().ok()?)))
            .filter(|(_, video)| video.status == status)
            .collect();
        let _ = writeln!(out, "{status} ({})", column.len());
        for (card, video) in column {
            let _ = write!(
                out,
                "  {}  {}  \"{}\"",
                card.slug, video.format, video.title
            );
            if let Some(date) = video.target_date {
                let _ = write!(
                    out,
                    "  target {:04}-{:02}-{:02}",
                    date.year(),
                    date.month(),
                    date.day()
                );
            }
            if !card.has_branch {
                out.push_str("  (no branch yet)");
            }
            out.push('\n');
        }
    }
    let broken: Vec<_> = cards
        .iter()
        .filter_map(|card| Some((&card.slug, card.video.as_ref().err()?)))
        .collect();
    if !broken.is_empty() {
        let _ = writeln!(out, "unreadable ({})", broken.len());
        for (slug, error) in broken {
            let _ = writeln!(out, "  {slug}  {error}");
        }
    }
    out
}

pub fn activity(entries: &[Activity]) -> String {
    if entries.is_empty() {
        return "no activity on video branches\n".to_owned();
    }
    entries
        .iter()
        .map(|e| {
            format!(
                "{}  {}  {}  {}  {}\n",
                utc(e.time),
                &e.sha[..e.sha.len().min(7)],
                e.slug,
                e.author,
                e.subject
            )
        })
        .collect()
}

pub fn proposals(found: &[Proposal]) -> String {
    if found.is_empty() {
        return "no open proposals\n".to_owned();
    }
    found
        .iter()
        .map(|p| format!("{}/{}  {}\n", p.slug, p.name, p.subject))
        .collect()
}

pub fn diffs(files: &[FileDiff]) -> String {
    if files.is_empty() {
        return "the proposal changes nothing compared to its base\n".to_owned();
    }
    files.iter().map(|f| f.patch.as_str()).collect()
}

/// `2026-11-07 14:03Z` from seconds since the Unix epoch (civil-from-days, proleptic Gregorian).
fn utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_seconds_as_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00Z");
        assert_eq!(utc(1_794_060_180), "2026-11-07 14:03Z");
    }

    #[test]
    fn status_typos_list_the_valid_columns() {
        assert_eq!(parse_status("ready"), Ok(Status::Ready));
        assert!(
            parse_status("done")
                .unwrap_err()
                .contains("ideas, scripting")
        );
    }
}
