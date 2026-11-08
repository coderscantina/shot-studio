//! The Ship screen's words: names of cuts and checks, sizes, times and run steps.

use std::time::Duration;

use shot_core::{DerivedKind, OutputKind};
use shot_ship::check::CheckKind;
use shot_ship::ship::{Destination, PlannedCut, Step};
use shot_ship::youtube::Privacy;

pub fn cut_title(cut: &PlannedCut) -> String {
    match cut.kind {
        OutputKind::Main => "Main video".to_owned(),
        OutputKind::Derived(DerivedKind::Short) => format!("Short: {}", cut.name),
        OutputKind::Derived(DerivedKind::XClip) => format!("Clip for X: {}", cut.name),
    }
}

pub fn aspect(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::Main => "16:9",
        OutputKind::Derived(DerivedKind::Short) => "9:16",
        OutputKind::Derived(DerivedKind::XClip) => "1:1",
    }
}

pub fn destination(cut: &PlannedCut) -> &'static str {
    match (cut.destination, cut.kind) {
        (Destination::File, _) => "File",
        (_, OutputKind::Main) => "YouTube",
        _ => "Shorts",
    }
}

pub fn privacy(privacy: Privacy) -> &'static str {
    match privacy {
        Privacy::Private => "Private",
        Privacy::Unlisted => "Unlisted",
        Privacy::Public => "Public",
    }
}

pub fn check_name(kind: CheckKind) -> &'static str {
    match kind {
        CheckKind::Loudness => "Loudness",
        CheckKind::Captions => "Captions",
        CheckKind::Frames => "No black or frozen frames",
        CheckKind::Links => "Links have cards",
        CheckKind::Chapters => "Chapters",
        CheckKind::Thumbnail => "Thumbnail",
        CheckKind::Overrides => "Overrides find their element",
    }
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// A file size the way Finder shows it, in decimal units: "5.8 GB", "420 MB".
pub fn bytes(n: u64) -> String {
    const MB: f64 = 1e6;
    const GB: f64 = 1e9;
    let n = n as f64;
    if n >= 10.0 * GB {
        format!("{:.0} GB", n / GB)
    } else if n >= GB {
        format!("{:.1} GB", n / GB)
    } else {
        format!("{:.0} MB", (n / MB).max(1.0))
    }
}

/// How long ago something happened: "just now", "12 min ago", "3 h ago", "2 days ago".
pub fn ago(elapsed: Duration) -> String {
    let minutes = elapsed.as_secs() / 60;
    match minutes {
        0 => "just now".to_owned(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        _ => match minutes / 1440 {
            1 => "yesterday".to_owned(),
            days => format!("{days} days ago"),
        },
    }
}

/// The time left of `done` of `total` units after `elapsed`, once there is a pace to go by.
pub fn left(elapsed: Duration, done: usize, total: usize) -> Option<String> {
    if done == 0 || done >= total {
        return None;
    }
    let left = elapsed
        .mul_f64((total - done) as f64 / done as f64)
        .as_secs();
    Some(match left {
        0..60 => "under a minute left".to_owned(),
        60..3600 => format!("about {} min left", left.div_ceil(60)),
        _ => format!("about {} h {} min left", left / 3600, left % 3600 / 60),
    })
}

/// The line a run shows for `step`.
pub fn step_line(step: &Step) -> String {
    match step {
        Step::Rendering {
            cut, to_render: 0, ..
        } => format!("Preparing {cut}"),
        Step::Rendering {
            cut,
            rendered,
            to_render,
        } => format!("Rendering {cut}, {rendered} of {to_render} chunks"),
        Step::Checking { cut } => format!("Checking {cut}"),
        Step::Uploading { cut, sent } => format!(
            "Uploading {cut}, {} of {}",
            bytes(sent.bytes),
            bytes(sent.total)
        ),
        Step::Uploaded { cut, .. } | Step::AlreadyUploaded { cut, .. } => {
            format!("Uploaded {cut}")
        }
        Step::Thumbnail { cut } => format!("Set the thumbnail of {cut}"),
        Step::ThumbnailRefused { cut, .. } => format!("YouTube refused the thumbnail of {cut}"),
        Step::File { cut, .. } => format!("{cut} is ready as a file"),
        Step::Moved { slug } => format!("Moved {slug} to shipped"),
        Step::Tagged { tag } => format!("Tagged {tag}"),
    }
}

/// How far `step` is, 0 to 1, when it has a measure.
pub fn step_fraction(step: &Step) -> Option<f32> {
    match step {
        Step::Rendering {
            rendered,
            to_render,
            ..
        } if *to_render > 0 => Some(*rendered as f32 / *to_render as f32),
        Step::Uploading { sent, .. } => Some(sent.bytes as f32 / sent.total.max(1) as f32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_ship::youtube::Sent;

    #[test]
    fn sizes_read_like_finder() {
        assert_eq!(bytes(5_812_000_000), "5.8 GB");
        assert_eq!(bytes(48_300_000_000), "48 GB");
        assert_eq!(bytes(420_400_000), "420 MB");
        assert_eq!(bytes(1_000), "1 MB");
    }

    #[test]
    fn times_read_in_plain_words() {
        assert_eq!(ago(Duration::from_secs(20)), "just now");
        assert_eq!(ago(Duration::from_secs(12 * 60 + 5)), "12 min ago");
        assert_eq!(ago(Duration::from_secs(3 * 3600)), "3 h ago");
        assert_eq!(ago(Duration::from_secs(30 * 3600)), "yesterday");
        assert_eq!(ago(Duration::from_secs(80 * 3600)), "3 days ago");
    }

    #[test]
    fn the_time_left_follows_the_pace_so_far() {
        let minute = Duration::from_secs(60);
        assert_eq!(left(minute, 0, 10), None, "no pace yet");
        assert_eq!(left(minute, 10, 10), None, "done");
        assert_eq!(left(minute, 5, 10).unwrap(), "about 1 min left");
        assert_eq!(left(minute, 1, 4).unwrap(), "about 3 min left");
        assert_eq!(left(minute * 30, 1, 4).unwrap(), "about 1 h 30 min left");
        assert_eq!(
            left(Duration::from_secs(10), 1, 2).unwrap(),
            "under a minute left"
        );
    }

    #[test]
    fn steps_say_where_a_run_is() {
        let rendering = |rendered, to_render| Step::Rendering {
            cut: "main".to_owned(),
            rendered,
            to_render,
        };
        assert_eq!(step_line(&rendering(0, 0)), "Preparing main");
        assert_eq!(
            step_line(&rendering(3, 12)),
            "Rendering main, 3 of 12 chunks"
        );
        assert_eq!(step_fraction(&rendering(3, 12)), Some(0.25));
        assert_eq!(step_fraction(&rendering(0, 0)), None);
        let uploading = Step::Uploading {
            cut: "main".to_owned(),
            sent: Sent {
                bytes: 1_500_000_000,
                total: 6_000_000_000,
            },
        };
        assert_eq!(step_line(&uploading), "Uploading main, 1.5 GB of 6.0 GB");
        assert_eq!(step_fraction(&uploading), Some(0.25));
    }
}
