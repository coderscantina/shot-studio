//! Round trips and incremental segmentation over the contract's example `script.md` and a long
//! script made from it.

use std::ops::Range;
use std::path::Path;
use std::time::{Duration, Instant};

use pulldown_cmark::{Event, Options, Parser, Tag};

use super::blocks::BlockChange;
use super::*;

/// The example in the `script.md` section of the project files contract, byte for byte.
fn contract_example() -> String {
    let contract =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/contracts/project-files.md");
    let text = std::fs::read_to_string(&contract)
        .unwrap_or_else(|error| panic!("{}: {error}", contract.display()));
    let section = &text[text
        .find("## `script.md`")
        .expect("the contract has a script.md section")..];
    let fence = "```markdown\n";
    let start = section.find(fence).expect("the section has an example") + fence.len();
    let end = start
        + section[start..]
            .find("\n```")
            .expect("the example is closed")
        + 1;
    section[start..end].to_owned()
}

/// The example, and a script of about 10,000 words made by repeating its chapters.
fn documents() -> Vec<String> {
    let example = contract_example();
    let chapters = &example[example.find("## ").expect("the example has chapters")..];
    let mut long = example.clone();
    while long.split_whitespace().count() < 10_000 {
        long.push('\n');
        long.push_str(chapters);
    }
    vec![example, long]
}

fn replace(
    buffer: &mut Buffer,
    blocks: &mut Blocks,
    range: Range<usize>,
    text: &str,
) -> BlockChange {
    let caret = Selection::caret(range.start);
    let splice = buffer
        .transact(
            EditKind::Discrete,
            vec![Edit::new(range, text)],
            caret,
            caret,
            Instant::now(),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    blocks.apply(buffer.text(), splice)
}

#[test]
fn the_contract_example_round_trips_byte_for_byte_through_an_edit_and_its_undo() {
    let example = contract_example();
    let mut buffer = Buffer::new(example.clone());
    let mut blocks = Blocks::parse(buffer.text());
    let joined: String = blocks
        .as_slice()
        .iter()
        .map(|block| &buffer.text()[block.range.clone()])
        .collect();
    assert_eq!(joined, example);

    let at = example
        .find("And nothing burns.")
        .expect("the example's last script line");
    replace(&mut buffer, &mut blocks, at..at, "Really, ");
    assert!(buffer.text().contains("Really, And nothing burns."));
    let undone = buffer.undo().expect("the edit is on the undo stack");
    blocks.apply(buffer.text(), undone.splice);
    assert_eq!(buffer.text().as_bytes(), example.as_bytes());
    assert_eq!(blocks, Blocks::parse(&example));
}

#[test]
fn an_edit_in_one_block_leaves_all_other_block_bytes_identical() {
    for original in documents() {
        let mut buffer = Buffer::new(original.clone());
        let mut blocks = Blocks::parse(buffer.text());
        let before = blocks.clone();
        let lists: Vec<usize> = (before.as_slice().iter().enumerate())
            .filter(|(_, block)| block.kind == BlockKind::List)
            .map(|(index, _)| index)
            .collect();
        let target = lists[lists.len() / 2];
        let at = before.as_slice()[target].range.start + "- ".len();
        replace(&mut buffer, &mut blocks, at..at, "Inserted ");

        let after = blocks.as_slice();
        assert_eq!(after.len(), before.as_slice().len());
        for (index, (old, new)) in before.as_slice().iter().zip(after).enumerate() {
            let old_text = &original[old.range.clone()];
            let new_text = &buffer.text()[new.range.clone()];
            if index == target {
                assert_eq!(new_text.replacen("Inserted ", "", 1), old_text);
            } else {
                assert_eq!(new_text, old_text, "block {index}");
            }
        }
    }
}

/// A small deterministic generator, so a failure reproduces.
struct Random(u64);

impl Random {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % below as u64) as usize
    }
}

fn boundary_near(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

const INSERTS: &[&str] = &[
    "x", "# ", "## ", "> ", "- ", "* ", "1. ", "\n", "\n\n", "```\n", "<!-- ", " -->", "---\n",
    "***\n", "    ", "[a](b)", "===\n", "<div>\n", "ä", "\r\n", "\r",
];

#[test]
fn incremental_updates_always_match_a_full_parse() {
    let mut random = Random(0x2545_F491_4F6C_DD1D);
    for document in documents() {
        let end = boundary_near(&document, document.len().min(8000));
        let mut buffer = Buffer::new(&document[..end]);
        let mut blocks = Blocks::parse(buffer.text());
        for _ in 0..300 {
            let len = buffer.text().len();
            let start = boundary_near(buffer.text(), random.next(len + 1));
            let (range, insert) = if random.next(3) == 0 {
                let end = boundary_near(buffer.text(), (start + random.next(40)).min(len));
                (start..end, "")
            } else {
                (start..start, INSERTS[random.next(INSERTS.len())])
            };
            let old = buffer.text().to_owned();
            replace(&mut buffer, &mut blocks, range.clone(), insert);
            let full = Blocks::parse(buffer.text());
            assert!(
                blocks == full,
                "replacing {range:?} with {insert:?} diverged\nold: {old:?}\nincremental: {blocks:?}\nfull: {full:?}",
            );
        }
    }
}

/// Every top-level block pulldown-cmark sees starts a block of the same family here.
#[test]
fn block_starts_agree_with_pulldown_cmark() {
    for text in documents() {
        let blocks = Blocks::parse(&text);
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES | Options::ENABLE_STRIKETHROUGH;
        let mut depth = 0;
        for (event, range) in Parser::new_ext(&text, options).into_offset_iter() {
            match event {
                Event::Start(tag) => {
                    if depth == 0 {
                        let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
                        let block = &blocks.as_slice()[blocks.index_at(line_start)];
                        assert_eq!(
                            block.range.start, line_start,
                            "{tag:?} starts inside {:?}",
                            block.kind
                        );
                        assert!(
                            family_matches(&tag, block.kind),
                            "{tag:?} is {:?}",
                            block.kind
                        );
                    }
                    depth += 1;
                }
                Event::End(_) => depth -= 1,
                _ => {}
            }
        }
    }
}

fn family_matches(tag: &Tag, kind: BlockKind) -> bool {
    match tag {
        Tag::Paragraph => kind == BlockKind::Paragraph,
        Tag::Heading { .. } => matches!(kind, BlockKind::Heading { .. }),
        Tag::List(_) => kind == BlockKind::List,
        Tag::BlockQuote(_) => kind == BlockKind::Quote,
        Tag::CodeBlock(_) => kind == BlockKind::Code,
        _ => true,
    }
}

/// The budget is a frame at 120 Hz, so this holds in an unoptimized build.
#[test]
fn an_edit_resegments_a_ten_thousand_word_script_far_below_one_frame() {
    let long = documents().pop().expect("the long script");
    let mut buffer = Buffer::new(long);
    let mut blocks = Blocks::parse(buffer.text());
    let mut worst = Duration::ZERO;
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    for _ in 0..500 {
        let lists: Vec<usize> = (blocks.as_slice().iter())
            .filter(|block| block.kind == BlockKind::List)
            .map(|block| block.range.start + "- ".len())
            .collect();
        let at = lists[random.next(lists.len())];
        let started = Instant::now();
        let caret = Selection::caret(at);
        let splice = buffer
            .transact(
                EditKind::Typing,
                vec![Edit::new(at..at, "x")],
                caret,
                Selection::caret(at + 1),
                Instant::now(),
            )
            .unwrap_or_else(|error| panic!("{error}"));
        blocks.apply(buffer.text(), splice);
        worst = worst.max(started.elapsed());
    }
    assert!(
        worst < Duration::from_millis(8),
        "worst edit took {worst:?}"
    );
}
