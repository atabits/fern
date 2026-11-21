//! A text input. GPUI has no ready-made input widget; this one starts from the
//! `gpui/examples/input.rs` example in the Zed repository and adds soft wrapping and a
//! multi-line mode.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style, Subscription, TextRun,
    UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative,
    size,
};
use unicode_segmentation::*;

use theme::ActiveTheme;

use crate::touch_keyboard;

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
    placeholder_color: Option<gpui::Hsla>,
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
    /// A password field: the text shows as "*", one per byte so that every offset stays the
    /// same, and it cannot be copied.
    masked: bool,
    /// When true on a single-line input, pasting 2+ non-empty lines emits [`MultiLinePasted`]
    /// instead of flattening newlines into spaces (used for Batch Add Tasks).
    intercept_multiline_paste: bool,
    /// Hides the touch keyboard this input opened once it loses focus.
    keyboard_blur: Option<Subscription>,
    undo_stack: Vec<(SharedString, Range<usize>)>,
    redo_stack: Vec<(SharedString, Range<usize>)>,
}

/// The layout from the last frame, used for mouse hit-testing, up/down arrows and IME.
struct TextLayout {
    /// Paragraphs (split at `\n`), each with its offset in `content` and its vertical position.
    paragraphs: Vec<Paragraph>,
    line_height: Pixels,
    bounds: Bounds<Pixels>,
    /// Horizontal viewport position. Text coordinates remain independent of scrolling.
    scroll_left: Pixels,
    /// The layout is of the placeholder, so its positions do not map to the text.
    is_placeholder: bool,
}

struct Paragraph {
    line: WrappedLine,
    start: usize,
    top: Pixels,
}

impl TextLayout {
    fn origin(&self) -> Point<Pixels> {
        self.bounds.origin - point(self.scroll_left, px(0.))
    }

    /// The caret position for a byte offset in unscrolled text coordinates.
    fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
        if self.is_placeholder || self.paragraphs.is_empty() {
            return point(px(0.), px(0.));
        }
        let paragraph = self
            .paragraphs
            .iter()
            .rev()
            .find(|p| p.start <= offset)
            .unwrap_or(&self.paragraphs[0]);
        let local = (offset - paragraph.start).min(paragraph.line.len());
        let position = paragraph
            .line
            .position_for_index(local, self.line_height)
            .unwrap_or_default();
        point(position.x, position.y + paragraph.top)
    }

    /// The closest offset for a point in unscrolled text coordinates.
    fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        if self.is_placeholder || self.paragraphs.is_empty() {
            return 0;
        }
        let paragraph = self
            .paragraphs
            .iter()
            .rev()
            .find(|p| p.top <= position.y)
            .unwrap_or(&self.paragraphs[0]);
        let height = paragraph.line.size(self.line_height).height;
        let local = point(
            position.x.max(px(0.)),
            (position.y - paragraph.top).clamp(px(0.), (height - px(1.)).max(px(0.))),
        );
        let index = match paragraph
            .line
            .closest_index_for_position(local, self.line_height)
        {
            Ok(index) | Err(index) => index,
        };
        paragraph.start + index
    }
}

impl TextInput {
    pub fn new(mode: Mode, placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            mode,
            content: "".into(),
            placeholder: placeholder.into(),
            placeholder_color: None,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            pending_click: None,
            is_selecting: false,
            cancellable: true,
            highlights: Vec::new(),
            masked: false,
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

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_placeholder_color(&mut self, color: gpui::Hsla, cx: &mut Context<Self>) {
        self.placeholder_color = Some(color);
        cx.notify();
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
        self.push_undo_snapshot();
        let len = text.len();
        let clamp_boundary = |offset: usize| {
            let mut idx = offset.min(len);
            while idx > 0 && !text.is_char_boundary(idx) {
                idx -= 1;
            }
            idx
        };
        let start = clamp_boundary(selection.start);
        let end = clamp_boundary(selection.end).max(start);
        self.content = text.into();
        self.selected_range = start..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.pending_click = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    pub fn clear(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.set_text("", cx);
        window.notify_text_input_changed(&self.focus_handle);
    }

    /// Marks ranges of the text (byte offsets) as recognized; out-of-range ones are ignored.
    pub fn set_highlights(&mut self, ranges: Vec<Range<usize>>, cx: &mut Context<Self>) {
        let len = self.content.len();
        let ranges: Vec<Range<usize>> = ranges.into_iter().filter(|r| r.start < r.end && r.end <= len).collect();
        if ranges != self.highlights {
            self.highlights = ranges;
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
        let Some((prev_text, prev_sel)) = self.undo_stack.pop() else {
            cx.propagate();
            return;
        };
        self.redo_stack.push((self.content.clone(), self.selected_range.clone()));
        self.content = prev_text;
        self.selected_range = prev_sel;
        self.selection_reversed = false;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        let Some((next_text, next_sel)) = self.redo_stack.pop() else {
            cx.propagate();
            return;
        };
        self.undo_stack.push((self.content.clone(), self.selected_range.clone()));
        self.content = next_text;
        self.selected_range = next_sel;
        self.selection_reversed = false;
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(Changed);
        cx.notify();
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::MultiLine {
            if self.selected_range.is_empty() {
                let cursor = self.selected_range.end;
                let line_start = self.content[..cursor].rfind('\n').map_or(0, |i| i + 1);
                let before_cursor = &self.content[line_start..cursor];
                match markdown_list_continuation(before_cursor) {
                    Some(ListEnter::Continue(prefix)) => {
                        self.replace_text_in_range(None, &format!("\n{prefix}"), window, cx);
                        return;
                    }
                    Some(ListEnter::ClearLine) => {
                        self.selected_range = line_start..cursor;
                        self.replace_text_in_range(None, "", window, cx);
                        return;
                    }
                    None => {}
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

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
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
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if self.mode == Mode::SingleLine && self.intercept_multiline_paste {
                let lines: Vec<String> = text
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
                text
            } else {
                text.replace('\n', " ")
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
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
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

    /// The offset one line above (`direction = -1`) or below (`1`) at the same x.
    fn vertical_offset(&self, direction: f32) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.last_layout.as_ref() else {
            return cursor;
        };
        let position = layout.position_for_offset(cursor);
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
        let y = layout.position_for_offset(cursor).y + layout.line_height * 0.5;
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
            // Each mask glyph represents a byte, but editing must never split the
            // original UTF-8 character or an emoji/combining-character sequence.
            self.content.grapheme_indices(true)
                .map(|(index, _)| index)
                .chain(std::iter::once(self.content.len()))
                .min_by_key(|index| index.abs_diff(offset))
                .unwrap_or(0)
        } else {
            offset
        }
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
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
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
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
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.push_undo_snapshot();
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
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
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.push_undo_snapshot();
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        let insertion_start_utf16 = self.offset_to_utf16(range.start);
        let inserted_length_utf16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|selected| {
                self.range_from_utf16(
                    &(insertion_start_utf16 + selected.start.min(inserted_length_utf16)
                        ..insertion_start_utf16 + selected.end.min(inserted_length_utf16)),
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
        let start = layout.position_for_offset(range.start);
        let end = layout.position_for_offset(range.end);
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

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    layout: Option<TextLayout>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    /// Tinted backgrounds under the highlighted ranges.
    highlights: Vec<PaintQuad>,
    /// Vertical offset aligning single-line input text with `LabelText`.
    text_y_offset: Pixels,
}

const CARET_WIDTH: Pixels = px(2.);

fn text_width(width: Pixels) -> Pixels {
    (width - CARET_WIDTH).max(px(0.))
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A password field's text: one "*" per byte, so that offsets into it match the real text.
fn masked_text(text: &str) -> SharedString {
    "*".repeat(text.len()).into()
}

/// The text to display and its style runs (placeholder, IME underline).
fn display_runs(
    input: &TextInput,
    window: &Window,
    placeholder_color: gpui::Hsla,
    highlight_color: gpui::Hsla,
) -> (SharedString, Vec<TextRun>, bool) {
    let style = window.text_style();
    let is_placeholder = input.content.is_empty();
    let (text, color) = if is_placeholder {
        (input.placeholder.clone(), input.placeholder_color.unwrap_or(placeholder_color))
    } else if input.masked {
        (masked_text(&input.content), style.color)
    } else {
        (input.content.clone(), style.color)
    };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = match input.marked_range.as_ref() {
        Some(marked) if !is_placeholder => vec![
            TextRun { len: marked.start, ..run.clone() },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun { len: text.len() - marked.end, ..run },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect(),
        // Highlighted ranges take the accent color.
        _ if !is_placeholder && !input.highlights.is_empty() => {
            let mut runs = Vec::new();
            let mut at = 0;
            for range in &input.highlights {
                if range.start > at {
                    runs.push(TextRun { len: range.start - at, ..run.clone() });
                }
                runs.push(TextRun { len: range.end - range.start, color: highlight_color, ..run.clone() });
                at = range.end;
            }
            if at < text.len() {
                runs.push(TextRun { len: text.len() - at, ..run });
            }
            runs
        }
        _ => vec![run],
    };
    (text, runs, is_placeholder)
}

fn shape(
    text: SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    wrap_width: Option<Pixels>,
    window: &Window,
) -> Vec<WrappedLine> {
    window
        .text_system()
        .shape_text(text, font_size, runs, wrap_width, None)
        .map(|lines| lines.into_vec())
        .unwrap_or_default()
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.min_size.width = px(0.).into();
        let line_height = window.line_height();
        let placeholder_color = cx.theme().colors().text_placeholder;
        let input = self.input.read(cx);
        if !input.mode.wraps() {
            style.size.height = line_height.into();
            return (window.request_layout(style, [], cx), ());
        }

        // The height depends on the width, so it is measured during layout.
        let (text, runs, _) = display_runs(input, window, placeholder_color, cx.theme().colors().accent);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let layout_id = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            let lines = shape(text.clone(), &runs, font_size, width.map(text_width), window);
            let height = lines
                .iter()
                .map(|line| line.size(line_height).height)
                .fold(px(0.), |a, b| a + b)
                .max(line_height);
            size(width.unwrap_or_default(), height)
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.theme().clone();
        let input = self.input.read(cx);
        let line_height = window.line_height();
        let (text, runs, is_placeholder) =
            display_runs(input, window, theme.colors().text_placeholder, theme.colors().text_accent);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let wrap_width = input.mode.wraps().then_some(text_width(bounds.size.width));
        let lines = shape(text, &runs, font_size, wrap_width, window);
        let text_y_offset = if !input.mode.wraps()
            && let [line] = lines.as_slice()
        {
            ui::align_icons_to_input(bounds, line, line_height, window, cx)
        } else {
            px(0.)
        };

        let mut paragraphs = Vec::with_capacity(lines.len());
        let (mut start, mut top) = (0, px(0.));
        for line in lines {
            let len = line.len();
            let height = line.size(line_height).height;
            paragraphs.push(Paragraph { line, start, top });
            start += len + 1;
            top += height;
        }
        let mut layout = TextLayout {
            paragraphs,
            line_height,
            bounds,
            scroll_left: px(0.),
            is_placeholder,
        };
        let pending_offset = input.pending_click.map(|position| {
            if position.y < layout.bounds.top() {
                0
            } else if position.y > layout.bounds.bottom() {
                input.content.len()
            } else {
                input.offset_for_text_position(&layout, position - layout.origin())
            }
        });
        if let Some(offset) = pending_offset {
            let focus_handle = self.input.update(cx, |input, _| {
                input.pending_click = None;
                input.selected_range = offset..offset;
                input.selection_reversed = false;
                input.focus_handle.clone()
            });
            window.notify_text_selection_changed(&focus_handle);
        }
        let input = self.input.read(cx);
        if !input.mode.wraps() && !is_placeholder {
            let viewport_width = text_width(bounds.size.width);
            let content_width = layout.paragraphs.iter()
                .map(|paragraph| paragraph.line.width())
                .fold(px(0.), Pixels::max);
            let max_scroll = (content_width - viewport_width).max(px(0.));
            layout.scroll_left = input.last_layout.as_ref()
                .map_or(px(0.), |layout| layout.scroll_left)
                .min(max_scroll);
            if input.focus_handle.is_focused(window) || input.is_selecting {
                let caret_x = layout.position_for_offset(input.cursor_offset()).x;
                layout.scroll_left = layout.scroll_left
                    .max(caret_x - viewport_width)
                    .min(caret_x)
                    .clamp(px(0.), max_scroll);
            }
        }
        let origin = layout.origin() + point(px(0.), text_y_offset);

        let selected_range = input.selected_range.clone();
        let mut selection = Vec::new();
        let mut cursor = None;
        if selected_range.is_empty() {
            let position = layout.position_for_offset(input.cursor_offset());
            cursor = Some(fill(
                Bounds::new(origin + position, size(CARET_WIDTH, line_height)),
                theme.colors().accent,
            ));
        } else {
            let start = layout.position_for_offset(selected_range.start);
            let end = layout.position_for_offset(selected_range.end);
            let rect = |from: Point<Pixels>, to_x: Pixels| {
                fill(
                    Bounds::from_corners(
                        origin + from,
                        origin + point(to_x, from.y + line_height),
                    ),
                    theme.colors().text_selection,
                )
            };
            if start.y == end.y {
                selection.push(rect(start, end.x));
            } else {
                selection.push(rect(start, bounds.size.width));
                let mut y = start.y + line_height;
                while y < end.y {
                    selection.push(rect(point(px(0.), y), bounds.size.width));
                    y += line_height;
                }
                selection.push(rect(point(px(0.), end.y), end.x));
            }
        }

        // A rounded tint under each highlighted range on one line, as the original shows them.
        let highlights = if is_placeholder {
            Vec::new()
        } else {
            input
                .highlights
                .iter()
                .filter_map(|range| {
                    let start = layout.position_for_offset(range.start);
                    let end = layout.position_for_offset(range.end);
                    (start.y == end.y).then(|| {
                        fill(
                            Bounds::from_corners(
                                origin + point(start.x - px(3.), start.y + px(1.)),
                                origin + point(end.x + px(3.), end.y + line_height - px(1.)),
                            ),
                            theme.colors().accent.opacity(0.18),
                        )
                        .corner_radii(px(4.))
                    })
                })
                .collect()
        };

        PrepaintState { layout: Some(layout), cursor, selection, highlights, text_y_offset }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let Some(layout) = prepaint.layout.take() else { return };
        // WrappedLine's bounds control alignment, not clipping. Keep every text layer
        // inside the field, independently of whether its parent also clips overflow.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in prepaint.highlights.drain(..) {
                window.paint_quad(quad);
            }
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }
            for paragraph in &layout.paragraphs {
                paragraph
                    .line
                    .paint(
                        layout.origin() + point(px(0.), paragraph.top + prepaint.text_y_offset),
                        layout.line_height,
                        gpui::TextAlign::Left,
                        Some(bounds),
                        window,
                        cx,
                    )
                    .ok();
            }

            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(layout);
        });
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

#[cfg(test)]
#[path = "text_input/tests.rs"]
mod tests;
