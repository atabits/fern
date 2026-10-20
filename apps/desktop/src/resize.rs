//! Side panels resized by dragging their inner edge: the lists column and the task details.
//!
//! The edge is a strip a few pixels wide along the panel's border, with the column-resize cursor;
//! it lights up in the accent color while hovered or dragged. A double click puts the panel back
//! to its default width. The widths are the same for every list and are kept across restarts.

use gpui::CursorStyle;
use ui::prelude::*;

/// The strip along the border that takes the drag.
const HANDLE_WIDTH: f32 = 6.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    /// The lists column, dragged by its right edge.
    Lists,
    /// The task details, dragged by its left edge.
    Detail,
}

/// The value carried while an edge is dragged. It shows nothing: the panel itself follows.
#[derive(Clone, Copy)]
pub struct DraggedEdge(pub Panel);

impl Render for DraggedEdge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// The edge of `panel`, laid over the panel's border inside it, lit while `dragged`; `on_reset`
/// runs on a double click.
pub fn handle(panel: Panel, dragged: bool, on_reset: impl Fn(&mut Window, &mut App) + 'static, cx: &App) -> impl IntoElement {
    let accent = cx.theme().colors().accent;
    let id = match panel {
        Panel::Lists => "lists-resize",
        Panel::Detail => "detail-resize",
    };
    div()
        .id(id)
        .group(id)
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(HANDLE_WIDTH))
        .map(|el| match panel {
            Panel::Lists => el.right_0(),
            Panel::Detail => el.left_0(),
        })
        .cursor(CursorStyle::ResizeColumn)
        .child(
            // The line sits on the panel's border, which is on the strip's outer side.
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(2.))
                .map(|el| match panel {
                    Panel::Lists => el.right_0(),
                    Panel::Detail => el.left_0(),
                })
                .group_hover(id, |el| el.bg(accent))
                .when(dragged, |el| el.bg(accent)),
        )
        .on_drag(DraggedEdge(panel), |edge, _, _, cx| cx.new(|_| *edge))
        .on_click(move |event, window, cx| {
            if event.click_count() == 2 {
                on_reset(window, cx);
            }
        })
}
