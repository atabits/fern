use gpui::{
    AnyElement, Anchor, App, Div, IntoElement, ParentElement, RenderOnce, Styled, Window,
    anchored, deferred, div, px,
};
use theme::ActiveTheme;

/// The frame of a popover menu. Build the content with children, then attach it with
/// [`PopoverAttachment`].
pub fn popover(cx: &App) -> Div {
    let colors = cx.theme().colors();
    div()
        .flex()
        .flex_col()
        .p_1p5()
        .rounded(px(8.))
        .bg(colors.elevated_surface_background)
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .text_color(colors.text)
        .text_size(px(13.))
}

/// Where a popover attaches to the element it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    BelowLeft,
    BelowRight,
    /// Opens upwards, for buttons at the bottom of a panel.
    AboveRight,
    RightTop,
    /// To the right, growing upwards: for buttons at the bottom of the sidebar.
    RightBottom,
    /// Beside the parent on its left, level with its top: a side menu of a menu item.
    LeftSide,
    /// Beside the parent on its right, level with its top.
    RightSide,
}

/// Renders a popover on top of the rest of the UI, attached to its parent, which must be
/// `relative()`. The popover is moved inside the window when it would overflow.
#[derive(IntoElement)]
pub struct PopoverAttachment {
    placement: Placement,
    content: AnyElement,
}

impl PopoverAttachment {
    pub fn new(placement: Placement, content: impl IntoElement) -> Self {
        Self { placement, content: content.into_any_element() }
    }
}

impl RenderOnce for PopoverAttachment {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (attach, anchor) = match self.placement {
            Placement::BelowLeft => (div().absolute().top(px(30.)).left_0(), Anchor::TopLeft),
            Placement::BelowRight => (div().absolute().top(px(30.)).right_0(), Anchor::TopRight),
            Placement::AboveRight => (div().absolute().top(px(-4.)).right_0(), Anchor::BottomRight),
            Placement::RightTop => (div().absolute().top_0().left(px(38.)), Anchor::TopLeft),
            Placement::RightBottom => (div().absolute().bottom_0().left(px(38.)), Anchor::BottomLeft),
            // The menu's own padding (6 px) is kept between the two menus.
            Placement::LeftSide => (div().absolute().top(px(-6.)).left(px(-8.)), Anchor::TopRight),
            Placement::RightSide => (div().absolute().top(px(-6.)).right(px(-8.)), Anchor::TopLeft),
        };
        // Side menus open from inside another popover, so they paint above it.
        let priority = if matches!(self.placement, Placement::LeftSide | Placement::RightSide) { 2 } else { 1 };
        attach.child(
            deferred(anchored().anchor(anchor).snap_to_window_with_margin(px(8.)).child(self.content))
                .priority(priority),
        )
    }
}
