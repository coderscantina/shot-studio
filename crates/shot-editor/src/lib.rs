//! The script editor: `script.md` as live-styled Markdown source, written in place.
//!
//! The text core in `source/` and the line display in `display.rs` are a copy of Druck's
//! (`druck/cli/desktop/src/source/` and `editor/display.rs` at druck c19da2c, same author, MIT),
//! cut to what a script uses: Druck's directives, citations, footnotes, placeholders and marks are
//! gone. The spell checker in `spell/` and `spelling.rs` is Druck's too. The view, its element and
//! its layout are written for this crate after Druck's `editor/` module, much smaller: a script is
//! short, so every block is shaped and kept, and the view scrolls by pixels.
//!
//! [`EditorView`] shows headings large, bullets as beats with the marker in the margin, and a link
//! as its label, each with its source markers hidden until the caret is on them. It has a caret and
//! selection by mouse and keys, input methods, undo, soft wrap, a drawn scrollbar, Enter continuing
//! a bullet, Cmd+K for a link, copy and paste, and misspelled words underlined with suggestions on
//! a right click. It emits [`Changed`] on every edit and never writes a file: the host reads
//! [`EditorView::text`] and saves it.

mod display;
mod element;
mod input;
mod keys;
mod layout;
mod scrollbar;
mod selection;
mod source;
mod spell;
mod spelling;

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels, Point,
    ScrollEvent, SharedString, Task, WeakEntity, Window, div, point, px, size,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};

use display::{Syntax, display_lines, hidden_ranges};
use keys::*;
use layout::{BlockLayout, ShapeKey, shape_block};
use source::inline::{inline_spans, line_markers};
use source::{BlockKind, Blocks, Buffer, Edit, EditKind, Selection, motion};
use spelling::Spelling;

pub use keys::bind_keys;
pub use layout::EditorStyle;

/// The text changed by an edit, an undo or a redo. Not sent for [`EditorView::set_text_external`].
pub struct Changed;

const BLINK: Duration = Duration::from_millis(500);
/// Room the caret keeps from the editor's top and bottom edge when the view scrolls to it.
const CARET_MARGIN: Pixels = px(24.);
const SCROLLBAR_WIDTH: Pixels = px(10.);
const THUMB_MIN: Pixels = px(28.);

/// The cached derivations of one block. A change to the block replaces its slot.
#[derive(Default)]
struct Slot {
    syntax: Option<Rc<Syntax>>,
    layout: Option<Rc<BlockLayout>>,
}

/// A mouse selection in progress. `unit` is what a double or triple click started with, which a
/// drag extends by.
#[derive(Clone)]
struct Drag {
    origin: Range<usize>,
    unit: DragUnit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragUnit {
    Character,
    Word,
    Line,
}

#[derive(Clone, Copy)]
struct Scrollbar {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
}

/// A caret place in the shaped text.
#[derive(Clone, Copy)]
struct Located {
    block: usize,
    line: usize,
    row: usize,
    display: usize,
}

pub struct EditorView {
    buffer: Buffer,
    blocks: Blocks,
    slots: Vec<Slot>,
    /// Each block's top, from the text's top, as last laid out.
    tops: Vec<Pixels>,
    content_height: Pixels,
    style: EditorStyle,
    selection: Selection,
    /// The caret shows at the end of the upper row of a wrap.
    upstream: bool,
    /// The column up and down aim for.
    goal_x: Option<Pixels>,
    /// Marked text of an input method.
    marked: Option<Range<usize>>,
    /// Where markers reveal during a drag, so nothing moves under the pointer.
    reveal_at: usize,
    focus: FocusHandle,
    caret_on: bool,
    blink: Task<()>,
    /// The element's bounds, and the text column inside them, in window coordinates.
    bounds: Bounds<Pixels>,
    text_left: Pixels,
    text_width: Pixels,
    /// How far the text is scrolled up.
    scroll: Pixels,
    /// Bring the caret into view at the next layout.
    autoscroll: bool,
    /// Bring this offset's line to the top at the next layout.
    scroll_to: Option<usize>,
    drag: Option<Drag>,
    pointer: Point<Pixels>,
    scrollbar: Option<Scrollbar>,
    thumb_grab: Option<Pixels>,
    thumb_hovered: bool,
    /// A 1-based line marked as wrong, with why.
    error: Option<(usize, SharedString)>,
    /// 1-based line ranges, end exclusive, shown marked.
    highlights: Vec<Range<usize>>,
    spelling: Spelling,
    /// The misspelled word a right click landed on, for the context menu.
    menu_word: Option<Range<usize>>,
}

impl EventEmitter<Changed> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EditorView {
    pub fn new(text: &str, style: EditorStyle, cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::new(text);
        let blocks = Blocks::parse(buffer.text());
        let slots = blocks.as_slice().iter().map(|_| Slot::default()).collect();
        let mut view = Self {
            buffer,
            blocks,
            slots,
            tops: Vec::new(),
            content_height: px(0.),
            style,
            selection: Selection::caret(text.len()),
            upstream: false,
            goal_x: None,
            marked: None,
            reveal_at: text.len(),
            focus: cx.focus_handle(),
            caret_on: true,
            blink: Task::ready(()),
            bounds: Bounds::default(),
            text_left: px(0.),
            text_width: px(0.),
            scroll: px(0.),
            autoscroll: false,
            scroll_to: None,
            drag: None,
            pointer: Point::default(),
            scrollbar: None,
            thumb_grab: None,
            thumb_hovered: false,
            error: None,
            highlights: Vec::new(),
            spelling: Spelling::new(cx),
            menu_word: None,
        };
        view.check_blocks();
        view
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replaces the text with a change from outside, such as the file changing on disk, as one undo
    /// step. Only the part that differs is replaced, so the caret stays where it was around it.
    pub fn set_text_external(&mut self, text: &str, cx: &mut Context<Self>) {
        let old = self.buffer.text();
        if old == text {
            return;
        }
        let prefix = common_prefix(old, text);
        let suffix = common_suffix(&old[prefix..], &text[prefix..]);
        let removed = prefix..old.len() - suffix;
        let inserted = &text[prefix..text.len() - suffix];
        let map = |offset: usize| {
            if offset <= removed.start {
                offset
            } else if offset >= removed.end {
                offset - removed.len() + inserted.len()
            } else {
                prefix + inserted.len()
            }
        };
        let after = Selection {
            anchor: map(self.selection.anchor),
            head: map(self.selection.head),
        };
        let edit = Edit::new(removed, inserted);
        let Ok(splice) =
            (self.buffer).transact(EditKind::External, vec![edit], self.selection, after, now())
        else {
            return;
        };
        self.resegment(splice);
        self.marked = None;
        self.selection = after;
        self.check_blocks();
        cx.notify();
    }

    /// Marks a 1-based line as wrong and shows `message` under it, or clears the mark.
    pub fn set_error(&mut self, error: Option<(usize, SharedString)>, cx: &mut Context<Self>) {
        if self.error != error {
            self.error = error;
            cx.notify();
        }
    }

    /// Marks the 1-based line ranges, end exclusive, such as the lines a proposal changes.
    pub fn set_highlights(&mut self, lines: Vec<Range<usize>>, cx: &mut Context<Self>) {
        if self.highlights != lines {
            self.highlights = lines;
            cx.notify();
        }
    }

    /// Puts the caret at the start of a 1-based line, shows that line at the top and gives the
    /// editor the keys.
    pub fn go_to_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let start = self.buffer.offset_of_line(line);
        let offset = selection::past_line_marker(self.buffer.text(), &self.blocks, start);
        window.focus(&self.focus, cx);
        self.set_selection(Selection::caret(offset), false, window, cx);
        self.autoscroll = false;
        self.scroll_to = Some(start);
    }

    // Layout and geometry

    /// Places the text column in the element's bounds.
    fn set_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.bounds = bounds;
        let room = (bounds.size.width - self.style.padding * 2. - SCROLLBAR_WIDTH)
            .max(self.style.size * 8.);
        self.text_width = room.min(self.style.max_width);
        self.text_left = bounds.left() + self.style.padding + (room - self.text_width) * 0.5;
    }

    /// The window y of the text's top.
    fn text_top(&self) -> Pixels {
        self.bounds.top() + self.style.padding - self.scroll
    }

    fn block_top(&self, index: usize) -> Pixels {
        self.text_top() + self.tops.get(index).copied().unwrap_or_default()
    }

    fn max_scroll(&self) -> Pixels {
        (self.content_height + self.style.padding * 2. - self.bounds.size.height).max(px(0.))
    }

    fn scroll_by(&mut self, delta: Pixels) {
        self.scroll = (self.scroll + delta).clamp(px(0.), self.max_scroll());
    }

    /// Shapes every block that changed and stacks them.
    fn lay_out(&mut self, window: &Window) {
        self.tops.clear();
        let mut y = px(0.);
        for index in 0..self.blocks.as_slice().len() {
            self.tops.push(y);
            y += self.layout_block(index, window).height;
        }
        self.content_height = y;
        self.scroll = self.scroll.clamp(px(0.), self.max_scroll());
    }

    /// Where markers reveal: at the caret of a focused editor, and at the start of a drag. A
    /// selection shows its text clean, so nothing moves under it.
    fn reveal_offset(&self, window: &Window) -> Option<usize> {
        if !self.focus.is_focused(window) {
            None
        } else if self.drag.is_some() {
            Some(self.reveal_at)
        } else {
            self.selection.is_empty().then_some(self.selection.head)
        }
    }

    fn layout_block(&mut self, index: usize, window: &Window) -> Rc<BlockLayout> {
        let block = self.blocks.as_slice()[index].clone();
        let list = self.blocks.as_slice();
        let text = self.buffer.text();
        let block_text = &text[block.range.clone()];
        let trailing_line = index + 1 == list.len() && text.ends_with('\n');
        let syntax = self.slots[index]
            .syntax
            .get_or_insert_with(|| {
                Rc::new(Syntax {
                    spans: inline_spans(block.kind, block_text),
                    markers: line_markers(block.kind, block_text),
                })
            })
            .clone();
        let caret = (self.reveal_offset(window))
            .filter(|at| self.blocks.index_at(*at) == index)
            .map(|at| at - block.range.start);
        let hidden = hidden_ranges(block.kind, block_text, &syntax, caret, trailing_line);
        let marked = self.marked.as_ref().and_then(|marked| {
            let start = marked.start.max(block.range.start);
            let end = marked.end.min(block.range.end);
            (start < end).then(|| start - block.range.start..end - block.range.start)
        });
        let separator =
            block.kind == BlockKind::Blank && motion::is_separator(text, block.range.start);
        let gap = if separator {
            self.style
                .gap_before(list.get(index + 1).map(|lower| lower.kind))
        } else {
            px(0.)
        };
        let note = self.error.as_ref().and_then(|(line, message)| {
            let at = self.buffer.offset_of_line(*line);
            (self.blocks.index_at(at) == index).then(|| {
                let line_in_block = text[block.range.start..at].matches('\n').count();
                (line_in_block, message.clone())
            })
        });
        let key = ShapeKey {
            hidden,
            marked,
            width: self.text_width,
            gap,
            note,
        };
        if let Some(layout) = &self.slots[index].layout
            && layout.key == key
        {
            return layout.clone();
        }
        let lines = display_lines(
            block.kind,
            block_text,
            &syntax,
            &key.hidden,
            key.marked.clone(),
            trailing_line,
        );
        let layout = Rc::new(shape_block(block.kind, lines, key, &self.style, window));
        self.slots[index].layout = Some(layout.clone());
        layout
    }

    /// Scrolls so the caret is on screen, if it is not.
    fn scroll_to_caret(&mut self, window: &Window) {
        let here = self.locate(self.selection.head, self.upstream, window);
        let layout = self.layout_block(here.block, window);
        let line = &layout.lines[here.line];
        let top = self.tops[here.block] + line.y + line.line_height * here.row as f32;
        let height = self.bounds.size.height;
        let on_screen = top + self.style.padding - self.scroll;
        if on_screen < CARET_MARGIN {
            self.scroll = top + self.style.padding - CARET_MARGIN;
        } else if on_screen + line.line_height > height - CARET_MARGIN {
            self.scroll = top + self.style.padding + line.line_height + CARET_MARGIN - height;
        }
        self.scroll = self.scroll.clamp(px(0.), self.max_scroll());
    }

    /// Scrolls so the line holding `offset` stands at the top.
    fn scroll_line_to_top(&mut self, offset: usize, window: &Window) {
        let here = self.locate(offset, false, window);
        let layout = self.layout_block(here.block, window);
        self.scroll =
            (self.tops[here.block] + layout.lines[here.line].y).clamp(px(0.), self.max_scroll());
    }

    fn locate(&mut self, offset: usize, upstream: bool, window: &Window) -> Located {
        let block = self.blocks.index_at(offset);
        let layout = self.layout_block(block, window);
        let relative = offset - self.blocks.as_slice()[block].range.start;
        let line = (layout.lines.iter())
            .rposition(|line| line.display.source.start <= relative)
            .unwrap_or(0);
        let shaped = &layout.lines[line];
        let display = shaped.display.to_display(relative);
        Located {
            block,
            line,
            row: shaped.row_of(display, upstream),
            display,
        }
    }

    /// The source offset nearest a window position, and whether it shows at the end of an upper
    /// row.
    fn offset_at(&mut self, position: Point<Pixels>, window: &Window) -> (usize, bool) {
        let y = position.y - self.text_top();
        let index = self.tops.partition_point(|top| *top <= y).saturating_sub(1);
        let layout = self.layout_block(index, window);
        let range = self.blocks.as_slice()[index].range.clone();
        let relative_y = (y - self.tops[index]).max(px(0.));
        let line_index = (layout.lines.iter())
            .rposition(|line| line.y <= relative_y && line.height > px(0.))
            .unwrap_or(0);
        let line = &layout.lines[line_index];
        if line.skipped && line.height > px(0.) {
            return if relative_y < line.y + line.height * 0.5 {
                (
                    motion::prev_grapheme(self.buffer.text(), range.start),
                    false,
                )
            } else {
                (range.end, false)
            };
        }
        let row = (((relative_y - line.y) / line.line_height) as usize).min(line.rows.len() - 1);
        let x = position.x - self.text_left - line.x;
        let display = line.display_at_x(row, x.max(px(0.)));
        let upstream = display == line.rows[row].end && !line.is_last_row(row);
        (range.start + line.display.to_source(display), upstream)
    }

    fn caret_bounds(&mut self, window: &Window) -> Bounds<Pixels> {
        self.offset_bounds(self.selection.head, self.upstream, window)
    }

    /// The window rectangle of a caret at `offset`.
    fn offset_bounds(&mut self, offset: usize, upstream: bool, window: &Window) -> Bounds<Pixels> {
        let here = self.locate(offset, upstream, window);
        let layout = self.layout_block(here.block, window);
        let line = &layout.lines[here.line];
        let origin = point(
            self.text_left + line.x + line.x_in_row(here.display, here.row),
            self.block_top(here.block) + line.y + line.line_height * here.row as f32,
        );
        Bounds::new(origin, size(px(2.), line.line_height))
    }

    /// The scrollbar for this frame, at the element's right edge.
    fn update_scrollbar(&mut self) {
        let height = f32::from(self.bounds.size.height);
        let content = f32::from(self.content_height + self.style.padding * 2.);
        let track = Bounds::new(
            point(self.bounds.right() - SCROLLBAR_WIDTH, self.bounds.top()),
            size(SCROLLBAR_WIDTH, self.bounds.size.height),
        );
        self.scrollbar = scrollbar::thumb(
            f32::from(self.scroll),
            content,
            height,
            height,
            f32::from(THUMB_MIN),
        )
        .map(|(top, thumb_height)| Scrollbar {
            track,
            thumb: Bounds::new(
                point(track.left() + px(2.), track.top() + px(top)),
                size(SCROLLBAR_WIDTH - px(4.), px(thumb_height)),
            ),
        });
    }

    fn scrollbar_mouse_down(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(bar) = self.scrollbar.filter(|bar| bar.track.contains(&position)) else {
            return false;
        };
        let grab = if bar.thumb.contains(&position) {
            position.y - bar.thumb.top()
        } else {
            // A click on the track brings the thumb's middle there.
            bar.thumb.size.height * 0.5
        };
        self.thumb_grab = Some(grab);
        self.scrollbar_mouse_moved(position, cx);
        true
    }

    fn scrollbar_mouse_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(grab), Some(bar)) = (self.thumb_grab, self.scrollbar) else {
            return;
        };
        let height = f32::from(bar.track.size.height);
        let content = f32::from(self.content_height + self.style.padding * 2.);
        let top = f32::from(position.y - bar.track.top() - grab);
        let scrolled = scrollbar::scrolled_at(
            top,
            f32::from(bar.thumb.size.height),
            content,
            height,
            height,
        );
        self.scroll = px(scrolled);
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event
            .delta
            .pixel_delta(self.style.size * self.style.leading);
        self.scroll_by(px(0.) - delta.y);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    /// Shows the caret and blinks it from now on. Only a focused editor in the active window
    /// blinks.
    fn restart_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_on = true;
        if !self.focus.is_focused(window) || !window.is_window_active() {
            self.blink = Task::ready(());
            return;
        }
        self.blink = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let blinked = this.update_in(cx, |this, window, cx| {
                    let showing = this.focus.is_focused(window) && window.is_window_active();
                    if showing && this.selection.is_empty() {
                        this.caret_on = !this.caret_on;
                        cx.notify();
                    }
                    showing
                });
                if !matches!(blinked, Ok(true)) {
                    break;
                }
            }
        });
    }

    /// A right click: on a misspelled word it selects the word for the menu, anywhere else the menu
    /// stays shut.
    fn right_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let (offset, _) = self.offset_at(event.position, window);
        self.menu_word =
            (self.misspelled().into_iter()).find(|word| word.start <= offset && offset <= word.end);
        if let Some(word) = self.menu_word.clone() {
            self.set_selection(
                Selection {
                    anchor: word.start,
                    head: word.end,
                },
                false,
                window,
                cx,
            );
        }
    }

    /// Writes a suggestion over the word, as one undo step.
    fn apply_suggestion(
        &mut self,
        word: Range<usize>,
        suggestion: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let after = Selection::caret(word.start + suggestion.len());
        self.edit(
            EditKind::Discrete,
            vec![Edit::new(word, suggestion)],
            after,
            None,
            window,
            cx,
        );
    }

    /// Adds the word to the dictionary and takes its underlines away everywhere.
    fn learn(&mut self, word: Range<usize>, cx: &mut Context<Self>) {
        self.spelling.checker.learn(&self.buffer.text()[word]);
        self.recheck();
        cx.notify();
    }
}

/// The spelling menu for the word a right click selected, empty elsewhere, which keeps it shut.
fn spelling_menu(
    menu: PopupMenu,
    view: &WeakEntity<EditorView>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let editor = entity.read(cx);
    let Some(word) = editor.menu_word.clone() else {
        return menu;
    };
    let mut suggestions = editor
        .spelling
        .checker
        .suggestions(&editor.buffer.text()[word.clone()]);
    suggestions.truncate(spelling::SUGGESTIONS);
    let mut menu = if suggestions.is_empty() {
        menu.label("No suggestions")
    } else {
        menu
    };
    for suggestion in suggestions {
        let (view, word) = (view.clone(), word.clone());
        menu = menu.item(
            PopupMenuItem::new(suggestion.clone()).on_click(move |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.apply_suggestion(word.clone(), &suggestion, window, cx)
                })
                .ok();
            }),
        );
    }
    let view = view.clone();
    menu.separator().item(
        PopupMenuItem::new("Learn spelling").on_click(move |_, _, cx| {
            view.update(cx, |view, cx| view.learn(word.clone(), cx))
                .ok();
        }),
    )
}

fn now() -> std::time::Instant {
    std::time::Instant::now()
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut at = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut len = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    while !a.is_char_boundary(a.len() - len) || !b.is_char_boundary(b.len() - len) {
        len -= 1;
    }
    len
}

/// One listener per caret motion, as a move and as a selection.
macro_rules! motions {
    ($element:expr, $cx:expr, $($action:ident => $motion:ident, $select:ident),* $(,)?) => {
        $element
            $(
                .on_action($cx.listener(|this, _: &$action, window, cx| {
                    this.move_caret(selection::Motion::$motion, false, window, cx)
                }))
                .on_action($cx.listener(|this, _: &$select, window, cx| {
                    this.move_caret(selection::Motion::$motion, true, window, cx)
                }))
            )*
    };
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let pane =
            div()
                .id("script-editor")
                .key_context(CONTEXT)
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .on_action(cx.listener(Self::backspace))
                .on_action(cx.listener(Self::delete))
                .on_action(cx.listener(Self::delete_word_left))
                .on_action(cx.listener(Self::delete_word_right))
                .on_action(cx.listener(Self::delete_to_line_start))
                .on_action(cx.listener(Self::new_block))
                .on_action(cx.listener(Self::new_line))
                .on_action(cx.listener(Self::select_all))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::undo))
                .on_action(cx.listener(Self::redo))
                .on_action(cx.listener(Self::insert_link))
                .on_action(cx.listener(|_, _: &ShowCharacterPalette, window, _| {
                    window.show_character_palette()
                }))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                .on_mouse_down(MouseButton::Right, cx.listener(Self::right_mouse_down))
                .on_scroll(cx.listener(Self::scroll));
        motions!(
            pane,
            cx,
            Left => Left, SelectLeft,
            Right => Right, SelectRight,
            Up => Up, SelectUp,
            Down => Down, SelectDown,
            WordLeft => WordLeft, SelectWordLeft,
            WordRight => WordRight, SelectWordRight,
            LineStart => LineStart, SelectLineStart,
            LineEnd => LineEnd, SelectLineEnd,
            DocStart => DocStart, SelectDocStart,
            DocEnd => DocEnd, SelectDocEnd,
            PageUp => PageUp, SelectPageUp,
            PageDown => PageDown, SelectPageDown,
        )
        .child(element::TextElement::new(cx.entity()))
        .context_menu(move |menu, _, cx| spelling_menu(menu, &view, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_changed_part_is_found_on_character_boundaries() {
        let (old, new) = ("Größe und mehr", "Grüße und mehr");
        let prefix = common_prefix(old, new);
        assert_eq!(prefix, "Gr".len());
        assert_eq!(
            common_suffix(&old[prefix..], &new[prefix..]),
            "ße und mehr".len()
        );
        assert_eq!(common_suffix("aé", "bé"), "é".len());
    }
}
