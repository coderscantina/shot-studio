//! The syntax markers inside one block, as ranges relative to the block's start, so a view can hide
//! them.
//!
//! Emphasis, strong, strikethrough, code, and links come from pulldown-cmark, run over the block
//! alone. A reference link, `[text][label]` or `[text][]`, is a link here whatever the rest of the
//! file defines: its definition is another block, which this layer does not see. A lone `[label]`
//! stays text.

use std::ops::Range;

use pulldown_cmark::{BrokenLink, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};

use super::blocks::BlockKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Emphasis,
    Strong,
    Code,
    Link,
    /// `~~x~~`
    Strike,
}

/// One inline construct: `range` holds all of it, `open` and `close` its markers at either end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSpan {
    pub kind: SpanKind,
    pub range: Range<usize>,
    pub open: Range<usize>,
    pub close: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// `# ` or a closing ` #`, or a setext underline line.
    Heading,
    /// `> `
    Quote,
    /// `- `
    Bullet,
    /// `1. `
    Number,
}

/// A marker that belongs to a line of a block rather than to a span of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMarker {
    pub kind: MarkerKind,
    pub range: Range<usize>,
}

/// Whether a kind of block holds running text. The others stay raw.
pub fn has_inline(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Heading { .. }
            | BlockKind::Paragraph
            | BlockKind::List
            | BlockKind::Quote
            | BlockKind::FootnoteDefinition
    )
}

const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_GFM);

/// The inline spans of a block, sorted by start, outer spans before the spans inside them.
pub fn inline_spans(kind: BlockKind, text: &str) -> Vec<InlineSpan> {
    if !has_inline(kind) {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut open: Vec<(SpanKind, Range<usize>, usize)> = Vec::new();
    let mut last_inner_end = 0;
    let mut in_image = 0;
    // The definition of a reference link stands in another block, so the parser is told that one
    // exists.
    let defined = |link: BrokenLink<'_>| {
        matches!(link.link_type, LinkType::Reference | LinkType::Collapsed)
            .then(|| (CowStr::Borrowed(""), CowStr::Borrowed("")))
    };
    for (event, range) in
        Parser::new_with_broken_link_callback(text, OPTIONS, Some(defined)).into_offset_iter()
    {
        match event {
            Event::Start(Tag::Image { .. }) => in_image += 1,
            Event::End(TagEnd::Image) => in_image -= 1,
            _ if in_image > 0 => {}
            Event::Start(Tag::Emphasis) => open.push((SpanKind::Emphasis, range, 1)),
            Event::Start(Tag::Strong) => open.push((SpanKind::Strong, range, 2)),
            Event::Start(Tag::Strikethrough) => open.push((SpanKind::Strike, range, 2)),
            Event::Start(Tag::Link { link_type, .. }) => {
                // The parser ends a collapsed reference, `[text][]`, before its empty brackets.
                let collapsed =
                    matches!(link_type, LinkType::Collapsed | LinkType::CollapsedUnknown)
                        && text[range.end..].starts_with("[]");
                let end = range.end + if collapsed { "[]".len() } else { 0 };
                open.push((SpanKind::Link, range.start..end, 1));
                last_inner_end = 0;
            }
            Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough) => {
                if let Some((kind, whole, marker)) = open.pop() {
                    last_inner_end = last_inner_end.max(whole.end);
                    spans.push(InlineSpan {
                        kind,
                        open: whole.start..whole.start + marker,
                        close: whole.end - marker..whole.end,
                        range: whole,
                    });
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some((kind, whole, marker)) = open.pop() {
                    let inner_end = last_inner_end.max(whole.start + marker);
                    spans.push(InlineSpan {
                        kind,
                        open: whole.start..whole.start + marker,
                        close: inner_end..whole.end,
                        range: whole,
                    });
                }
            }
            Event::Code(_) => {
                let marker = text.as_bytes()[range.clone()]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                spans.push(InlineSpan {
                    kind: SpanKind::Code,
                    open: range.start..range.start + marker,
                    close: range.end - marker..range.end,
                    range: range.clone(),
                });
                last_inner_end = last_inner_end.max(range.end);
            }
            _ => {
                if !open.is_empty() {
                    last_inner_end = last_inner_end.max(range.end);
                }
            }
        }
    }
    spans.sort_by_key(|span| (span.range.start, std::cmp::Reverse(span.range.end)));
    spans
}

/// The line markers of a block.
pub fn line_markers(kind: BlockKind, text: &str) -> Vec<LineMarker> {
    match kind {
        BlockKind::Heading { .. } => heading_markers(text),
        BlockKind::Quote => per_line(text, quote_marker),
        BlockKind::List => per_line(text, list_marker),
        _ => Vec::new(),
    }
}

fn per_line(text: &str, find: impl Fn(&str) -> Option<LineMarker>) -> Vec<LineMarker> {
    let mut start = 0;
    let mut markers = Vec::new();
    for line in text.split_inclusive('\n') {
        if let Some(marker) = find(line) {
            markers.push(LineMarker {
                kind: marker.kind,
                range: start + marker.range.start..start + marker.range.end,
            });
        }
        start += line.len();
    }
    markers
}

fn leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn heading_markers(text: &str) -> Vec<LineMarker> {
    let marker = |range| LineMarker {
        kind: MarkerKind::Heading,
        range,
    };
    let first_end = text.find('\n').map_or(text.len(), |at| at + 1);
    let first = text[..first_end].trim_end_matches(['\n', '\r']);
    let spaces = leading_spaces(first);
    if first[spaces..].starts_with('#') {
        let hashes = first[spaces..]
            .bytes()
            .take_while(|byte| *byte == b'#')
            .count();
        let gap = first[spaces + hashes..]
            .bytes()
            .take_while(|byte| *byte == b' ' || *byte == b'\t')
            .count();
        let mut markers = vec![marker(0..spaces + hashes + gap)];
        let trimmed = first.trim_end();
        let body = trimmed.trim_end_matches('#');
        if body.len() < trimmed.len() && body.len() > spaces + hashes && body.ends_with([' ', '\t'])
        {
            markers.push(marker(body.trim_end().len()..first.len()));
        }
        return markers;
    }
    // A setext heading: the last line is the underline.
    let content = text.trim_end_matches(['\n', '\r']);
    let underline = content.rfind('\n').map_or(0, |at| at + 1);
    vec![marker(underline..content.len())]
}

fn quote_marker(line: &str) -> Option<LineMarker> {
    let mut end = leading_spaces(line);
    if end > 3 || !line[end..].starts_with('>') {
        return None;
    }
    while line[end..].starts_with('>') {
        end += 1;
        if line[end..].starts_with(' ') {
            end += 1;
        }
    }
    Some(LineMarker {
        kind: MarkerKind::Quote,
        range: 0..end,
    })
}

/// The marker itself and the gap after it. Leading spaces stay: they are the nesting.
fn list_marker(line: &str) -> Option<LineMarker> {
    let spaces = leading_spaces(line);
    let content = &line[spaces..];
    let bytes = content.as_bytes();
    let (kind, marker) = match bytes.first()? {
        b'-' | b'+' | b'*' => (MarkerKind::Bullet, 1),
        b'0'..=b'9' => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            (MarkerKind::Number, digits + 1)
        }
        _ => return None,
    };
    let rest = &content[marker..];
    if !rest.starts_with([' ', '\t']) && !rest.trim_end_matches(['\n', '\r']).is_empty() {
        return None;
    }
    let gap = rest
        .bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count();
    Some(LineMarker {
        kind,
        range: spaces..spaces + marker + gap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<(SpanKind, &str, &str, &str)> {
        inline_spans(BlockKind::Paragraph, text)
            .into_iter()
            .map(|span| {
                (
                    span.kind,
                    &text[span.range.clone()],
                    &text[span.open.clone()],
                    &text[span.close.clone()],
                )
            })
            .collect()
    }

    #[test]
    fn finds_emphasis_strong_and_code_with_their_markers() {
        use SpanKind::*;
        assert_eq!(
            spans("a *b* **c** `d` ``e`` ~~f~~"),
            [
                (Emphasis, "*b*", "*", "*"),
                (Strong, "**c**", "**", "**"),
                (Code, "`d`", "`", "`"),
                (Code, "``e``", "``", "``"),
                (Strike, "~~f~~", "~~", "~~"),
            ]
        );
    }

    #[test]
    fn a_reference_link_is_a_link_and_a_lone_label_is_text() {
        assert_eq!(
            spans("see [the *docs*][d] and [d][] but [d]")[..2],
            [
                (SpanKind::Link, "[the *docs*][d]", "[", "][d]"),
                (SpanKind::Emphasis, "*docs*", "*", "*"),
            ]
        );
        assert_eq!(
            spans("[d][] but [d]"),
            [(SpanKind::Link, "[d][]", "[", "][]")]
        );
    }

    #[test]
    fn nested_spans_come_outer_first() {
        use SpanKind::*;
        assert_eq!(
            spans("***x***"),
            [
                (Emphasis, "***x***", "*", "*"),
                (Strong, "**x**", "**", "**")
            ]
        );
    }

    #[test]
    fn a_link_hides_all_but_its_text() {
        assert_eq!(
            spans("see [the post](sources/never-friday.md) now")[0],
            (
                SpanKind::Link,
                "[the post](sources/never-friday.md)",
                "[",
                "](sources/never-friday.md)"
            )
        );
    }

    #[test]
    fn raw_blocks_have_no_spans() {
        for kind in [
            BlockKind::Code,
            BlockKind::Table,
            BlockKind::Comment,
            BlockKind::Raw,
        ] {
            assert!(inline_spans(kind, "*a* [b](c)").is_empty());
        }
    }

    #[test]
    fn heading_markers_cover_prefix_suffix_and_underline() {
        let heading = BlockKind::Heading { level: 2 };
        let ranges = |text: &str| -> Vec<(usize, usize)> {
            line_markers(heading, text)
                .into_iter()
                .map(|marker| (marker.range.start, marker.range.end))
                .collect()
        };
        assert_eq!(ranges("## Title\n"), [(0, 3)]);
        assert_eq!(ranges("## Title ##\n"), [(0, 3), (8, 11)]);
        assert_eq!(ranges("Title\n---\n"), [(6, 9)]);
        assert_eq!(ranges("#"), [(0, 1)]);
    }

    #[test]
    fn quote_and_list_markers_are_per_line() {
        let markers = line_markers(BlockKind::Quote, "> a\n>> b\nlazy\n");
        assert_eq!(
            markers
                .iter()
                .map(|marker| marker.range.clone())
                .collect::<Vec<_>>(),
            [0..2, 4..7]
        );
        let markers = line_markers(BlockKind::List, "- a\n  1. b\n\n  more\n");
        assert_eq!(
            markers
                .iter()
                .map(|marker| (marker.kind, marker.range.clone()))
                .collect::<Vec<_>>(),
            [(MarkerKind::Bullet, 0..2), (MarkerKind::Number, 6..9)]
        );
    }
}
