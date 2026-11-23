use gpui::{
    App, AvailableSpace, Bounds, ContentMask, ElementId, ElementInputHandler, Entity,
    GlobalElementId, LayoutId, PaintQuad, Pixels, Point, SharedString, Style, TextRun,
    UnderlineStyle, Window, WrappedLine, fill, point, prelude::*, px, relative, size,
};
use theme::ActiveTheme;

use super::TextInput;

pub(super) const CARET_WIDTH: Pixels = px(2.);

/// The layout from the last frame, used for mouse hit-testing, up/down arrows and IME.
pub(super) struct TextLayout {
    /// Paragraphs (split at `\n`), each with its offset in `content` and its vertical position.
    pub(super) paragraphs: Vec<Paragraph>,
    pub(super) line_height: Pixels,
    pub(super) bounds: Bounds<Pixels>,
    /// Horizontal viewport position. Text coordinates remain independent of scrolling.
    pub(super) scroll_left: Pixels,
    /// The layout is of the placeholder, so its positions do not map to the text.
    pub(super) is_placeholder: bool,
}

pub(super) struct Paragraph {
    pub(super) line: WrappedLine,
    pub(super) start: usize,
    pub(super) top: Pixels,
}

impl TextLayout {
    pub(super) fn origin(&self) -> Point<Pixels> {
        self.bounds.origin - point(self.scroll_left, px(0.))
    }

    /// The caret position for a byte offset in unscrolled text coordinates.
    pub(super) fn position_for_offset(&self, offset: usize) -> Point<Pixels> {
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
    pub(super) fn offset_for_position(&self, position: Point<Pixels>) -> usize {
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

pub(super) struct TextElement {
    pub(super) input: Entity<TextInput>,
}

pub(super) struct PrepaintState {
    layout: Option<TextLayout>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    /// Tinted backgrounds under the highlighted ranges.
    highlights: Vec<PaintQuad>,
    /// Vertical offset aligning single-line input text with `LabelText`.
    text_y_offset: Pixels,
}

fn text_width(width: Pixels) -> Pixels {
    (width - CARET_WIDTH).max(px(0.))
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A password field's text: one "*" per Unicode scalar value (`char`), not per UTF-8 byte.
pub(super) fn masked_text(text: &str) -> SharedString {
    "*".repeat(text.chars().count()).into()
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
        (masked_text(&input.content), input.text_color.unwrap_or(style.color))
    } else {
        (input.content.clone(), input.text_color.unwrap_or(style.color))
    };
    let text_len = text.len();
    let run = TextRun {
        len: text_len,
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = match input.marked_range.as_ref() {
        Some(marked) if !is_placeholder => {
            let start = input.display_offset(marked.start).min(text_len);
            let end = input.display_offset(marked.end).clamp(start, text_len);
            vec![
                TextRun { len: start, ..run.clone() },
                TextRun {
                    len: end - start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun { len: text_len - end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        }
        // Highlighted ranges take the accent color.
        _ if !is_placeholder && !input.highlights.is_empty() => {
            let mut runs = Vec::new();
            let mut at = 0;
            for range in &input.highlights {
                let start = input.display_offset(range.start).clamp(at, text_len);
                let end = input.display_offset(range.end).clamp(start, text_len);
                if start > at {
                    runs.push(TextRun { len: start - at, ..run.clone() });
                }
                if end > start {
                    runs.push(TextRun { len: end - start, color: highlight_color, ..run.clone() });
                }
                at = end;
            }
            if at < text_len {
                runs.push(TextRun { len: text_len - at, ..run });
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
        let highlight_color = input.caret_color.unwrap_or(cx.theme().colors().accent);
        let (text, runs, _) = display_runs(input, window, placeholder_color, highlight_color);
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
        let highlight_color = input.caret_color.unwrap_or(theme.colors().text_accent);
        let (text, runs, is_placeholder) =
            display_runs(input, window, theme.colors().text_placeholder, highlight_color);
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
                let caret_x = layout.position_for_offset(input.display_offset(input.cursor_offset())).x;
                layout.scroll_left = layout.scroll_left
                    .max(caret_x - viewport_width)
                    .min(caret_x)
                    .clamp(px(0.), max_scroll);
            }
        }
        let origin = layout.origin() + point(px(0.), text_y_offset);

        let selected_range = input.selected_range.clone();
        let caret_color = input.caret_color.unwrap_or(theme.colors().accent);
        let selection_color = input
            .caret_color
            .or(input.text_color)
            .map_or(theme.colors().text_selection, |c| c.opacity(0.22));
        let highlight_bg = input
            .caret_color
            .or(input.text_color)
            .map_or_else(|| theme.colors().accent.opacity(0.18), |c| c.opacity(0.18));
        let mut selection = Vec::new();
        let mut cursor = None;
        if selected_range.is_empty() {
            if !input.read_only {
                let position = layout.position_for_offset(input.display_offset(input.cursor_offset()));
                cursor = Some(fill(
                    Bounds::new(origin + position, size(CARET_WIDTH, line_height)),
                    caret_color,
                ));
            }
        } else {
            let start = layout.position_for_offset(input.display_offset(selected_range.start));
            let end = layout.position_for_offset(input.display_offset(selected_range.end));
            let rect = |from: Point<Pixels>, to_x: Pixels| {
                fill(
                    Bounds::from_corners(
                        origin + from,
                        origin + point(to_x, from.y + line_height),
                    ),
                    selection_color,
                )
            };
            if start.y == end.y {
                selection.push(rect(start, end.x));
            } else {
                selection.push(rect(start, bounds.size.width));
                if line_height > px(0.) {
                    let mut y = start.y + line_height;
                    while y < end.y {
                        selection.push(rect(point(px(0.), y), bounds.size.width));
                        y += line_height;
                    }
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
                    let start = layout.position_for_offset(input.display_offset(range.start));
                    let end = layout.position_for_offset(input.display_offset(range.end));
                    (start.y == end.y).then(|| {
                        fill(
                            Bounds::from_corners(
                                origin + point(start.x - px(3.), start.y + px(1.)),
                                origin + point(end.x + px(3.), end.y + line_height - px(1.)),
                            ),
                            highlight_bg,
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
