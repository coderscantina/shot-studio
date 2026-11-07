//! Claude's reading of a take: the prompt that sends it the transcript, and its answer turned
//! into elements. Text only ([0005]): Claude sees the words, never the media. The caller runs
//! the prompt through the `claude` bridge.
//!
//! [0005]: ../../../../docs/decisions/0005-privacy-text-only-to-claude.md

use std::fmt::Write;

use serde::Deserialize;
use shot_core::{Attr, Element, ElementKind, TimeRange, Transcript};

use super::{BY_CLAUDE, sentences};

/// The kinds Claude marks.
pub const KINDS: [ElementKind; 6] = [
    ElementKind::Claim,
    ElementKind::Punchline,
    ElementKind::Aside,
    ElementKind::NeedsSource,
    ElementKind::TopicShift,
    ElementKind::Product,
];

/// The request for `transcript`: its sentences, numbered, and the answer format.
pub fn prompt(transcript: &Transcript) -> String {
    let words = transcript.words();
    let mut out = String::from(
        "Read the transcript of one recording below and mark what it means. Do not call any \
         tool and do not propose anything: answer only with JSON objects, one per line, and \
         nothing else.\n\nEach object is {\"kind\": ..., \"from\": <first sentence>, \"to\": \
         <last sentence>, \"confidence\": <0 to 100>}, sentences by their number. Kinds:\n\
         - claim: a statement the video argues for\n\
         - punchline: the line a joke or point lands on\n\
         - aside: a digression from the topic\n\
         - needs-source: a statement of fact that should cite a source\n\
         - topic-shift: where a new topic starts, \"from\" only\n\
         - product: a product or tool named, with \"name\": its name\n\
         Mark only what is clearly there; an empty answer is fine.\n\nSentences:\n",
    );
    for (n, sentence) in sentences(words).iter().enumerate() {
        let text: Vec<&str> = words[sentence.clone()]
            .iter()
            .map(|w| w.text.as_str())
            .collect();
        let _ = writeln!(out, "{n} {}", text.join(" "));
    }
    out
}

#[derive(Deserialize)]
struct Mark {
    kind: String,
    from: usize,
    to: Option<usize>,
    confidence: Option<u8>,
    name: Option<String>,
}

/// The elements in Claude's `answer` to [`prompt`] of `transcript`. Lines that are not a mark,
/// name a sentence that does not exist or a kind Claude does not mark are left out.
pub fn read_answer(transcript: &Transcript, answer: &str) -> Vec<Element> {
    let words = transcript.words();
    let sentences = sentences(words);
    let span = |from: usize, to: usize| {
        let (first, last) = (sentences.get(from)?, sentences.get(to)?);
        TimeRange::new(
            words[first.start].range.from(),
            words[last.end - 1].range.to(),
        )
    };
    answer
        .lines()
        .filter_map(|line| serde_json::from_str::<Mark>(line.trim()).ok())
        .filter_map(|mark| {
            let kind = ElementKind::parse(&mark.kind).filter(|k| KINDS.contains(k))?;
            let range = span(mark.from, mark.to.unwrap_or(mark.from).max(mark.from))?;
            let element = if kind.is_point() {
                Element::at(kind, range.from(), BY_CLAUDE)
            } else {
                Element::over(kind, range, BY_CLAUDE)
            };
            let element = match mark.confidence {
                Some(c) => element.with_confidence(c.min(100)),
                None => element,
            };
            match kind {
                ElementKind::Product => {
                    Some(element.with_text(Attr::Name, mark.name.filter(|n| !n.is_empty())?))
                }
                _ => Some(element),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Id, Micros, Word};

    #[test]
    fn the_prompt_numbers_sentences_and_the_answer_becomes_elements() {
        let mut transcript = Transcript::new(Id::new("t01").unwrap());
        for (i, text) in ["We", "ship.", "Fridays", "rock.", "Use", "Zed."]
            .iter()
            .enumerate()
        {
            let from = i as u64 * 1_000_000;
            transcript
                .push(Word {
                    text: (*text).to_owned(),
                    range: TimeRange::new(Micros(from), Micros(from + 500_000)).unwrap(),
                    confidence: 90,
                })
                .unwrap();
        }
        let prompt = prompt(&transcript);
        assert!(prompt.ends_with("0 We ship.\n1 Fridays rock.\n2 Use Zed.\n"));
        let answer = concat!(
            "Here you go:\n",
            "{\"kind\":\"claim\",\"from\":0,\"to\":1,\"confidence\":80}\n",
            "{\"kind\":\"topic-shift\",\"from\":2}\n",
            "{\"kind\":\"product\",\"from\":2,\"name\":\"Zed\"}\n",
            "{\"kind\":\"word\",\"from\":0}\n",
            "{\"kind\":\"aside\",\"from\":7}\n",
        );
        let lines: Vec<String> = read_answer(&transcript, answer)
            .into_iter()
            .map(|e| format!("{} {} {:?} {:?}", e.kind, e.t.0, e.to, e.confidence))
            .collect();
        assert_eq!(
            lines,
            [
                "claim 0 Some(Micros(3500000)) Some(80)",
                "topic-shift 4000000 None None",
                "product 4000000 Some(Micros(5500000)) None",
            ]
        );
    }
}
