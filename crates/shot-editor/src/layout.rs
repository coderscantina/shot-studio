//! A block shaped for the text, and the geometry questions asked of it.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    Font, FontStyle, FontWeight, Hsla, LineLayout, Pixels, SharedString, StrikethroughStyle,
    TextRun, UnderlineStyle, Window, WrapBoundary, WrappedLine, WrappedLineLayout, font, px,
};

use crate::display::{DisplayLine, RunStyle};
use crate::source::BlockKind;

/// How the text looks. The host fills it from its theme.
#[derive(Clone)]
pub struct EditorStyle {
    pub font: SharedString,
    /// Code spans and raw blocks.
    pub mono: SharedString,
    pub size: Pixels,
    /// Line height as a multiple of the size.
    pub leading: f32,
    /// `#`, the title.
    pub title_size: Pixels,
    /// `##`, a chapter.
    pub chapter_size: Pixels,
    pub heading: Hsla,
    /// A bullet's line, a beat.
    pub text: Hsla,
    /// Paragraphs and the lines under a beat: its script.
    pub body: Hsla,
    /// Visible markers, bullets, and raw blocks.
    pub quiet: Hsla,
    pub link: Hsla,
    pub code_background: Hsla,
    pub caret: Hsla,
    pub selection: Hsla,
    /// Behind lines the host marks, such as the ones a proposal changes.
    pub highlight: Hsla,
    /// The message of a marked error and the tint behind its line.
    pub error: Hsla,
    pub error_background: Hsla,
    pub misspelled: Hsla,
    pub thumb: Hsla,
    /// The text never runs wider than this.
    pub max_width: Pixels,
    /// Room around the text inside the editor's bounds.
    pub padding: Pixels,
}

impl EditorStyle {
    fn base(&self, kind: BlockKind) -> LineStyle {
        let line = |family: &SharedString, size: Pixels, weight, color| LineStyle {
            font: Font {
                weight,
                ..font(family.clone())
            },
            size,
            line_height: size * self.leading,
            color,
        };
        match kind {
            BlockKind::Heading { level: 1 } => line(
                &self.font,
                self.title_size,
                FontWeight::SEMIBOLD,
                self.heading,
            ),
            BlockKind::Heading { level: 2 } => line(
                &self.font,
                self.chapter_size,
                FontWeight::SEMIBOLD,
                self.heading,
            ),
            BlockKind::Heading { .. } => {
                line(&self.font, self.size, FontWeight::SEMIBOLD, self.heading)
            }
            BlockKind::List => line(&self.font, self.size, FontWeight::NORMAL, self.text),
            BlockKind::Paragraph
            | BlockKind::Quote
            | BlockKind::FootnoteDefinition
            | BlockKind::Blank => line(&self.font, self.size, FontWeight::NORMAL, self.body),
            _ => line(&self.mono, self.size * 0.9, FontWeight::NORMAL, self.quiet),
        }
    }

    fn run(&self, base: &LineStyle, len: usize, style: RunStyle) -> TextRun {
        let family = if style.code {
            &self.mono
        } else {
            &base.font.family
        };
        let font = Font {
            weight: if style.bold {
                FontWeight::BOLD
            } else {
                base.font.weight
            },
            style: if style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..font(family.clone())
        };
        let color = if style.quiet {
            self.quiet
        } else if style.link {
            self.link
        } else {
            base.color
        };
        TextRun {
            len,
            font,
            color,
            background_color: style.code.then_some(self.code_background),
            underline: (style.marked || style.link).then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: style.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: Some(color),
            }),
        }
    }

    /// The room a single blank line between two blocks stands for, more above a heading.
    pub fn gap_before(&self, lower: Option<BlockKind>) -> Pixels {
        match lower {
            Some(BlockKind::Heading { .. }) => self.size * 1.6,
            _ => self.size * 0.7,
        }
    }

    /// The height of the error message under its line.
    pub fn note_height(&self) -> Pixels {
        self.size * 1.5
    }
}

struct LineStyle {
    font: Font,
    size: Pixels,
    line_height: Pixels,
    color: Hsla,
}

/// One source line, shaped and wrapped.
pub struct ShapedLine {
    pub display: DisplayLine,
    pub shaped: WrappedLine,
    pub rows: Vec<Range<usize>>,
    /// Distance from the block's top.
    pub y: Pixels,
    /// Distance from the text column's left edge.
    pub x: Pixels,
    pub line_height: Pixels,
    pub height: Pixels,
    /// A line the caret passes over: the blank between two blocks, or a hidden underline.
    pub skipped: bool,
    /// The bullet drawn in the margin, and its distance from the column's left edge.
    pub bullet: Option<(SharedString, Pixels)>,
    /// How far in the first row starts. Negative for a heading whose marker hangs into the margin.
    pub indent: Pixels,
    /// The error message shown under the line.
    pub note: Option<SharedString>,
}

impl ShapedLine {
    /// The row showing a displayed offset. At a wrap, `upstream` means the end of the upper row.
    pub fn row_of(&self, display: usize, upstream: bool) -> usize {
        let row = self
            .rows
            .iter()
            .position(|row| display < row.end)
            .unwrap_or(self.rows.len() - 1);
        if upstream && row > 0 && display == self.rows[row].start {
            row - 1
        } else {
            row
        }
    }

    /// The distance of a displayed offset from the start of its row.
    pub fn x_in_row(&self, display: usize, row: usize) -> Pixels {
        let layout = &self.shaped.unwrapped_layout;
        layout.x_for_index(display) - layout.x_for_index(self.rows[row].start) + self.row_left(row)
    }

    fn row_left(&self, row: usize) -> Pixels {
        if row == 0 { self.indent } else { px(0.) }
    }

    /// The displayed offset closest to `x` from the start of a row.
    pub fn display_at_x(&self, row: usize, x: Pixels) -> usize {
        let range = &self.rows[row];
        let layout = &self.shaped.unwrapped_layout;
        let start_x = layout.x_for_index(range.start);
        let x = (x - self.row_left(row)).max(px(0.));
        layout
            .closest_index_for_x(start_x + x)
            .clamp(range.start, range.end)
    }

    pub fn is_last_row(&self, row: usize) -> bool {
        row + 1 == self.rows.len()
    }
}

/// What a block was shaped for. A change to any of it means shaping again.
#[derive(Clone, PartialEq)]
pub struct ShapeKey {
    pub hidden: Vec<Range<usize>>,
    pub marked: Option<Range<usize>>,
    pub width: Pixels,
    /// The room a separator takes, zero for any other block.
    pub gap: Pixels,
    /// The line of the block that carries the error message, and the message.
    pub note: Option<(usize, SharedString)>,
}

pub struct BlockLayout {
    pub lines: Vec<ShapedLine>,
    pub height: Pixels,
    pub key: ShapeKey,
}

/// Shapes a block's display lines. A `gap` above zero makes the block a separator of that height.
pub fn shape_block(
    kind: BlockKind,
    lines: Vec<DisplayLine>,
    key: ShapeKey,
    style: &EditorStyle,
    window: &Window,
) -> BlockLayout {
    let base = style.base(kind);
    let hang = style.size * 1.25;
    let unit = hang * 0.5;
    let separator = key.gap > px(0.);
    let mut shaped_lines = Vec::with_capacity(lines.len());
    let mut y = px(0.);
    for (index, display) in lines.into_iter().enumerate() {
        let x = match kind {
            BlockKind::List if display.bullet.is_some() || display.hang > 0 => {
                unit * display.nesting as f32 + hang
            }
            BlockKind::List => unit * display.nesting as f32,
            BlockKind::Quote => hang,
            _ => px(0.),
        };
        let wrap = (key.width - x).max(style.size * 4.);
        let mut runs: Vec<TextRun> = (display.runs.iter())
            .map(|(len, run)| style.run(&base, *len, *run))
            .collect();
        if runs.is_empty() {
            runs.push(style.run(&base, 0, RunStyle::default()));
        }
        let text: SharedString = display.text.clone().into();
        let shape = |wrap_at| {
            window
                .text_system()
                .shape_text(text.clone(), base.size, &runs, wrap_at, None)
                .ok()
                .and_then(|mut lines| lines.pop())
                .unwrap_or_default()
        };
        // The toolkit wraps every row alike, so a line whose shown marker hangs into the margin is
        // broken here.
        let (shaped, indent) = if display.hang > 0 {
            let mut shaped = shape(None);
            let indent = px(0.) - shaped.unwrapped_layout.x_for_index(display.hang);
            let breaks = breaks_within(&shaped, indent, wrap);
            indent_rows(&mut shaped, &breaks, indent);
            (shaped, indent)
        } else {
            (shape(Some(wrap)), px(0.))
        };
        let rows = rows_of(&shaped);
        let collapsed = display.is_collapsed();
        let note = (key.note.as_ref())
            .filter(|(line, _)| *line == index)
            .map(|(_, message)| message.clone());
        let line_height = if separator { key.gap } else { base.line_height };
        let height = if collapsed {
            px(0.)
        } else if separator {
            key.gap
        } else {
            line_height * rows.len() as f32
                + if note.is_some() {
                    style.note_height()
                } else {
                    px(0.)
                }
        };
        let bullet = (display.bullet.clone())
            .map(|text| (SharedString::from(text), unit * display.nesting as f32));
        shaped_lines.push(ShapedLine {
            display,
            shaped,
            rows,
            y,
            x,
            line_height,
            height,
            skipped: separator || collapsed,
            bullet,
            indent,
            note,
        });
        y += height;
    }
    BlockLayout {
        lines: shaped_lines,
        height: y,
        key,
    }
}

/// Where a line breaks with its first row starting `indent` in: at the last space that fits each
/// row, or inside a word with no space to break at. A space that runs past the end hangs there, as
/// the toolkit's own wrapping has it.
fn breaks_within(shaped: &WrappedLine, indent: Pixels, width: Pixels) -> Vec<usize> {
    let line = &shaped.unwrapped_layout;
    let bytes = shaped.text.as_bytes();
    let glyphs: Vec<usize> = (line.runs.iter())
        .flat_map(|run| &run.glyphs)
        .map(|glyph| glyph.index)
        .collect();
    let mut breaks = Vec::new();
    let mut start = 0;
    let mut room = width - indent;
    let mut last_space = None;
    for (at, index) in glyphs.iter().copied().enumerate() {
        let is_space = bytes.get(index) == Some(&b' ');
        let end = glyphs.get(at + 1).copied().unwrap_or(line.len);
        if !is_space && index > start && line.x_for_index(end) - line.x_for_index(start) > room {
            let cut = last_space.filter(|space| *space > start).unwrap_or(index);
            breaks.push(cut);
            start = cut;
            room = width;
            last_space = None;
        }
        if is_space {
            last_space = Some(end);
        }
    }
    breaks
}

/// Breaks a shaped line at `breaks`, with the first row starting `indent` in. Positions stay rising
/// from row to row, as the caret's lookups expect, and rows are measured from their first glyph.
fn indent_rows(shaped: &mut WrappedLine, breaks: &[usize], indent: Pixels) {
    let line = &shaped.unwrapped_layout;
    let mut runs = line.runs.clone();
    let mut boundaries = Vec::new();
    let mut row = 0;
    for (run_ix, run) in runs.iter_mut().enumerate() {
        for (glyph_ix, glyph) in run.glyphs.iter_mut().enumerate() {
            if breaks.get(row).is_some_and(|at| glyph.index >= *at) {
                row += 1;
                boundaries.push(WrapBoundary { run_ix, glyph_ix });
            }
            glyph.position.x += indent;
        }
    }
    let layout = WrappedLineLayout {
        unwrapped_layout: Arc::new(LineLayout {
            font_size: line.font_size,
            width: line.width + indent,
            ascent: line.ascent,
            descent: line.descent,
            runs,
            len: line.len,
        }),
        wrap_boundaries: boundaries.into_iter().collect(),
        wrap_width: None,
    };
    **shaped = Arc::new(layout);
}

fn rows_of(shaped: &WrappedLine) -> Vec<Range<usize>> {
    let layout = &shaped.unwrapped_layout;
    let mut rows = Vec::with_capacity(shaped.wrap_boundaries.len() + 1);
    let mut start = 0;
    for boundary in &shaped.wrap_boundaries {
        let end = layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
        rows.push(start..end);
        start = end;
    }
    rows.push(start..shaped.len());
    rows
}

#[cfg(test)]
mod tests {
    use gpui::{FontId, GlyphId, ShapedGlyph, ShapedRun, point};

    use super::*;
    use crate::display::{Syntax, display_lines};

    /// A line shaped by hand: every byte is a glyph 10 px wide.
    fn shaped(text: &str) -> WrappedLine {
        let glyphs = (0..text.len())
            .map(|index| ShapedGlyph {
                id: GlyphId(0),
                position: point(px(index as f32 * 10.), px(0.)),
                index,
                is_emoji: false,
            })
            .collect();
        let layout = LineLayout {
            font_size: px(20.),
            width: px(text.len() as f32 * 10.),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                glyphs,
            }],
            len: text.len(),
            ..LineLayout::default()
        };
        let mut wrapped = WrappedLine::default();
        wrapped.text = text.to_owned().into();
        *wrapped = Arc::new(WrappedLineLayout {
            unwrapped_layout: Arc::new(layout),
            wrap_boundaries: Default::default(),
            wrap_width: None,
        });
        wrapped
    }

    fn source_line(text: &str, indent: Pixels, width: Pixels) -> ShapedLine {
        let mut line = shaped(text);
        let breaks = breaks_within(&line, indent, width);
        indent_rows(&mut line, &breaks, indent);
        let display = display_lines(
            BlockKind::Paragraph,
            text,
            &Syntax::default(),
            &[],
            None,
            false,
        )
        .remove(0);
        ShapedLine {
            rows: rows_of(&line),
            display,
            shaped: line,
            y: px(0.),
            x: px(0.),
            line_height: px(20.),
            height: px(40.),
            skipped: false,
            bullet: None,
            indent,
            note: None,
        }
    }

    #[test]
    fn an_indented_first_row_has_less_room_and_the_caret_keeps_its_place_on_both_sides_of_a_break()
    {
        // 90 px hold nine letters, or six after a 30 px indent.
        let line = source_line("aaaa bbbb cccc", px(30.), px(90.));
        assert_eq!(line.rows, [0..5, 5..14]);
        assert_eq!(line.x_in_row(0, 0), px(30.));
        assert_eq!(line.x_in_row(5, 1), px(0.));
        assert_eq!(line.row_of(5, false), 1);
        assert_eq!(line.row_of(5, true), 0);
        for (at, row) in [(0, 0), (2, 0), (5, 0), (5, 1), (9, 1)] {
            assert_eq!(
                line.display_at_x(row, line.x_in_row(at, row)),
                at,
                "offset {at} in row {row}"
            );
        }
    }

    #[test]
    fn a_hanging_marker_gives_the_first_row_more_room() {
        let line = source_line("## aaaa bbbb", px(-30.), px(90.));
        assert_eq!(line.rows.len(), 1);
    }

    #[test]
    fn a_word_with_no_space_breaks_where_the_room_ends() {
        let line = shaped("aaaaaaaaaaaa");
        assert_eq!(breaks_within(&line, px(20.), px(50.)), [3, 8]);
    }
}
