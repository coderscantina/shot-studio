//! Spelling in the editor: what is checked, a cache by each block's text, and the answers coming
//! back. The check runs off the main thread behind [`crate::spell::Checker`], only when the writer
//! pauses. A script is short, so every block is checked.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{Context, Task};

use crate::EditorView;
use crate::source::BlockKind;
use crate::source::inline::{SpanKind, inline_spans, line_markers};
use crate::spell::{self, Checker};

/// How long after the last key the blocks are checked.
const PAUSE: Duration = Duration::from_millis(500);
/// How many suggestions the menu offers.
pub const SUGGESTIONS: usize = 6;

/// A block's text as a key, so an answer stays good for any block that did not change.
fn key(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// A block's text as the checker sees it: without markers and what is not prose, with the way back
/// to the block's bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Prose {
    pub text: String,
    /// Where each kept piece starts, in the checked text and in the block.
    pieces: Vec<(usize, usize)>,
}

impl Prose {
    /// The block's bytes for a range of the checked text. A word that crosses a hidden marker takes
    /// the marker in.
    pub fn to_block(&self, range: Range<usize>) -> Range<usize> {
        self.place(range.start)..self.place(range.end - 1) + 1
    }

    fn place(&self, at: usize) -> usize {
        let index = self
            .pieces
            .partition_point(|piece| piece.0 <= at)
            .saturating_sub(1);
        let (clean, source) = self.pieces[index];
        source + at - clean
    }
}

/// What to check of a block, or `None` for what is not prose. Left out are line markers, inline
/// markers, code spans, link targets, and URLs and addresses.
pub fn prose_of(kind: BlockKind, text: &str) -> Option<Prose> {
    let checked = matches!(
        kind,
        BlockKind::Paragraph | BlockKind::Heading { .. } | BlockKind::List | BlockKind::Quote
    );
    if !checked {
        return None;
    }
    let mut left_out: Vec<Range<usize>> = line_markers(kind, text)
        .into_iter()
        .map(|marker| marker.range)
        .collect();
    for span in inline_spans(kind, text) {
        match span.kind {
            SpanKind::Code => left_out.push(span.range),
            _ => {
                left_out.push(span.open);
                left_out.push(span.close);
            }
        }
    }
    left_out.extend(addresses(text));
    left_out.retain(|range| !range.is_empty());
    left_out.sort_by_key(|range| range.start);
    let mut prose = Prose {
        text: String::with_capacity(text.len()),
        pieces: Vec::with_capacity(left_out.len() + 1),
    };
    let mut at = 0;
    for range in left_out
        .iter()
        .chain(std::iter::once(&(text.len()..text.len())))
    {
        if range.start > at {
            prose.pieces.push((prose.text.len(), at));
            prose.text.push_str(&text[at..range.start]);
        }
        at = at.max(range.end);
    }
    Some(prose)
}

/// Words that are URLs or addresses.
fn addresses(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut at = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        if trimmed.contains("://") || trimmed.starts_with("www.") || trimmed.contains('@') {
            found.push(at..at + trimmed.len());
        }
        at += word.len();
    }
    found
}

/// What the editor knows of spelling.
pub struct Spelling {
    pub(crate) checker: Rc<dyn Checker>,
    /// The misspelled words of each block's text, as bytes of the block.
    found: HashMap<u64, Rc<[Range<usize>]>>,
    asked: HashSet<u64>,
    pause: Option<Task<()>>,
    results: UnboundedSender<(u64, Vec<Range<usize>>)>,
    _inbox: Task<()>,
}

impl Spelling {
    pub fn new(cx: &mut Context<EditorView>) -> Self {
        let (results, mut inbox) = unbounded();
        let task = cx.spawn(async move |this, cx| {
            while let Some(result) = inbox.next().await {
                let mut batch = vec![result];
                while let Ok(more) = inbox.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this: &mut EditorView, cx| {
                        this.spelling_results(batch, cx)
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            checker: spell::system(),
            found: HashMap::new(),
            asked: HashSet::new(),
            pause: None,
            results,
            _inbox: task,
        }
    }
}

impl EditorView {
    /// Waits out the pause again. The only thing that happens on a key.
    pub(crate) fn restart_spelling(&mut self, cx: &mut Context<Self>) {
        self.spelling.pause = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PAUSE).await;
            this.update(cx, |this, _| {
                this.spelling.pause = None;
                this.check_blocks();
            })
            .ok();
        }));
    }

    /// Asks the checker about every block with no answer.
    pub(crate) fn check_blocks(&mut self) {
        let text = self.buffer.text();
        for block in self.blocks.as_slice() {
            let block_text = &text[block.range.clone()];
            let key = key(block_text);
            if self.spelling.found.contains_key(&key) || !self.spelling.asked.insert(key) {
                continue;
            }
            let Some(prose) = prose_of(block.kind, block_text) else {
                self.spelling.found.insert(key, Rc::from(Vec::new()));
                self.spelling.asked.remove(&key);
                continue;
            };
            let results = self.spelling.results.clone();
            let checked = prose.text.clone();
            self.spelling.checker.check(
                checked,
                Box::new(move |ranges| {
                    let ranges = ranges
                        .into_iter()
                        .map(|range| prose.to_block(range))
                        .collect();
                    results.unbounded_send((key, ranges)).ok();
                }),
            );
        }
    }

    /// Takes the checker's answers, on the main thread.
    fn spelling_results(&mut self, batch: Vec<(u64, Vec<Range<usize>>)>, cx: &mut Context<Self>) {
        let mut shows = false;
        for (key, ranges) in batch {
            self.spelling.asked.remove(&key);
            shows |= !ranges.is_empty();
            self.spelling.found.insert(key, Rc::from(ranges));
        }
        if shows {
            cx.notify();
        }
    }

    /// The misspelled words of the text, in its offsets.
    pub(crate) fn misspelled(&self) -> Vec<Range<usize>> {
        let text = self.buffer.text();
        let mut ranges = Vec::new();
        for block in self.blocks.as_slice() {
            if let Some(found) = self.spelling.found.get(&key(&text[block.range.clone()])) {
                ranges.extend(
                    found.iter().map(|range| {
                        block.range.start + range.start..block.range.start + range.end
                    }),
                );
            }
        }
        ranges
    }

    /// Forgets every answer and asks again, after the dictionary learned a word.
    pub(crate) fn recheck(&mut self) {
        self.spelling.found.clear();
        self.check_blocks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked(kind: BlockKind, text: &str) -> String {
        prose_of(kind, text).expect("prose").text
    }

    #[test]
    fn code_and_other_blocks_that_are_not_prose_are_not_checked() {
        for kind in [
            BlockKind::Code,
            BlockKind::Table,
            BlockKind::Comment,
            BlockKind::Raw,
        ] {
            assert_eq!(prose_of(kind, "teh words"), None);
        }
    }

    #[test]
    fn markers_code_link_targets_and_urls_are_left_out_and_words_stay_whole() {
        assert_eq!(
            checked(
                BlockKind::List,
                "- A **fet**ty `kodee` word [the pots](sources/pots.md) at https://exampel.org.\n"
            ),
            "A fetty  word the pots at \n"
        );
    }

    #[test]
    fn a_heading_and_a_list_lose_their_line_markers() {
        assert_eq!(
            checked(BlockKind::Heading { level: 2 }, "## Hook\n"),
            "Hook\n"
        );
        assert_eq!(checked(BlockKind::List, "- one\n- two\n"), "one\ntwo\n");
    }

    #[test]
    fn a_found_word_maps_back_to_the_blocks_bytes_past_what_was_left_out() {
        let text = "Ä `code` Wört **fet**t";
        let prose = prose_of(BlockKind::Paragraph, text).expect("prose");
        let word = prose.text.find("Wört").expect("kept");
        assert_eq!(&text[prose.to_block(word..word + "Wört".len())], "Wört");
        let word = prose.text.find("fett").expect("joined across the marker");
        assert_eq!(&text[prose.to_block(word..word + "fett".len())], "fet**t");
    }
}
