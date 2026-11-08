//! `shot check`: the rules every cut passes before it ships. Each rule is a pure function over
//! what was measured, so it is tested without media. Passing checks return nothing.

use std::collections::HashMap;
use std::fmt;

use shot_audio::{CEILING_DBTP, TARGET_LUFS};
use shot_core::{CaptionFix, CaptionStyle, Lock, Micros, Output, OutputKind, Override};

use crate::describe::LINK_CARD;
use crate::spoken::{ChapterMark, SpokenWord, clock};

/// Integrated loudness may miss the target by this much.
pub const LOUDNESS_TOLERANCE: f64 = 1.0;
/// The true peak ceiling plus rounding slack of the meter.
pub const PEAK_MAX: f64 = CEILING_DBTP + 0.1;
/// Words recognized below this confidence are worth a look before captions ship.
pub const LOW_CONFIDENCE: u8 = 50;
/// YouTube shows chapters only with at least three, each at least 10 s.
pub const CHAPTERS_MIN: usize = 3;
pub const CHAPTER_MIN_LENGTH: Micros = Micros(10_000_000);
/// A black stretch at least this long is a problem, a dip to black between shots is not.
pub const BLACK_MIN: Micros = Micros(1_000_000);
/// A picture that does not change at all for this long is a frozen frame.
pub const FROZEN_MIN: Micros = Micros(3_000_000);
/// A said link needs a link card starting within this distance.
pub const LINK_CARD_WINDOW: Micros = Micros(10_000_000);

/// Overrides the lock lists as orphans: after re-analysis no element of their kind starts within
/// [`Override::TOLERANCE`] of their `at`, so they change nothing.
pub fn orphans(lock: &Lock) -> Vec<Problem> {
    let within = Override::TOLERANCE.0 as f64 / 1e6;
    lock.takes
        .iter()
        .flat_map(|t| t.orphans.iter().map(move |o| (&t.take, o)))
        .map(|(take, o)| {
            problem(
                CheckKind::Overrides,
                format!(
                    "override {take} {} at {:.1} s lost its element: no {} starts within {within} s, so it changes nothing. Move its `at` to the element or delete it",
                    o.kind,
                    o.at.0 as f64 / 1e6,
                    o.kind
                ),
            )
        })
        .collect()
}

/// Endings that make a said word a web address.
const TLDS: [&str; 12] = [
    "com", "dev", "io", "org", "net", "sh", "app", "rs", "ai", "co", "gg", "tv",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckKind {
    Loudness,
    Captions,
    Frames,
    Links,
    Chapters,
    Thumbnail,
    /// Overrides in `cut.kdl` whose element is gone.
    Overrides,
}

impl CheckKind {
    pub const ALL: [Self; 7] = [
        Self::Loudness,
        Self::Captions,
        Self::Frames,
        Self::Links,
        Self::Chapters,
        Self::Thumbnail,
        Self::Overrides,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loudness => "loudness",
            Self::Captions => "captions",
            Self::Frames => "frames",
            Self::Links => "links",
            Self::Chapters => "chapters",
            Self::Thumbnail => "thumbnail",
            Self::Overrides => "overrides",
        }
    }
}

/// A failed check and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub check: CheckKind,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.check.as_str(), self.message)
    }
}

fn problem(check: CheckKind, message: String) -> Problem {
    Problem { check, message }
}

/// Loudness of the exported audio against -14 LUFS and the -1 dBTP ceiling.
pub fn loudness(lufs: Option<f64>, true_peak: f64) -> Vec<Problem> {
    let mut out = Vec::new();
    match lufs {
        None => out.push(problem(
            CheckKind::Loudness,
            "the audio is silent".to_owned(),
        )),
        Some(lufs) if (lufs - TARGET_LUFS).abs() > LOUDNESS_TOLERANCE => {
            out.push(problem(
                CheckKind::Loudness,
                format!(
                    "integrated loudness is {lufs:.1} LUFS, the target is {TARGET_LUFS:.0} LUFS"
                ),
            ));
        }
        Some(_) => {}
    }
    if true_peak > PEAK_MAX {
        out.push(problem(
            CheckKind::Loudness,
            format!("true peak is {true_peak:.1} dBTP, above the -1 dBTP ceiling"),
        ));
    }
    out
}

/// Captions are present and every caption fix made it into them. `sidecar` is the WebVTT text
/// next to the export for the main cut, `None` when there is none; burned-in captions of
/// derived cuts have no file to read. Low-confidence words not covered by a fix are listed so a
/// person can add one.
pub fn captions(
    style: CaptionStyle,
    burned: bool,
    sidecar: Option<&str>,
    fixes: &[CaptionFix],
    words: &[SpokenWord],
    takes_without_transcript: &[String],
) -> Vec<Problem> {
    if style == CaptionStyle::Off {
        return Vec::new();
    }
    let mut out = Vec::new();
    if !takes_without_transcript.is_empty() {
        out.push(problem(
            CheckKind::Captions,
            format!(
                "no transcript for {}, so no captions there: run `shot transcribe`",
                takes_without_transcript.join(", ")
            ),
        ));
    }
    if !burned {
        match sidecar {
            None => out.push(problem(
                CheckKind::Captions,
                "the caption sidecar is missing, export again".to_owned(),
            )),
            Some(vtt) => {
                let lower = vtt.to_lowercase();
                for fix in fixes {
                    if contains_words(&lower, &fix.heard.to_lowercase()) {
                        out.push(problem(
                            CheckKind::Captions,
                            format!(
                                "the sidecar still reads \"{}\" where the fix says \"{}\", export again",
                                fix.heard, fix.said
                            ),
                        ));
                    }
                }
            }
        }
    }
    let fixed: Vec<String> = fixes.iter().map(|f| f.heard.to_lowercase()).collect();
    let unsure: Vec<String> = words
        .iter()
        .filter(|w| w.confidence < LOW_CONFIDENCE)
        .filter(|w| {
            let text = w.text.to_lowercase();
            !fixed
                .iter()
                .any(|f| f.split_whitespace().any(|h| h == text))
        })
        .map(|w| format!("\"{}\" at {}", w.text, clock(w.at)))
        .collect();
    if !unsure.is_empty() {
        let shown: Vec<_> = unsure.iter().take(5).cloned().collect();
        let more = unsure.len().saturating_sub(shown.len());
        out.push(problem(
            CheckKind::Captions,
            format!(
                "{} word{} heard with low confidence: {}{}; add a caption fix where it was misheard",
                unsure.len(),
                if unsure.len() == 1 { "" } else { "s" },
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ),
        ));
    }
    out
}

/// Whether `haystack` holds `needle` as whole words.
fn contains_words(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// One sampled frame of the export: when, its mean luma and a coarse grid of block means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSample {
    pub at: Micros,
    /// Mean of the luma plane in video range, 16 is black.
    pub luma: u8,
    pub grid: Vec<u8>,
}

/// Mean luma under this is a black frame.
const BLACK_LUMA: u8 = 24;

/// Black stretches and frozen pictures from frames sampled in order.
pub fn frames(samples: &[FrameSample]) -> Vec<Problem> {
    let mut out = Vec::new();
    let runs = |same: &dyn Fn(&FrameSample, &FrameSample) -> bool,
                single: &dyn Fn(&FrameSample) -> bool| {
        let mut runs = Vec::new();
        let mut start: Option<usize> = None;
        for i in 0..samples.len() {
            let joins = single(&samples[i]) && start.is_none_or(|s| same(&samples[s], &samples[i]));
            match (start, joins) {
                (None, true) => start = Some(i),
                (Some(s), false) => {
                    runs.push((s, i - 1));
                    start = single(&samples[i]).then_some(i);
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            runs.push((s, samples.len() - 1));
        }
        runs
    };
    let length = |(s, e): (usize, usize)| Micros(samples[e].at.0 - samples[s].at.0);
    for run in runs(&|_, _| true, &|f| f.luma < BLACK_LUMA) {
        if length(run) >= BLACK_MIN {
            out.push(problem(
                CheckKind::Frames,
                format!(
                    "black from {} to {}",
                    clock(samples[run.0].at),
                    clock(samples[run.1].at)
                ),
            ));
        }
    }
    let frozen = |a: &FrameSample, b: &FrameSample| {
        a.grid.len() == b.grid.len() && a.grid.iter().zip(&b.grid).all(|(x, y)| x.abs_diff(*y) <= 1)
    };
    for run in runs(&frozen, &|f| f.luma >= BLACK_LUMA) {
        if length(run) >= FROZEN_MIN {
            out.push(problem(
                CheckKind::Frames,
                format!(
                    "the picture freezes from {} to {}",
                    clock(samples[run.0].at),
                    clock(samples[run.1].at)
                ),
            ));
        }
    }
    out
}

/// A web address the speaker says, with when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaidLink {
    pub text: String,
    pub at: Micros,
}

/// Web addresses in the spoken words: `laravel.com` as one word, or `laravel dot com`.
pub fn said_links(words: &[SpokenWord]) -> Vec<SaidLink> {
    let clean = |w: &str| {
        w.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.')
            .trim_end_matches('.')
            .to_lowercase()
    };
    let mut found = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let text = clean(&word.text);
        let host = text.split('/').next().unwrap_or_default();
        if let Some((name, tld)) = host.rsplit_once('.')
            && !name.is_empty()
            && TLDS.contains(&tld)
        {
            found.push(SaidLink { text, at: word.at });
        } else if text == "dot"
            && let (Some(before), Some(after)) = (i.checked_sub(1), words.get(i + 1))
            && TLDS.contains(&clean(&after.text).as_str())
        {
            found.push(SaidLink {
                text: format!("{}.{}", clean(&words[before].text), clean(&after.text)),
                at: words[before].at,
            });
        }
    }
    found
}

/// Every link said out loud has a link card near it.
pub fn links(said: &[SaidLink], cards: &[Micros]) -> Vec<Problem> {
    said.iter()
        .filter(|link| {
            !cards
                .iter()
                .any(|card| card.0.abs_diff(link.at.0) <= LINK_CARD_WINDOW.0)
        })
        .map(|link| {
            problem(
                CheckKind::Links,
                format!(
                    "\"{}\" is said at {} without a link card",
                    link.text,
                    clock(link.at)
                ),
            )
        })
        .collect()
}

/// Where each link card of `output` starts, in output time.
pub fn link_cards(output: &Output) -> Vec<Micros> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for segment in output.chapters.iter().flat_map(|c| &c.segments) {
        let source = segment.source;
        for overlay in &segment.overlays {
            let from = overlay.range.from();
            if overlay.template.as_str() == LINK_CARD && source.from() <= from && from < source.to()
            {
                starts.push(Micros(offset + from.0 - source.from().0));
            }
        }
        offset += source.duration().0;
    }
    starts
}

/// YouTube's chapter rules: at least three, each at least 10 s, and no placeholder titles.
pub fn chapters(kind: OutputKind, marks: &[ChapterMark], length: Micros) -> Vec<Problem> {
    if kind != OutputKind::Main {
        return Vec::new();
    }
    let mut out = Vec::new();
    if marks.len() < CHAPTERS_MIN {
        out.push(problem(
            CheckKind::Chapters,
            format!(
                "{} chapter{}, YouTube shows chapters from {CHAPTERS_MIN}",
                marks.len(),
                if marks.len() == 1 { "" } else { "s" }
            ),
        ));
    }
    for (i, mark) in marks.iter().enumerate() {
        let end = marks.get(i + 1).map_or(length, |next| next.at);
        if end.0.saturating_sub(mark.at.0) < CHAPTER_MIN_LENGTH.0 {
            out.push(problem(
                CheckKind::Chapters,
                format!(
                    "\"{}\" at {} is shorter than 10 s",
                    mark.title,
                    clock(mark.at)
                ),
            ));
        }
        if mark.title.starts_with("Take ") {
            out.push(problem(
                CheckKind::Chapters,
                format!(
                    "\"{}\" at {} still has its placeholder title",
                    mark.title,
                    clock(mark.at)
                ),
            ));
        }
    }
    out
}

/// The main cut ships with a thumbnail.
pub fn thumbnail(kind: OutputKind, present: bool) -> Vec<Problem> {
    if kind != OutputKind::Main || present {
        return Vec::new();
    }
    vec![problem(
        CheckKind::Thumbnail,
        "no thumbnail yet, pick one from `shot thumbnails`".to_owned(),
    )]
}

/// Takes the output plays from that have no transcript, sorted.
pub fn takes_without_transcript<T>(
    output: &Output,
    transcripts: &HashMap<shot_core::Id, T>,
) -> Vec<String> {
    let mut missing: Vec<String> = output
        .chapters
        .iter()
        .flat_map(|c| &c.segments)
        .filter(|s| !transcripts.contains_key(&s.take))
        .map(|s| s.take.as_str().to_owned())
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, at: u64, confidence: u8) -> SpokenWord {
        SpokenWord {
            text: text.to_owned(),
            at: Micros(at),
            to: Micros(at + 300_000),
            chapter: 0,
            confidence,
        }
    }

    #[test]
    fn loudness_passes_on_target_and_flags_quiet_or_clipping_audio() {
        assert!(loudness(Some(-14.3), -1.2).is_empty());
        assert_eq!(loudness(Some(-18.0), -1.2).len(), 1);
        assert_eq!(loudness(Some(-14.0), -0.2).len(), 1);
        assert_eq!(loudness(None, -70.0)[0].message, "the audio is silent");
    }

    #[test]
    fn captions_need_the_sidecar_with_fixes_applied_and_list_unsure_words() {
        let fixes = [CaptionFix {
            heard: "lara vel".to_owned(),
            said: "Laravel".to_owned(),
        }];
        let words = [
            word("lara", 0, 30),
            word("vel", 300_000, 30),
            word("ship", 600_000, 95),
        ];
        assert!(
            captions(
                CaptionStyle::Clean,
                false,
                Some("WEBVTT\n\nLaravel ship\n"),
                &fixes,
                &words,
                &[]
            )
            .is_empty()
        );
        assert!(captions(CaptionStyle::Off, false, None, &fixes, &words, &[]).is_empty());
        let stale = captions(
            CaptionStyle::Clean,
            false,
            Some("WEBVTT\n\nlara vel ship\n"),
            &fixes,
            &words,
            &[],
        );
        assert!(stale[0].message.contains("still reads \"lara vel\""));
        let missing = captions(
            CaptionStyle::Clean,
            false,
            None,
            &[],
            &[word("jeep", 61_000_000, 20)],
            &[],
        );
        assert!(missing[0].message.contains("sidecar is missing"));
        assert!(missing[1].message.contains("\"jeep\" at 1:01"));
    }

    #[test]
    fn frames_flag_long_black_and_frozen_stretches_only() {
        let sample = |at: u64, luma: u8, grid: u8| FrameSample {
            at: Micros(at),
            luma,
            grid: vec![grid; 4],
        };
        let mut samples: Vec<_> = (0..4).map(|i| sample(i * 500_000, 16, 16)).collect();
        samples.push(sample(2_000_000, 120, 40));
        samples.extend((5..14).map(|i| sample(i * 500_000, 120, 90)));
        samples.push(sample(7_000_000, 120, 10));
        let found = frames(&samples);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].message.starts_with("black from 0:00 to 0:01"));
        assert!(
            found[1]
                .message
                .starts_with("the picture freezes from 0:02 to 0:06")
        );
        assert!(frames(&samples[..2]).is_empty());
    }

    #[test]
    fn said_links_need_a_link_card_nearby() {
        let words = [
            word("see", 0, 90),
            word("laravel.com.", 1_000_000, 90),
            word("and", 2_000_000, 90),
            word("shot", 40_000_000, 90),
            word("dot", 40_300_000, 90),
            word("dev", 40_600_000, 90),
        ];
        let said = said_links(&words);
        assert_eq!(
            said.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["laravel.com", "shot.dev"]
        );
        let found = links(&said, &[Micros(3_000_000)]);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].message,
            "\"shot.dev\" is said at 0:40 without a link card"
        );
    }

    #[test]
    fn chapters_follow_youtube_rules_on_the_main_cut() {
        let mark = |title: &str, at: u64| ChapterMark {
            title: title.to_owned(),
            at: Micros(at),
        };
        let good = [
            mark("Hook", 0),
            mark("Why", 20_000_000),
            mark("How", 60_000_000),
        ];
        assert!(chapters(OutputKind::Main, &good, Micros(90_000_000)).is_empty());
        let bad = [mark("Take t01", 0), mark("Why", 5_000_000)];
        let found = chapters(OutputKind::Main, &bad, Micros(90_000_000));
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(
            chapters(
                OutputKind::Derived(shot_core::DerivedKind::Short),
                &bad,
                Micros(1)
            )
            .is_empty()
        );
    }

    #[test]
    fn an_override_that_lost_its_element_is_a_problem() {
        let lock =
            Lock::parse("lock 1\n\nbrand {\n}\n\ntake t01 {\n    orphan filler at=30_000_000\n}\n")
                .unwrap();
        let found = orphans(&lock);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].check, CheckKind::Overrides);
        assert!(
            found[0]
                .message
                .starts_with("override t01 filler at 30.0 s lost its element"),
            "{}",
            found[0].message
        );
        assert!(orphans(&Lock::default()).is_empty());
    }
}
