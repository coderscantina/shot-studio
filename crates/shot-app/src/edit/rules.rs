//! What the style rules cut of the video, by kind, for the Text inspector. The rules and the
//! pacing are the format's style rules; a person answers single findings in the transcript.

use gpui::{Div, FontWeight, div, prelude::*, px};
use shot_core::{Cut, ElementKind, Lock, Micros, RemovedBy, Target};

use crate::model;
use crate::theme::*;
use crate::ui::{label, mono};

/// The name of what the rules cut of an element kind, in the rule list.
fn rule_name(kind: ElementKind) -> String {
    match kind {
        ElementKind::Silence => "Silences".to_owned(),
        ElementKind::Retake => "Restarts".to_owned(),
        ElementKind::Misspoken => "Misspoken words".to_owned(),
        ElementKind::Pause => "Pauses mid-sentence".to_owned(),
        ElementKind::Filler => "Filler words".to_owned(),
        ElementKind::Idle => "Idle screen".to_owned(),
        other => format!("Cuts on {other}"),
    }
}

/// What the lock and a person removed of one element kind, for its row in the rule list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KindCount {
    kind: ElementKind,
    removed: usize,
    removed_time: Micros,
    queued: usize,
}

/// The rules' cuts and open review items by element kind, in the order kinds come first.
fn counts(cut: &Cut, lock: &Lock) -> Vec<KindCount> {
    let mut out: Vec<KindCount> = Vec::new();
    let mut add = |kind: ElementKind, removed: Option<Micros>| {
        let index = match out.iter().position(|c| c.kind == kind) {
            Some(index) => index,
            None => {
                out.push(KindCount {
                    kind,
                    removed: 0,
                    removed_time: Micros(0),
                    queued: 0,
                });
                out.len() - 1
            }
        };
        let count = &mut out[index];
        match removed {
            Some(length) => {
                count.removed += 1;
                count.removed_time.0 += length.0;
            }
            None => count.queued += 1,
        }
    };
    for (_, edit) in model::blocks(cut) {
        for (range, by) in model::removals(edit, lock) {
            if let RemovedBy::Rule(Target::Kind(kind)) = by {
                add(kind, Some(range.duration()));
            }
        }
    }
    for (_, review) in cut.review_queue(lock) {
        add(review.kind, None);
    }
    out
}

/// `M:SS` of a length.
fn minutes(t: Micros) -> String {
    let seconds = t.0 / 1_000_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A length as "2.1 s" under a minute, "1:48" from there.
fn length(t: Micros) -> String {
    if t.0 < 60_000_000 {
        model::seconds(t)
    } else {
        minutes(t)
    }
}

/// What the rules did of a kind, for its row: "12 cuts, 2:41 · 3 to look at". Each piece says
/// whether it is a duration, which shows in Geist Mono.
fn rule_detail(count: &KindCount) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    if count.removed > 0 {
        let cuts = if count.removed == 1 { "cut" } else { "cuts" };
        parts.push((format!("{} {cuts},", count.removed), false));
        // The unit is set in Geist, as a space in Geist Mono reads twice as wide.
        let time = length(count.removed_time);
        match time.strip_suffix(" s") {
            Some(number) => parts.extend([(number.to_owned(), true), ("s".to_owned(), false)]),
            None => parts.push((time, true)),
        }
    }
    if count.queued > 0 {
        if !parts.is_empty() {
            parts.push(("·".to_owned(), false));
        }
        parts.push((format!("{} to look at", count.queued), false));
    }
    parts
}

/// The rules' cuts by kind, one line each, and where the rules are set.
pub fn summary(cut: &Cut, lock: &Lock, format: &str) -> Div {
    let counts = counts(cut, lock);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Cut by the rules"),
        )
        .children(counts.iter().map(|count| {
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .text_size(TEXT_SM)
                .child(div().flex_1().text_color(BODY).child(rule_name(count.kind)))
                .child(div().flex().gap(px(4.)).text_color(MUTED).children(
                    rule_detail(count).into_iter().map(|(text, duration)| {
                        if duration {
                            mono(text)
                        } else {
                            div().child(text)
                        }
                    }),
                ))
        }))
        .when(counts.is_empty(), |list| list.child(label("Nothing yet")))
        .child(label(format!(
            "The rules and the pacing are the {format} format's, for every video of it."
        )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_and_rule_rows_read_plainly() {
        assert_eq!(length(Micros(2_100_000)), "2.1 s");
        assert_eq!(length(Micros(108_000_000)), "1:48");
        let count = KindCount {
            kind: ElementKind::Silence,
            removed: 12,
            removed_time: Micros(161_000_000),
            queued: 3,
        };
        let text: Vec<String> = rule_detail(&count).into_iter().map(|(t, _)| t).collect();
        assert_eq!(text.join(" "), "12 cuts, 2:41 · 3 to look at");
        assert_eq!(rule_name(ElementKind::Retake), "Restarts");
    }
}
