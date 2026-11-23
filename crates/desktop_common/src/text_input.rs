//! A text input. GPUI has no ready-made input widget; this one starts from the
//! `gpui/examples/input.rs` example in the Zed repository and adds soft wrapping and a
//! multi-line mode.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, SharedString, Subscription, UTF16Selection, Window, actions, div, point, prelude::*,
    px,
};
#[cfg(test)]
use gpui::size;
use unicode_segmentation::*;

use crate::touch_keyboard;

mod element;
use element::{TextElement, TextLayout};
#[cfg(test)]
use element::{CARET_WIDTH, masked_text};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordBackward,
        DeleteWordForward,
        Left,
        Right,
        WordLeft,
        WordRight,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocumentHome,
        DocumentEnd,
        SelectDocumentHome,
        SelectDocumentEnd,
        Indent,
        Outdent,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Enter,
        Cancel,
    ]
);


/// Enter was pressed in an input that does not take newlines; carries the current text.
pub struct Submitted(pub String);

/// The user changed the text (not via [`TextInput::set_text`]).
pub struct Changed;

impl EventEmitter<Submitted> for TextInput {}
/// Escape was pressed.
pub struct Cancelled;

/// Multi-line text (2+ non-empty lines) was pasted into a single-line input that opted in.
pub struct MultiLinePasted(pub Vec<String>);

impl EventEmitter<Changed> for TextInput {}
impl EventEmitter<Cancelled> for TextInput {}
impl EventEmitter<MultiLinePasted> for TextInput {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One line without wrapping; Enter emits [`Submitted`].
    SingleLine,
    /// Long text wraps, but Enter emits [`Submitted`] instead of inserting a newline.
    Wrapped,
    /// Multi-line text: Enter inserts a newline.
    MultiLine,
}

impl Mode {
    fn wraps(self) -> bool {
        self != Mode::SingleLine
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    mode: Mode,
    content: SharedString,
    placeholder: SharedString,
    text_color: Option<gpui::Hsla>,
    placeholder_color: Option<gpui::Hsla>,
    caret_color: Option<gpui::Hsla>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<TextLayout>,
    pending_click: Option<Point<Pixels>>,
    is_selecting: bool,
    cancellable: bool,
    /// Byte ranges shown in the accent color on a tinted background, such as the date and tags
    /// quick add recognized.
    highlights: Vec<Range<usize>>,
    /// A password field: the text shows as "*", one per Unicode scalar value, and it cannot be
    /// copied.
    masked: bool,
    /// When true, the user can select and copy text, but cannot modify the content.
    read_only: bool,
    /// When true on a single-line input, pasting 2+ non-empty lines emits [`MultiLinePasted`]
    /// instead of flattening newlines into spaces (used for Batch Add Tasks).
    intercept_multiline_paste: bool,
    /// Hides the touch keyboard this input opened once it loses focus.
    keyboard_blur: Option<Subscription>,
    undo_stack: Vec<(SharedString, Range<usize>)>,
    redo_stack: Vec<(SharedString, Range<usize>)>,
}

impl TextInput {
    pub fn new(mode: Mode, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            mode,
            content: "".into(),
            placeholder: placeholder.into(),
            text_color: None,
            placeholder_color: None,
            caret_color: None,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            pending_click: None,
            is_selecting: false,
            cancellable: true,
            highlights: Vec::new(),
            masked: false,
            read_only: false,
            intercept_multiline_paste: false,
            keyboard_blur: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Lets the containing view handle Escape when this field has no edit to cancel.
    pub fn without_cancel(mut self) -> Self {
        self.cancellable = false;
        self
    }

    /// Emits [`MultiLinePasted`] instead of flattening newlines when 2+ lines are pasted.
    pub fn intercept_multiline_paste(mut self) -> Self {
        self.intercept_multiline_paste = true;
        self
    }

    /// Makes this a password field.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// Toggles read-only mode: text can still be selected and copied, but edits are ignored.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            if read_only {
                self.marked_range = None;
                touch_keyboard::release(&self.focus_handle);
            }
            cx.notify();
        }
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_placeholder_color(&mut self, color: gpui::Hsla, cx: &mut Context<Self>) {
        self.placeholder_color = Some(color);
        cx.notify();
    }

    /// Overrides typed text, placeholder, and caret colors for surfaces whose polarity
    /// can differ from the active app theme (such as sticky notes).
    pub fn set_ink_override(
        &mut self,
        text: Option<gpui::Hsla>,
        placeholder: Option<gpui::Hsla>,
        caret: Option<gpui::Hsla>,
        cx: &mut Context<Self>,
    ) {
        if self.text_color != text
            || self.placeholder_color != placeholder
            || self.caret_color != caret
        {
            self.text_color = text;
            self.placeholder_color = placeholder;
            self.caret_color = caret;
            cx.notify();
        }
    }

    /// Replaces the text without emitting [`Changed`], for loading data from outside.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.pending_click = None;
        self.highlights.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        cx.notify();
    }

    /// Places the caret at the character nearest `position` (in window coordinates).
    /// When called before the first layout, defers resolution until the initial prepaint.
    pub fn place_cursor_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.marked_range = None;
        if self.last_layout.is_some() {
            let offset = self.index_for_mouse_position(position);
            self.selected_range = offset..offset;
            self.selection_reversed = false;
            self.pending_click = None;
        } else {
            self.pending_click = Some(position);
        }
        cx.notify();
    }

    /// The selected byte range (the cursor when empty).
    pub fn selection(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    /// Replaces the text as an edit — it emits [`Changed`] — and selects `selection`.
    pub fn edit(&mut self, text: String, selection: Range<usize>, window: &Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.push_undo_snapshot();
        let start = snap_char_boundary(&text, selection.start);
        let end = snap_char_boundary(&text, selection.end).max(start);
        self.content = text.into();
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.pending_click = None;
        self.highlights.clear();
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    pub fn clear(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.set_text("", cx);
        window.notify_text_input_changed(&self.focus_handle);
    }

    /// Marks ranges of the text (byte offsets) as recognized; ranges are snapped to UTF-8 char
    /// boundaries, clamped to the current text, sorted, and merged so overlapping/out-of-range
    /// ranges can never panic during display shaping.
    pub fn set_highlights(&mut self, ranges: Vec<Range<usize>>, cx: &mut Context<Self>) {
        let mut normalized: Vec<Range<usize>> = ranges
            .into_iter()
            .filter_map(|r| {
                if r.start >= r.end {
                    return None;
                }
                let start = snap_char_boundary(&self.content, r.start);
                let end = snap_char_boundary(&self.content, r.end);
                (start < end).then_some(start..end)
            })
            .collect();
        normalized.sort_by_key(|r| (r.start, r.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(normalized.len());
        for range in normalized {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        if merged != self.highlights {
            self.highlights = merged;
            cx.notify();
        }
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(Cancelled);
    }

    fn push_undo_snapshot(&mut self) {
        if self.masked {
            return;
        }
        if self.undo_stack.last().is_some_and(|(text, _)| *text == self.content) {
            return;
        }
        const MAX_UNDO: usize = 64;
        if self.undo_stack.len() >= MAX_UNDO {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push((self.content.clone(), self.selected_range.clone()));
        self.redo_stack.clear();
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some((prev_text, prev_sel)) = self.undo_stack.pop() else {
            cx.propagate();
            return;
        };
        self.redo_stack.push((self.content.clone(), self.selected_range.clone()));
        self.content = prev_text;
        self.selected_range = prev_sel;
        self.selection_reversed = false;
        self.marked_range = None;
        self.highlights.clear();
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some((next_text, next_sel)) = self.redo_stack.pop() else {
            cx.propagate();
            return;
        };
        self.undo_stack.push((self.content.clone(), self.selected_range.clone()));
        self.content = next_text;
        self.selected_range = next_sel;
        self.selection_reversed = false;
        self.marked_range = None;
        self.highlights.clear();
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.mode == Mode::MultiLine {
            if self.selected_range.is_empty() && self.marked_range.is_none() {
                let cursor = self.selected_range.end;
                let line_start = self.content[..cursor].rfind('\n').map_or(0, |i| i + 1);
                let line_end = self.content[cursor..].find('\n').map_or(self.content.len(), |i| cursor + i);
                let before_cursor = &self.content[line_start..cursor];
                let after_cursor = &self.content[cursor..line_end];
                match markdown_list_continuation(before_cursor) {
                    Some(ListEnter::Continue(prefix)) => {
                        self.replace_text_in_range(None, &format!("\n{prefix}"), window, cx);
                        return;
                    }
                    Some(ListEnter::ClearLine) if after_cursor.trim().is_empty() => {
                        self.selected_range = line_start..line_end;
                        self.replace_text_in_range(None, "", window, cx);
                        return;
                    }
                    Some(ListEnter::ClearLine) | None => {}
                }
            }
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(Submitted(self.content.to_string()));
        }
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.previous_word_boundary(self.cursor_offset());
        self.move_to(offset, window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.next_word_boundary(self.cursor_offset());
        self.move_to(offset, window, cx);
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(-1.);
        self.move_to(offset, window, cx);
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(1.);
        self.move_to(offset, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.previous_word_boundary(self.cursor_offset());
        self.select_to(offset, window, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.next_word_boundary(self.cursor_offset());
        self.select_to(offset, window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(-1.);
        self.select_to(offset, window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(1.);
        self.select_to(offset, window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.visual_line_edge(false);
        self.move_to(offset, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.visual_line_edge(true);
        self.move_to(offset, window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.visual_line_edge(false);
        self.select_to(offset, window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.visual_line_edge(true);
        self.select_to(offset, window, cx);
    }

    fn document_home(&mut self, _: &DocumentHome, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
    }

    fn document_end(&mut self, _: &DocumentEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), window, cx);
    }

    fn select_document_home(
        &mut self,
        _: &SelectDocumentHome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, window, cx);
    }

    fn select_document_end(
        &mut self,
        _: &SelectDocumentEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.content.len(), window, cx);
    }

    fn indent(&mut self, _: &Indent, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.mode != Mode::MultiLine {
            cx.propagate();
            return;
        }
        let (new_text, new_sel) = adjust_lines_indent(&self.content, self.selected_range.clone(), true);
        if new_text != self.content.as_ref() {
            self.edit(new_text, new_sel, window, cx);
        }
    }

    fn outdent(&mut self, _: &Outdent, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.mode != Mode::MultiLine {
            cx.propagate();
            return;
        }
        let (new_text, new_sel) = adjust_lines_indent(&self.content, self.selected_range.clone(), false);
        if new_text != self.content.as_ref() {
            self.edit(new_text, new_sel, window, cx);
        }
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, window, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_backward(&mut self, _: &DeleteWordBackward, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selected_range.is_empty() {
            let prev = self.previous_word_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_forward(&mut self, _: &DeleteWordForward, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.next_word_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    /// Requests the platform/touch virtual keyboard when an input is opened by a tap rather than
    /// directly receiving `on_mouse_down` (for example, inline title editing).
    pub fn open_touch_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        window.request_virtual_keyboard();
        if touch_keyboard::show_for_tap(window, &self.focus_handle) && self.keyboard_blur.is_none()
        {
            self.keyboard_blur = Some(cx.on_blur(&self.focus_handle, window, |this, window, cx| {
                // Later, so that an input tapped next takes the keyboard over without it closing.
                let focus = this.focus_handle.clone();
                window.defer(cx, move |_, _| touch_keyboard::release(&focus));
            }));
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        self.open_touch_keyboard(window, cx);

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        } else if event.click_count == 2 {
            let offset = self.index_for_mouse_position(event.position);
            let range = self.word_range_at(offset);
            self.move_to(range.start, window, cx);
            self.select_to(range.end, window, cx);
            self.is_selecting = false;
        } else if event.click_count >= 3 {
            let offset = self.index_for_mouse_position(event.position);
            let start = self.content[..offset].rfind('\n').map_or(0, |i| i + 1);
            let end = self.content[offset..].find('\n').map_or(self.content.len(), |i| offset + i);
            self.move_to(start, window, cx);
            self.select_to(end, window, cx);
            self.is_selecting = false;
        } else {
            self.move_to(self.index_for_mouse_position(event.position), window, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let normalized = normalize_line_endings(&text);
            if self.mode == Mode::SingleLine && self.intercept_multiline_paste {
                let lines: Vec<String> = normalized
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                if lines.len() >= 2 {
                    cx.emit(MultiLinePasted(lines));
                    return;
                }
            }
            let text = if self.mode == Mode::MultiLine {
                normalized
            } else {
                normalized.replace('\n', " ")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        let had_marked = self.marked_range.take().is_some();
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        if had_marked {
            window.notify_text_input_changed(&self.focus_handle);
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Maps a byte offset in `self.content` to a byte offset in the displayed text
    /// (which differs when `self.masked` replaces each Unicode `char` with a single `'*'`).
    fn display_offset(&self, byte_offset: usize) -> usize {
        let clamped = snap_char_boundary(&self.content, byte_offset);
        if self.masked {
            self.content[..clamped].chars().count()
        } else {
            clamped
        }
    }

    /// The offset one line above (`direction = -1`) or below (`1`) at the same x.
    fn vertical_offset(&self, direction: f32) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.last_layout.as_ref() else {
            return cursor;
        };
        let position = layout.position_for_offset(self.display_offset(cursor));
        let target_y = position.y + layout.line_height * (direction + 0.5);
        if target_y < px(0.) {
            return 0;
        }
        let total_height = layout
            .paragraphs
            .last()
            .map_or(px(0.), |p| p.top + p.line.size(layout.line_height).height);
        if target_y >= total_height {
            return self.content.len();
        }
        self.offset_for_text_position(layout, point(position.x, target_y))
    }

    /// The start or end of the visual line the caret is on.
    fn visual_line_edge(&self, end: bool) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.last_layout.as_ref() else {
            return if end { self.content.len() } else { 0 };
        };
        let y = layout.position_for_offset(self.display_offset(cursor)).y + layout.line_height * 0.5;
        let x = if end { px(f32::MAX / 4.) } else { px(0.) };
        self.offset_for_text_position(layout, point(x, y))
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(layout) = self.last_layout.as_ref() else {
            return 0;
        };
        if position.y < layout.bounds.top() {
            return 0;
        }
        if position.y > layout.bounds.bottom() {
            return self.content.len();
        }
        self.offset_for_text_position(layout, position - layout.origin())
    }

    fn offset_for_text_position(&self, layout: &TextLayout, position: Point<Pixels>) -> usize {
        let offset = layout.offset_for_position(position);
        if self.masked {
            // Each mask glyph represents one Unicode scalar value (`char`), and editing
            // must never split a multi-scalar grapheme cluster (combining marks / ZWJ emoji).
            let byte_offset = self
                .content
                .char_indices()
                .nth(offset)
                .map_or(self.content.len(), |(idx, _)| idx);
            self.content
                .grapheme_indices(true)
                .map(|(index, _)| index)
                .chain(std::iter::once(self.content.len()))
                .min_by_key(|index| index.abs_diff(byte_offset))
                .unwrap_or(0)
        } else {
            offset
        }
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        let had_marked = self.marked_range.take().is_some();
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        if had_marked {
            window.notify_text_input_changed(&self.focus_handle);
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        let (start, end) = if range_utf16.start <= range_utf16.end {
            (range_utf16.start, range_utf16.end)
        } else {
            (range_utf16.end, range_utf16.start)
        };
        self.offset_from_utf16(start)..self.offset_from_utf16(end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        let prefix = &self.content[..offset.min(self.content.len())];
        let trimmed = prefix.trim_end_matches(|c: char| c.is_whitespace() && c != '\n');
        if trimmed.is_empty() {
            return 0;
        }
        if trimmed.ends_with('\n') && trimmed.len() == prefix.len() {
            return trimmed.len() - 1;
        }
        trimmed
            .split_word_bound_indices()
            .rfind(|(_, piece)| !piece.chars().all(char::is_whitespace))
            .map_or(0, |(idx, _)| idx)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.content.len());
        let suffix = &self.content[offset..];
        let leading_ws = suffix
            .char_indices()
            .find(|(_, c)| !c.is_whitespace() || *c == '\n')
            .map_or(suffix.len(), |(idx, _)| idx);
        let rest = &suffix[leading_ws..];
        if rest.is_empty() {
            return self.content.len();
        }
        if rest.starts_with('\n') && leading_ws == 0 {
            return offset + 1;
        }
        let first_len = rest
            .split_word_bounds()
            .next()
            .map_or(rest.len(), str::len);
        offset + leading_ws + first_len
    }

    fn word_range_at(&self, offset: usize) -> Range<usize> {
        let offset = offset.min(self.content.len());
        for (start, piece) in self.content.split_word_bound_indices() {
            let end = start + piece.len();
            if offset >= start && offset < end {
                return start..end;
            }
        }
        if let Some((start, piece)) = self.content.split_word_bound_indices().next_back() {
            return start..start + piece.len();
        }
        offset..offset
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let was_marked = self.marked_range.is_some();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        if !was_marked {
            self.push_undo_snapshot();
        }
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        self.highlights.clear();
        // If an IME composition ended back at the exact pre-composition content, drop the
        // redundant snapshot so Undo reverts the prior edit instead of being a no-op.
        if was_marked && self.undo_stack.last().is_some_and(|(text, _)| *text == self.content) {
            self.undo_stack.pop();
        }
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let was_marked = self.marked_range.is_some();
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        // Snapshot once when IME composition begins; intermediate preedit updates must not
        // flood the 64-entry undo stack.
        if !was_marked && !(range.is_empty() && new_text.is_empty()) {
            self.push_undo_snapshot();
        }
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
            if was_marked && self.undo_stack.last().is_some_and(|(text, _)| *text == self.content) {
                self.undo_stack.pop();
            }
        }
        self.highlights.clear();
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                let (s, e) = if selected.start <= selected.end {
                    (selected.start, selected.end)
                } else {
                    (selected.end, selected.start)
                };
                self.range_from_utf16(
                    &(insertion_start_utf16 + s.min(inserted_length_utf16)
                        ..insertion_start_utf16 + e.min(inserted_length_utf16)),
                )
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;

        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = layout.position_for_offset(self.display_offset(range.start));
        let end = layout.position_for_offset(self.display_offset(range.end));
        let origin = layout.origin();
        Some(Bounds::from_corners(
            origin + start,
            origin + point(end.x, end.y + layout.line_height),
        ))
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range_utf16, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.last_layout.as_ref()?;
        layout.bounds.localize(&point)?;
        Some(self.offset_to_utf16(self.offset_for_text_position(layout, point - layout.origin())))
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .w_full()
            .min_w_0()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_backward))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::document_home))
            .on_action(cx.listener(Self::document_end))
            .on_action(cx.listener(Self::select_document_home))
            .on_action(cx.listener(Self::select_document_end))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::enter))
            .when(self.cancellable, |el| el.on_action(cx.listener(Self::cancel)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Drop for TextInput {
    fn drop(&mut self) {
        touch_keyboard::release(&self.focus_handle);
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ListEnter {
    Continue(String),
    ClearLine,
}

fn markdown_list_continuation(line: &str) -> Option<ListEnter> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    for prefix in ["- [ ] ", "- [x] ", "- [X] "] {
        if let Some(after) = rest.strip_prefix(prefix) {
            return Some(if after.trim().is_empty() {
                ListEnter::ClearLine
            } else {
                ListEnter::Continue(format!("{indent}- [ ] "))
            });
        }
    }
    for prefix in ["- ", "* ", "> "] {
        if let Some(after) = rest.strip_prefix(prefix) {
            return Some(if after.trim().is_empty() {
                ListEnter::ClearLine
            } else {
                ListEnter::Continue(format!("{indent}{prefix}"))
            });
        }
    }
    let digits_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_len > 0
        && let Some(after) = rest[digits_len..].strip_prefix(". ")
        && let Ok(n) = rest[..digits_len].parse::<u64>()
    {
        return Some(if after.trim().is_empty() {
            ListEnter::ClearLine
        } else {
            ListEnter::Continue(format!("{indent}{}. ", n.saturating_add(1)))
        });
    }
    None
}

fn adjust_lines_indent(text: &str, sel: Range<usize>, indent: bool) -> (String, Range<usize>) {
    let start_line_begin = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
    // If a non-empty selection ends right at the start of a line (right after `\n`), don't include that trailing line.
    let effective_end = if sel.end > sel.start && text[..sel.end].ends_with('\n') {
        sel.end - 1
    } else {
        sel.end
    };
    let end_line_end = text[effective_end..]
        .find('\n')
        .map_or(text.len(), |i| effective_end + i);

    let mut out = String::with_capacity(text.len() + 16);
    out.push_str(&text[..start_line_begin]);

    let mut new_start = sel.start;
    let mut new_end = sel.end;
    let mut line_start = start_line_begin;

    for (idx, line) in text[start_line_begin..end_line_end].split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if indent {
            out.push_str("  ");
            out.push_str(line);
            if line_start <= sel.start {
                new_start += 2;
            }
            if line_start <= effective_end {
                new_end += 2;
            }
        } else {
            let removed = if line.starts_with("  ") {
                2
            } else if line.starts_with(' ') || line.starts_with('\t') {
                1
            } else {
                0
            };
            out.push_str(&line[removed..]);
            if removed > 0 {
                if line_start <= sel.start {
                    let shift = (sel.start - line_start).min(removed);
                    new_start -= shift;
                }
                if line_start <= effective_end {
                    let shift = (sel.end - line_start).min(removed);
                    new_end -= shift;
                }
            }
        }
        line_start += line.len() + 1;
    }

    out.push_str(&text[end_line_end..]);
    (out, new_start..new_end.max(new_start))
}

/// Clamps `offset` to `text.len()` and backs up to the nearest valid UTF-8 char boundary.
fn snap_char_boundary(text: &str, offset: usize) -> usize {
    let mut idx = offset.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Converts Windows (`\r\n`) and legacy Mac (`\r`) line endings to `\n`.
fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Wires both [`Submitted`] (Enter) and [`Cancelled`] (Escape) for a dialog [`TextInput`]
/// in a single call so dialog inputs never forget Escape handling.
pub fn wire_dialog_input<V: 'static>(
    input: &Entity<TextInput>,
    cx: &mut Context<V>,
    mut on_submit: impl FnMut(&mut V, &str, &mut Context<V>) + 'static,
    mut on_cancel: impl FnMut(&mut V, &mut Context<V>) + 'static,
) -> [Subscription; 2] {
    let sub_submit = cx.subscribe(input, move |this, _, event: &Submitted, cx| {
        on_submit(this, &event.0, cx);
    });
    let sub_cancel = cx.subscribe(input, move |this, _, _: &Cancelled, cx| {
        on_cancel(this, cx);
    });
    [sub_submit, sub_cancel]
}

/// Window-aware variant of [`wire_dialog_input`] using [`Context::subscribe_in`].
pub fn wire_dialog_input_in<V: 'static>(
    input: &Entity<TextInput>,
    window: &Window,
    cx: &mut Context<V>,
    mut on_submit: impl FnMut(&mut V, &str, &mut Window, &mut Context<V>) + 'static,
    mut on_cancel: impl FnMut(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> [Subscription; 2] {
    let sub_submit = cx.subscribe_in(input, window, move |this, _, event: &Submitted, window, cx| {
        on_submit(this, &event.0, window, cx);
    });
    let sub_cancel = cx.subscribe_in(input, window, move |this, _, _: &Cancelled, window, cx| {
        on_cancel(this, window, cx);
    });
    [sub_submit, sub_cancel]
}

#[cfg(test)]
#[path = "text_input/tests.rs"]
mod tests;
