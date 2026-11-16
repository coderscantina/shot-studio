//! Where the caret and the selection go: by keys, by row and page, and by the pointer.

use gpui::{Context, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Window, point, px};

use crate::keys::SelectAll;
use crate::source::{BlockKind, Blocks, Selection, motion};
use crate::{Drag, DragUnit, EditorView};

#[derive(Clone, Copy)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

/// How far a drag outside the editor scrolls per pointer move, per pixel it is outside.
const DRAG_SCROLL_SPEED: f32 = 0.3;

impl EditorView {
    pub(crate) fn set_selection(
        &mut self,
        selection: Selection,
        upstream: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = selection;
        self.upstream = upstream;
        self.goal_x = None;
        if self.drag.is_none() {
            self.reveal_at = selection.head;
        }
        self.autoscroll = true;
        self.restart_blink(window, cx);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    pub(crate) fn move_caret(
        &mut self,
        motion_kind: Motion,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.buffer.text();
        let head = self.selection.head;
        let range = self.selection.range();
        let collapsing = !extend && !self.selection.is_empty();
        let goal_x = self.goal_x;
        let mut new_goal = None;
        let (target, upstream) = match motion_kind {
            Motion::Left if collapsing => (range.start, false),
            Motion::Right if collapsing => (range.end, false),
            Motion::Left => (motion::left(text, head), false),
            Motion::Right => (motion::right(text, head), false),
            Motion::WordLeft => (motion::prev_word(text, head), false),
            Motion::WordRight => (motion::next_word(text, head), false),
            Motion::DocStart if extend => (0, false),
            Motion::DocStart => (past_line_marker(text, &self.blocks, 0), false),
            Motion::DocEnd => (text.len(), false),
            Motion::LineStart => self.row_edge(false, window),
            Motion::LineEnd => self.row_edge(true, window),
            Motion::Up | Motion::Down => {
                let (target, upstream, goal) =
                    self.vertical_target(matches!(motion_kind, Motion::Down), goal_x, window);
                new_goal = Some(goal);
                (target, upstream)
            }
            Motion::PageUp | Motion::PageDown => {
                let (target, upstream, goal) =
                    self.page_target(matches!(motion_kind, Motion::PageDown), goal_x, window);
                new_goal = Some(goal);
                (target, upstream)
            }
        };
        let selection = Selection {
            anchor: if extend {
                self.selection.anchor
            } else {
                target
            },
            head: target,
        };
        self.set_selection(selection, upstream, window, cx);
        self.goal_x = new_goal;
    }

    /// The source offset of the start or end of the caret's row.
    pub(crate) fn row_edge(&mut self, end: bool, window: &Window) -> (usize, bool) {
        let here = self.locate(self.selection.head, self.upstream, window);
        let layout = self.layout_block(here.block, window);
        let line = &layout.lines[here.line];
        let row = &line.rows[here.row];
        let display = if end { row.end } else { row.start };
        let upstream = end && !line.is_last_row(here.row);
        let start = self.blocks.as_slice()[here.block].range.start;
        (start + line.display.to_source(display), upstream)
    }

    /// The target of up or down, and the column it aims for.
    fn vertical_target(
        &mut self,
        down: bool,
        goal: Option<Pixels>,
        window: &Window,
    ) -> (usize, bool, Pixels) {
        let here = self.locate(self.selection.head, self.upstream, window);
        let layout = self.layout_block(here.block, window);
        let line = &layout.lines[here.line];
        let goal = goal.unwrap_or(line.x + line.x_in_row(here.display, here.row));
        let last_block = self.blocks.as_slice().len() - 1;
        let (mut block, mut line_index, mut row) = (here.block, here.line, here.row);
        let mut current = layout;
        loop {
            if down {
                if row + 1 < current.lines[line_index].rows.len() {
                    row += 1;
                } else if line_index + 1 < current.lines.len() {
                    line_index += 1;
                    row = 0;
                } else if block < last_block {
                    block += 1;
                    current = self.layout_block(block, window);
                    line_index = 0;
                    row = 0;
                } else {
                    return (self.buffer.text().len(), false, goal);
                }
            } else if row > 0 {
                row -= 1;
            } else if line_index > 0 {
                line_index -= 1;
                row = current.lines[line_index].rows.len() - 1;
            } else if block > 0 {
                block -= 1;
                current = self.layout_block(block, window);
                line_index = current.lines.len() - 1;
                row = current.lines[line_index].rows.len() - 1;
            } else {
                return (0, false, goal);
            }
            if !current.lines[line_index].skipped {
                break;
            }
        }
        let line = &current.lines[line_index];
        let display = line.display_at_x(row, goal - line.x);
        let upstream = display == line.rows[row].end && !line.is_last_row(row);
        let start = self.blocks.as_slice()[block].range.start;
        (start + line.display.to_source(display), upstream, goal)
    }

    /// Scrolls a page and returns the place at the caret's old height on screen.
    fn page_target(
        &mut self,
        down: bool,
        goal: Option<Pixels>,
        window: &Window,
    ) -> (usize, bool, Pixels) {
        let line = self.style.size * self.style.leading;
        let distance = (self.bounds.size.height - line * 2.).max(line);
        let caret = self.caret_bounds(window).origin;
        let goal = goal.unwrap_or(caret.x - self.text_left);
        self.scroll_by(if down { distance } else { px(0.) - distance });
        let target = point(
            self.text_left + goal,
            caret.y.clamp(self.bounds.top(), self.bounds.bottom()),
        );
        let (offset, upstream) = self.offset_at(target, window);
        (offset, upstream, goal)
    }

    pub(crate) fn select_all(
        &mut self,
        _: &SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = Selection {
            anchor: 0,
            head: self.buffer.text().len(),
        };
        self.set_selection(selection, false, window, cx);
        self.autoscroll = false;
    }

    pub(crate) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.pointer = event.position;
        if self.scrollbar_mouse_down(event.position, cx) {
            return;
        }
        let (offset, upstream) = self.offset_at(event.position, window);
        let text = self.buffer.text();
        let (origin, unit) = match event.click_count {
            1 => (offset..offset, DragUnit::Character),
            2 => (motion::word_range(text, offset), DragUnit::Word),
            _ => (motion::line_range(text, offset), DragUnit::Line),
        };
        let selection = if event.modifiers.shift {
            Selection {
                anchor: self.selection.anchor,
                head: offset,
            }
        } else {
            Selection {
                anchor: origin.start,
                head: origin.end,
            }
        };
        self.drag = Some(Drag { origin, unit });
        self.reveal_at = selection.head;
        self.set_selection(selection, upstream, window, cx);
    }

    pub(crate) fn mouse_moved(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer = event.position;
        let over_thumb = self
            .scrollbar
            .is_some_and(|bar| bar.track.contains(&event.position));
        if over_thumb != self.thumb_hovered {
            self.thumb_hovered = over_thumb;
            cx.notify();
        }
        if event.pressed_button != Some(MouseButton::Left) {
            if self.drag.is_some() || self.thumb_grab.is_some() {
                self.end_mouse(cx);
            }
            return;
        }
        if self.thumb_grab.is_some() {
            self.scrollbar_mouse_moved(event.position, cx);
        } else if self.drag.is_some() {
            // A pointer above or below the editor scrolls the text on with each move.
            let above = self.bounds.top() - event.position.y;
            let below = event.position.y - self.bounds.bottom();
            if above > px(0.) {
                self.scroll_by(px(0.) - above * DRAG_SCROLL_SPEED);
            } else if below > px(0.) {
                self.scroll_by(below * DRAG_SCROLL_SPEED);
            }
            self.extend_drag(window, cx);
        }
    }

    /// Extends the selection in progress to the pointer.
    fn extend_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        let (offset, upstream) = self.offset_at(self.pointer, window);
        let text = self.buffer.text();
        let unit = match drag.unit {
            DragUnit::Character => offset..offset,
            DragUnit::Word => motion::word_range(text, offset),
            DragUnit::Line => motion::line_range(text, offset),
        };
        let selection =
            if self.selection.anchor != drag.origin.start && drag.unit == DragUnit::Character {
                // A shift-click drag keeps the anchor it started from.
                Selection {
                    anchor: self.selection.anchor,
                    head: offset,
                }
            } else if unit.start < drag.origin.start {
                Selection {
                    anchor: drag.origin.end,
                    head: unit.start,
                }
            } else {
                Selection {
                    anchor: drag.origin.start,
                    head: unit.end.max(drag.origin.end),
                }
            };
        if selection != self.selection {
            self.set_selection(selection, upstream, window, cx);
            self.autoscroll = false;
        }
    }

    /// The button came up: a selection settles and the thumb lets go.
    pub(crate) fn end_mouse(&mut self, cx: &mut Context<Self>) {
        self.thumb_grab = None;
        if self.drag.take().is_some() {
            self.reveal_at = self.selection.head;
            cx.notify();
        }
    }
}

/// Where a heading's or bullet's text starts when `offset` is the start of its line: past the
/// marker and the space after it, which stand in the margin, so a typed letter goes into the text.
pub(crate) fn past_line_marker(text: &str, blocks: &Blocks, offset: usize) -> usize {
    let block = &blocks.as_slice()[blocks.index_at(offset)];
    if !matches!(block.kind, BlockKind::Heading { .. } | BlockKind::List) {
        return offset;
    }
    let line = &text[offset..motion::line_range(text, offset).end];
    let marker = line
        .bytes()
        .take_while(|byte| matches!(byte, b'#' | b'-' | b'*' | b'+'))
        .count();
    if marker == 0 {
        return offset;
    }
    offset
        + marker
        + line[marker..]
            .bytes()
            .take_while(|byte| *byte == b' ')
            .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_start_skips_its_heading_or_bullet_marker() {
        let text = "# Title\n\n## Hook\n\n- A beat\n  its script\n";
        let blocks = Blocks::parse(text);
        assert_eq!(past_line_marker(text, &blocks, 0), 2);
        assert_eq!(past_line_marker(text, &blocks, 9), 12);
        assert_eq!(past_line_marker(text, &blocks, 18), 20);
        assert_eq!(past_line_marker(text, &blocks, 27), 27);
    }
}
