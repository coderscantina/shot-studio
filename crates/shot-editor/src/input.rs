//! What typing, deleting, the clipboard, and undo do to the text, and the system's input methods.
//! Each change is one edit to the buffer, after which the blocks around it are read again.

use std::ops::Range;

use gpui::{
    Bounds, ClipboardItem, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, px,
    size,
};

use crate::keys::*;
use crate::source::buffer::{Restored, Splice};
use crate::source::{Edit, EditKind, Selection, motion, typing};
use crate::{Changed, EditorView, Slot, now};

impl EditorView {
    pub(crate) fn edit(
        &mut self,
        kind: EditKind,
        edits: Vec<Edit>,
        after: Selection,
        marked: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(splice) = self
            .buffer
            .transact(kind, edits, self.selection, after, now())
        else {
            return;
        };
        self.resegment(splice);
        self.marked = marked;
        self.set_selection(after, false, window, cx);
        window.notify_text_input_changed(&self.focus);
        self.restart_spelling(cx);
        cx.emit(Changed);
    }

    pub(crate) fn resegment(&mut self, splice: Splice) {
        let change = self.blocks.apply(self.buffer.text(), splice);
        let fresh = (0..change.inserted).map(|_| Slot::default());
        self.slots
            .splice(change.start..change.start + change.removed, fresh);
    }

    fn replace_selection(
        &mut self,
        text: &str,
        kind: EditKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.selection.range();
        let after = Selection::caret(range.start + text.len());
        self.edit(kind, vec![Edit::new(range, text)], after, None, window, cx);
    }

    fn delete_towards(
        &mut self,
        target: impl Fn(&str, usize) -> usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selection.is_empty() {
            let head = self.selection.head;
            let other = target(self.buffer.text(), head);
            head.min(other)..head.max(other)
        } else {
            self.selection.range()
        };
        if !range.is_empty() {
            let after = Selection::caret(range.start);
            self.edit(
                EditKind::Deleting,
                vec![Edit::new(range, "")],
                after,
                None,
                window,
                cx,
            );
        }
    }

    pub(crate) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_towards(motion::left, window, cx);
    }

    pub(crate) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_towards(motion::right, window, cx);
    }

    pub(crate) fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_towards(motion::prev_word, window, cx);
    }

    pub(crate) fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_towards(motion::next_word, window, cx);
    }

    pub(crate) fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.row_edge(false, window).0;
        self.delete_towards(move |_, _| target, window, cx);
    }

    /// Enter: a new paragraph in prose, the next beat after a bullet, and the end of the list on an
    /// empty one.
    pub(crate) fn new_block(&mut self, _: &NewBlock, window: &mut Window, cx: &mut Context<Self>) {
        let edit = if self.selection.is_empty() {
            typing::line_break(self.buffer.text(), &self.blocks, self.selection.head)
        } else {
            Edit::new(self.selection.range(), "\n\n")
        };
        let after = Selection::caret(edit.range.start + edit.text.len());
        self.edit(EditKind::Discrete, vec![edit], after, None, window, cx);
    }

    pub(crate) fn new_line(&mut self, _: &NewLine, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_selection("\n", EditKind::Discrete, window, cx);
    }

    /// Wraps the selection in `[selection]()` with the caret where the target goes. With nothing
    /// selected the caret goes between the empty brackets, for the label.
    pub(crate) fn insert_link(
        &mut self,
        _: &InsertLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.selection.range();
        let label = &self.buffer.text()[range.clone()];
        let text = format!("[{label}]()");
        let caret = if label.is_empty() {
            range.start + 1
        } else {
            range.start + text.len() - 1
        };
        self.edit(
            EditKind::Discrete,
            vec![Edit::new(range, text)],
            Selection::caret(caret),
            None,
            window,
            cx,
        );
    }

    pub(crate) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_none() {
            let restored = self.buffer.undo();
            self.restore(restored, window, cx);
        }
    }

    pub(crate) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_none() {
            let restored = self.buffer.redo();
            self.restore(restored, window, cx);
        }
    }

    fn restore(&mut self, restored: Option<Restored>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(restored) = restored else {
            return;
        };
        self.resegment(restored.splice);
        self.set_selection(restored.selection, false, window, cx);
        window.notify_text_input_changed(&self.focus);
        self.restart_spelling(cx);
        cx.emit(Changed);
    }

    pub(crate) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let text = self.buffer.text()[self.selection.range()].to_owned();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            let text = self.buffer.text()[self.selection.range()].to_owned();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.replace_selection("", EditKind::Discrete, window, cx);
        }
    }

    pub(crate) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text.replace("\r\n", "\n"), EditKind::Discrete, window, cx);
        }
    }

    /// The range an input method means: the one it names, else the marked text, else the selection.
    fn input_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| self.buffer.range_from_utf16(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.range())
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        Some(self.buffer.text()[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.selection.range()),
            reversed: self.selection.reversed(),
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.buffer.range_from_utf16(&range_utf16);
        let selection = Selection {
            anchor: range.start,
            head: range.end,
        };
        self.set_selection(selection, false, window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.buffer.utf16_len())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| self.buffer.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = if self.marked.is_some() {
            EditKind::Composing
        } else {
            EditKind::Typing
        };
        let range = self.input_range(range_utf16);
        let after = Selection::caret(range.start + new_text.len());
        self.edit(
            kind,
            vec![Edit::new(range, new_text)],
            after,
            None,
            window,
            cx,
        );
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.input_range(range_utf16);
        let new_len_utf16 = new_text.encode_utf16().count();
        let marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        // The new selection is in UTF-16 units within the marked text, which is only known once it
        // is written.
        let after = match new_selected_range_utf16 {
            Some(selected) => Selection {
                anchor: range.start + byte_offset(new_text, selected.start.min(new_len_utf16)),
                head: range.start + byte_offset(new_text, selected.end.min(new_len_utf16)),
            },
            None => Selection::caret(range.start + new_text.len()),
        };
        self.edit(
            EditKind::Composing,
            vec![Edit::new(range, new_text)],
            after,
            marked,
            window,
            cx,
        );
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        let start = self.offset_bounds(range.start, false, window);
        let end = self.offset_bounds(range.end, false, window);
        let width = if end.origin.y == start.origin.y {
            (end.origin.x - start.origin.x).max(px(1.))
        } else {
            px(1.)
        };
        Some(Bounds::new(start.origin, size(width, start.size.height)))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let (offset, _) = self.offset_at(point, window);
        Some(self.buffer.utf16_of(offset))
    }
}

/// The byte offset of a UTF-16 offset in a short string.
fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (at, character) in text.char_indices() {
        if units >= utf16 {
            return at;
        }
        units += character.len_utf16();
    }
    text.len()
}
