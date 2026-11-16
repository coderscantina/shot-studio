//! The text element: lays out the blocks, then paints the marked lines, the selection, the text,
//! the misspelled words, the scrollbar and the caret.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, ContentMask, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Style, TextAlign, TextRun, Window, fill,
    font, point, px, relative, size,
};

use crate::EditorView;
use crate::layout::{BlockLayout, EditorStyle};

pub struct TextElement {
    view: Entity<EditorView>,
}

impl TextElement {
    pub fn new(view: Entity<EditorView>) -> Self {
        Self { view }
    }
}

/// How far up a line a misspelled word's underline stands, as a share of the line's height.
const MISSPELLED_RISE: f32 = 0.14;

/// Everything paint needs, gathered while the view is at hand.
pub struct Frame {
    /// The visible blocks with their window top.
    blocks: Vec<(Pixels, Rc<BlockLayout>)>,
    text_left: Pixels,
    selection: Vec<Bounds<Pixels>>,
    highlights: Vec<Bounds<Pixels>>,
    errors: Vec<Bounds<Pixels>>,
    misspelled: Vec<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
    thumb: Option<(Bounds<Pixels>, bool)>,
    style: EditorStyle,
    hitbox: Hitbox,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.flex_grow = 1.;
        style.min_size.height = px(0.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Frame {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        self.view.update(cx, |view, _| {
            view.set_bounds(bounds);
            view.lay_out(window);
            if let Some(offset) = view.scroll_to.take() {
                view.scroll_line_to_top(offset, window);
            } else if view.autoscroll {
                view.autoscroll = false;
                view.scroll_to_caret(window);
            }
            view.update_scrollbar();
            view.frame(hitbox, window)
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Frame,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        window.set_cursor_style(CursorStyle::IBeam, &frame.hitbox);
        let style = &frame.style;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for band in &frame.highlights {
                window.paint_quad(fill(*band, style.highlight).corner_radii(px(4.)));
            }
            for band in &frame.errors {
                window.paint_quad(fill(*band, style.error_background).corner_radii(px(4.)));
            }
            for rectangle in &frame.selection {
                window.paint_quad(fill(*rectangle, style.selection));
            }
            for (top, layout) in &frame.blocks {
                paint_lines(layout, frame.text_left, *top, style, window, cx);
            }
            for word in &frame.misspelled {
                let line = Bounds::new(
                    point(
                        word.left(),
                        word.bottom() - word.size.height * MISSPELLED_RISE,
                    ),
                    size(word.size.width, px(1.5)),
                );
                window.paint_quad(fill(line, style.misspelled));
            }
            if let Some((thumb, strong)) = frame.thumb {
                let color = style.thumb.opacity(if strong { 1. } else { 0.6 });
                window.paint_quad(fill(thumb, color).corner_radii(px(3.)));
            }
            if let Some(caret) = frame.caret {
                window.paint_quad(fill(caret, style.caret));
            }
        });

        let view = self.view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase.bubble() {
                view.update(cx, |view, cx| view.mouse_moved(event, window, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase.bubble() {
                view.update(cx, |view, cx| view.end_mouse(cx));
            }
        });
    }
}

/// Paints a block's lines with its top left corner at (`left`, `top`): bullets in the margin, the
/// text, and an error's message under its line.
fn paint_lines(
    layout: &BlockLayout,
    left: Pixels,
    top: Pixels,
    style: &EditorStyle,
    window: &mut Window,
    cx: &mut App,
) {
    for line in &layout.lines {
        if line.height == px(0.) {
            continue;
        }
        let top = top + line.y;
        if let Some((bullet, x)) = &line.bullet {
            paint_label(
                bullet.clone(),
                style.size,
                style.quiet,
                &style.font,
                point(left + *x, top),
                line.line_height,
                window,
                cx,
            );
        }
        line.shaped
            .paint(
                point(left + line.x, top),
                line.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
        if let Some(note) = &line.note {
            let y = top + line.line_height * line.rows.len() as f32;
            paint_label(
                note.clone(),
                style.size * 0.85,
                style.error,
                &style.font,
                point(left + line.x, y),
                style.note_height(),
                window,
                cx,
            );
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one short line of text and where it goes"
)]
fn paint_label(
    text: SharedString,
    text_size: Pixels,
    color: Hsla,
    family: &SharedString,
    origin: gpui::Point<Pixels>,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let run = TextRun {
        len: text.len(),
        font: font(family.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(text, text_size, &[run], None);
    shaped
        .paint(origin, line_height, TextAlign::Left, None, window, cx)
        .ok();
}

impl EditorView {
    fn frame(&mut self, hitbox: Hitbox, window: &Window) -> Frame {
        let mut blocks = Vec::new();
        let mut starts = Vec::new();
        for index in 0..self.blocks.as_slice().len() {
            let top = self.block_top(index);
            let layout = self.layout_block(index, window);
            if top + layout.height < self.bounds.top() || top > self.bounds.bottom() {
                continue;
            }
            starts.push(self.blocks.as_slice()[index].range.start);
            blocks.push((top, layout));
        }
        let shown = BlocksOnScreen {
            blocks: &blocks,
            starts: &starts,
            left: self.text_left,
            newline_width: self.style.size * 0.4,
        };
        let lines_of = |lines: &Range<usize>| {
            self.buffer.offset_of_line(lines.start)..self.buffer.offset_of_line(lines.end)
        };
        // Wide enough on the left for a heading's marker hanging in the margin.
        let band = |rectangle: Bounds<Pixels>| {
            let (left, right) = (self.style.size * 2.5, self.style.size * 0.5);
            Bounds::new(
                point(self.text_left - left, rectangle.top()),
                size(self.text_width + left + right, rectangle.size.height),
            )
        };
        let highlights = (self.highlights.iter())
            .flat_map(|lines| shown.line_bands(lines_of(lines)))
            .map(band)
            .collect();
        let errors = (self.error.iter())
            .flat_map(|(line, _)| shown.line_bands(lines_of(&(*line..line + 1))))
            .map(band)
            .collect();
        let selection = shown.rectangles(self.selection.range());
        let misspelled = (self.misspelled().into_iter())
            .flat_map(|word| shown.rectangles(word))
            .collect();
        let caret_on = self.caret_on && self.selection.is_empty() && self.focus.is_focused(window);
        let caret = caret_on.then(|| self.caret_bounds(window));
        Frame {
            blocks,
            text_left: self.text_left,
            selection,
            highlights,
            errors,
            misspelled,
            caret,
            thumb: (self.scrollbar)
                .map(|bar| (bar.thumb, self.thumb_hovered || self.thumb_grab.is_some())),
            style: self.style.clone(),
            hitbox,
        }
    }
}

/// The blocks on screen, for the rectangles that cover a range of the text.
struct BlocksOnScreen<'a> {
    blocks: &'a [(Pixels, Rc<BlockLayout>)],
    /// Each block's start in the text.
    starts: &'a [usize],
    left: Pixels,
    newline_width: Pixels,
}

impl BlocksOnScreen<'_> {
    /// The rectangles that cover `range`, row by row. A range that runs past a line's end takes a
    /// little room there.
    fn rectangles(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        if range.is_empty() {
            return Vec::new();
        }
        let mut rectangles = Vec::new();
        for ((top, layout), block_start) in self.blocks.iter().zip(self.starts) {
            for line in layout.lines.iter().filter(|line| !line.skipped) {
                let line_start = block_start + line.display.source.start;
                let line_end = block_start + line.display.source.end;
                if range.end < line_start || range.start > line_end {
                    continue;
                }
                let from = (line.display).to_display(
                    range
                        .start
                        .saturating_sub(*block_start)
                        .max(line.display.source.start),
                );
                let to = (line.display).to_display(
                    range
                        .end
                        .saturating_sub(*block_start)
                        .min(line.display.source.end),
                );
                let crosses_line_end = range.end > line_end;
                for (row, shown) in line.rows.iter().enumerate() {
                    let start = from.max(shown.start);
                    let end = to.min(shown.end);
                    let past_end = line.is_last_row(row) && crosses_line_end;
                    if start > end || (start == end && !past_end) {
                        continue;
                    }
                    let x0 = line.x_in_row(start, row);
                    let mut x1 = line.x_in_row(end, row);
                    if past_end {
                        x1 += self.newline_width;
                    }
                    rectangles.push(Bounds::new(
                        point(
                            self.left + line.x + x0,
                            *top + line.y + line.line_height * row as f32,
                        ),
                        size(x1 - x0, line.line_height),
                    ));
                }
            }
        }
        rectangles
    }

    /// One rectangle per shown line that starts in `range`, its full height with an error's
    /// message.
    fn line_bands(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut bands = Vec::new();
        for ((top, layout), block_start) in self.blocks.iter().zip(self.starts) {
            for line in layout.lines.iter().filter(|line| !line.skipped) {
                let start = block_start + line.display.source.start;
                if range.start <= start && start < range.end.max(range.start + 1) {
                    bands.push(Bounds::new(
                        point(self.left, *top + line.y),
                        size(px(0.), line.height),
                    ));
                }
            }
        }
        bands
    }
}
