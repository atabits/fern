//! A text input. GPUI has no ready-made input widget; this one starts from the
//! `gpui/examples/input.rs` example in the Zed repository and adds soft wrapping and a
//! multi-line mode.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px, relative,
    size,
};
use unicode_segmentation::*;

use theme::ActiveTheme;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
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

impl EventEmitter<Changed> for TextInput {}
impl EventEmitter<Cancelled> for TextInput {}

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
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<TextLayout>,
    is_selecting: bool,
    /// Byte ranges shown in the accent color on a tinted background, such as the date and tags
    /// quick add recognized.
    highlights: Vec<Range<usize>>,
    /// A password field: the text shows as "*", one per byte so that every offset stays the
    /// same, and it cannot be copied.
    masked: bool,
}

/// The layout from the last frame, used for mouse hit-testing, up/down arrows and IME.
struct TextLayout {
    /// Paragraphs (split at `\n`), each with its offset in `content` and its vertical position.
    paragraphs: Vec<Paragraph>,
    line_height: Pixels,
    bounds: Bounds<Pixels>,
    /// The layout is of the placeholder, so its positions do not map to the text.
    is_placeholder: bool,
}

struct Paragraph {
    line: WrappedLine,
    start: usize,
    top: Pixels,
}

impl TextLayout {
    /// The caret position for a byte offset, relative to the input's top-left corner.
    fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
        if self.is_placeholder {
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

    /// The closest offset for a point relative to the input's top-left corner.
    fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        if self.is_placeholder {
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
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            is_selecting: false,
            highlights: Vec::new(),
            masked: false,
        }
    }

    /// Makes this a password field.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replaces the text without emitting [`Changed`], for loading data from outside.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.highlights.clear();
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

    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::MultiLine {
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

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        window.request_virtual_keyboard();

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), window, cx);
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
        layout.offset_for_position(point(position.x, target_y))
    }

    /// The start or end of the visual line the caret is on.
    fn visual_line_edge(&self, end: bool) -> usize {
        let cursor = self.cursor_offset();
        let Some(layout) = self.last_layout.as_ref() else {
            return if end { self.content.len() } else { 0 };
        };
        let y = layout.position_for_offset(cursor).y + layout.line_height * 0.5;
        let x = if end { px(f32::MAX / 4.) } else { px(0.) };
        layout.offset_for_position(point(x, y))
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
        layout.offset_for_position(position - layout.bounds.origin)
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
        let origin = layout.bounds.origin;
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
        let local = layout.bounds.localize(&point)?;
        Some(self.offset_to_utf16(layout.offset_for_position(local)))
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
        (input.placeholder.clone(), placeholder_color)
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
                runs.push(TextRun { len: text.len() - at, ..run.clone() });
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
            let lines = shape(text.clone(), &runs, font_size, width, window);
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
            display_runs(input, window, theme.colors().text_placeholder, theme.colors().accent);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let wrap_width = input.mode.wraps().then_some(bounds.size.width);
        let lines = shape(text, &runs, font_size, wrap_width, window);

        let mut paragraphs = Vec::with_capacity(lines.len());
        let (mut start, mut top) = (0, px(0.));
        for line in lines {
            let len = line.len();
            let height = line.size(line_height).height;
            paragraphs.push(Paragraph { line, start, top });
            start += len + 1;
            top += height;
        }
        let layout = TextLayout { paragraphs, line_height, bounds, is_placeholder };

        let selected_range = input.selected_range.clone();
        let mut selection = Vec::new();
        let mut cursor = None;
        if selected_range.is_empty() {
            let position = layout.position_for_offset(input.cursor_offset());
            cursor = Some(fill(
                Bounds::new(bounds.origin + position, size(px(2.), line_height)),
                theme.colors().accent,
            ));
        } else {
            let start = layout.position_for_offset(selected_range.start);
            let end = layout.position_for_offset(selected_range.end);
            let rect = |from: Point<Pixels>, to_x: Pixels| {
                fill(
                    Bounds::from_corners(
                        bounds.origin + from,
                        bounds.origin + point(to_x, from.y + line_height),
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
                                bounds.origin + point(start.x - px(3.), start.y + px(1.)),
                                bounds.origin + point(end.x + px(3.), end.y + line_height - px(1.)),
                            ),
                            theme.colors().accent.opacity(0.18),
                        )
                        .corner_radii(px(4.))
                    })
                })
                .collect()
        };

        PrepaintState { layout: Some(layout), cursor, selection, highlights }
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
        for quad in prepaint.highlights.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        let layout = prepaint.layout.take().unwrap();
        for paragraph in &layout.paragraphs {
            paragraph
                .line
                .paint(
                    bounds.origin + point(px(0.), paragraph.top),
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
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn masked_text_keeps_offsets() {
        let text = "s3cret-é";
        let masked = super::masked_text(text);
        assert_eq!(masked.len(), text.len());
        assert!(masked.chars().all(|c| c == '*'));
    }
}
