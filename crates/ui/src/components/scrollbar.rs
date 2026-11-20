use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Bounds, CursorStyle, ElementId, HitboxBehavior, Hsla, IntoElement, ListState,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels,
    RenderOnce, ScrollHandle, Size, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, canvas, div, fill, point, px, size,
};
use theme::ActiveTheme as _;

const TRACK_THICKNESS: f32 = 12.;
const THUMB_REST_THICKNESS: f32 = 4.;
const THUMB_ACTIVE_THICKNESS: f32 = 6.;
const EDGE_INSET: f32 = 2.;
const END_MARGIN: f32 = 4.;
const COMPACT_END_MARGIN: f32 = 2.;
const MIN_THUMB_LENGTH: f32 = 28.;
const COMPACT_MIN_THUMB_LENGTH: f32 = 20.;

const THUMB_REST_OPACITY: f32 = 0.22;
const THUMB_HOVER_OPACITY: f32 = 0.42;
const THUMB_DRAG_OPACITY: f32 = 0.58;
const TRACK_ACTIVE_OPACITY: f32 = 0.06;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Clone)]
enum ScrollTarget {
    Handle(ScrollHandle),
    List(ListState),
}

impl ScrollTarget {
    fn max_offset(&self) -> gpui::Point<Pixels> {
        match self {
            Self::Handle(handle) => handle.max_offset(),
            Self::List(list) => list.max_offset_for_scrollbar(),
        }
    }

    fn offset(&self) -> gpui::Point<Pixels> {
        match self {
            Self::Handle(handle) => handle.offset(),
            Self::List(list) => list.scroll_px_offset_for_scrollbar(),
        }
    }

    fn viewport_size(&self) -> Size<Pixels> {
        match self {
            Self::Handle(handle) => handle.bounds().size,
            Self::List(list) => list.viewport_bounds().size,
        }
    }

    fn set_axis_offset(&self, axis: Axis, fraction: f32, overflow: Pixels) {
        match self {
            Self::Handle(handle) => {
                let current = handle.offset();
                let next = match axis {
                    Axis::Vertical => point(current.x, -overflow * fraction),
                    Axis::Horizontal => point(-overflow * fraction, current.y),
                };
                handle.set_offset(next);
            }
            Self::List(list) => {
                // `ListState::max_offset_for_scrollbar` omits container padding (e.g. `.pb_4()`),
                // whereas `set_offset_from_scrollbar` clamps against content + padding. Push past
                // `overflow` at the end of the track so dragging to the bottom reveals the end.
                let target_y = if fraction >= 0.995 {
                    -(overflow + px(1000.))
                } else {
                    -overflow * fraction
                };
                list.set_offset_from_scrollbar(point(px(0.), target_y));
            }
        }
    }

    fn drag_started(&self) {
        if let Self::List(list) = self {
            list.scrollbar_drag_started();
        }
    }

    fn drag_ended(&self) {
        if let Self::List(list) = self {
            list.scrollbar_drag_ended();
        }
    }
}

#[derive(Clone, Default)]
struct DragState {
    grab_offset: Rc<Cell<Option<Pixels>>>,
}

/// A subtle overlay vertical scrollbar rendered at the right edge of a scrollable viewport.
#[derive(Clone, IntoElement)]
pub struct VerticalScrollbar {
    id: Option<ElementId>,
    target: ScrollTarget,
    drag: DragState,
    inset: Pixels,
    color: Option<Hsla>,
    compact: bool,
}

impl Default for VerticalScrollbar {
    fn default() -> Self {
        Self::new()
    }
}

impl VerticalScrollbar {
    /// Creates an unkeyed vertical scrollbar with its own `ScrollHandle`.
    pub fn new() -> Self {
        Self {
            id: None,
            target: ScrollTarget::Handle(ScrollHandle::new()),
            drag: DragState::default(),
            inset: px(0.),
            color: None,
            compact: false,
        }
    }

    /// Creates a vertical scrollbar tracking a `ScrollHandle` with keyed drag state.
    pub fn for_handle(id: impl Into<ElementId>, handle: ScrollHandle) -> Self {
        Self {
            id: Some(id.into()),
            target: ScrollTarget::Handle(handle),
            drag: DragState::default(),
            inset: px(0.),
            color: None,
            compact: false,
        }
    }

    /// Creates a vertical scrollbar tracking a virtualized `ListState` with keyed drag state.
    pub fn for_list(id: impl Into<ElementId>, list: ListState) -> Self {
        Self {
            id: Some(id.into()),
            target: ScrollTarget::List(list),
            drag: DragState::default(),
            inset: px(0.),
            color: None,
            compact: false,
        }
    }

    /// Insets the scrollbar away from the right edge (e.g. to clear a panel resize handle).
    pub fn inset(mut self, inset: Pixels) -> Self {
        self.inset = inset;
        self
    }

    /// Overrides the base foreground color used for the thumb and hover track.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Uses tighter end margins and a smaller minimum thumb size for compact popovers.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

impl RenderOnce for VerticalScrollbar {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let drag = resolve_drag_state(self.id.as_ref(), &self.drag, window);
        render_scrollbar(
            Axis::Vertical,
            self.target,
            drag,
            self.inset,
            self.color,
            self.compact,
        )
    }
}

/// A subtle overlay horizontal scrollbar rendered at the bottom edge of a scrollable viewport.
#[derive(Clone, IntoElement)]
pub struct HorizontalScrollbar {
    id: Option<ElementId>,
    target: ScrollTarget,
    drag: DragState,
    inset: Pixels,
    color: Option<Hsla>,
    compact: bool,
}

impl HorizontalScrollbar {
    /// Creates a horizontal scrollbar tracking a `ScrollHandle` with keyed drag state.
    pub fn for_handle(id: impl Into<ElementId>, handle: ScrollHandle) -> Self {
        Self {
            id: Some(id.into()),
            target: ScrollTarget::Handle(handle),
            drag: DragState::default(),
            inset: px(0.),
            color: None,
            compact: false,
        }
    }

    /// Insets the scrollbar away from the bottom edge.
    pub fn inset(mut self, inset: Pixels) -> Self {
        self.inset = inset;
        self
    }

    /// Overrides the base foreground color used for the thumb and hover track.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Uses tighter end margins and a smaller minimum thumb size for compact containers.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

impl RenderOnce for HorizontalScrollbar {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let drag = resolve_drag_state(self.id.as_ref(), &self.drag, window);
        render_scrollbar(
            Axis::Horizontal,
            self.target,
            drag,
            self.inset,
            self.color,
            self.compact,
        )
    }
}

/// A vertically scrollable wrapper that pairs a `Stateful<Div>` viewport with a [`VerticalScrollbar`].
///
/// Layout and container styles (`flex_1`, `min_h_0`, `max_h`, `h`, `bg`, `border`, `rounded`)
/// apply to the outer wrapper while scrolling happens inside the inner viewport.
#[derive(IntoElement)]
pub struct ScrollView {
    id: ElementId,
    viewport: gpui::Stateful<gpui::Div>,
    outer: gpui::Div,
    handle: Option<ScrollHandle>,
    inset: Pixels,
    color: Option<Hsla>,
    compact: bool,
}

/// Wraps a `Stateful<Div>` viewport in a [`ScrollView`] with an overlay [`VerticalScrollbar`].
pub fn v_scroll(id: impl Into<ElementId>, viewport: gpui::Stateful<gpui::Div>) -> ScrollView {
    ScrollView {
        id: id.into(),
        viewport,
        outer: div().relative().flex().flex_col().min_h_0(),
        handle: None,
        inset: px(0.),
        color: None,
        compact: false,
    }
}

impl ScrollView {
    /// Tracks scroll state with an externally managed `ScrollHandle` instead of an automatic one.
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.handle = Some(handle.clone());
        self
    }

    /// Insets the overlay scrollbar away from the right edge.
    pub fn inset(mut self, inset: Pixels) -> Self {
        self.inset = inset;
        self
    }

    /// Overrides the base foreground color of the scrollbar thumb and hover track.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Renders a compact scrollbar suited for popovers and small dropdowns.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

impl Styled for ScrollView {
    fn style(&mut self) -> &mut StyleRefinement {
        self.outer.style()
    }
}

impl RenderOnce for ScrollView {
    fn render(mut self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let handle = match self.handle {
            Some(handle) => handle,
            None => window.with_global_id((self.id.clone(), "v-scroll").into(), |global_id, window| {
                window.with_element_state(global_id, |state: Option<ScrollHandle>, _| {
                    let handle = state.unwrap_or_default();
                    (handle.clone(), handle)
                })
            }),
        };
        let mut bar = VerticalScrollbar::for_handle(self.id, handle.clone()).inset(self.inset);
        if let Some(color) = self.color {
            bar = bar.color(color);
        }
        if self.compact {
            bar = bar.compact();
        }

        // Propagate definite/max height bounds to the inner viewport so `max_h` containers
        // shrink-wrap short content while constraining and scrolling overflowing content.
        let mut viewport = self
            .viewport
            .flex_auto()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&handle);
        viewport.style().max_size.height = self.outer.style().max_size.height;

        self.outer.child(viewport).child(bar)
    }
}

fn resolve_drag_state(
    id: Option<&ElementId>,
    fallback: &DragState,
    window: &mut Window,
) -> DragState {
    let Some(id) = id else {
        return fallback.clone();
    };
    window.with_global_id((id.clone(), "scrollbar-drag").into(), |global_id, window| {
        window.with_element_state(global_id, |state: Option<DragState>, _| {
            let state = state.unwrap_or_default();
            (state.clone(), state)
        })
    })
}

fn render_scrollbar(
    axis: Axis,
    target: ScrollTarget,
    drag: DragState,
    inset: Pixels,
    color: Option<Hsla>,
    compact: bool,
) -> impl IntoElement {
    let margin = px(if compact { COMPACT_END_MARGIN } else { END_MARGIN });
    let min_thumb = px(if compact {
        COMPACT_MIN_THUMB_LENGTH
    } else {
        MIN_THUMB_LENGTH
    });

    let canvas_el = canvas(
        {
            let target = target.clone();
            move |track, window, _| {
                let overflow = match axis {
                    Axis::Vertical => target.max_offset().y,
                    Axis::Horizontal => target.max_offset().x,
                };
                let viewport_len = match axis {
                    Axis::Vertical => target.viewport_size().height,
                    Axis::Horizontal => target.viewport_size().width,
                };
                let track_len = match axis {
                    Axis::Vertical => track.size.height,
                    Axis::Horizontal => track.size.width,
                };
                if overflow <= px(0.) || viewport_len <= px(0.) || track_len <= min_thumb {
                    return None;
                }

                let thumb_len =
                    (track_len * (viewport_len / (viewport_len + overflow))).max(min_thumb);
                let travel = track_len - thumb_len;
                if travel <= px(0.) {
                    return None;
                }

                let offset_along = match axis {
                    Axis::Vertical => -target.offset().y,
                    Axis::Horizontal => -target.offset().x,
                };
                let fraction = (offset_along / overflow).clamp(0., 1.);

                let thumb = match axis {
                    Axis::Vertical => Bounds::new(
                        point(track.left(), track.top() + travel * fraction),
                        size(track.size.width, thumb_len),
                    ),
                    Axis::Horizontal => Bounds::new(
                        point(track.left() + travel * fraction, track.top()),
                        size(thumb_len, track.size.height),
                    ),
                };
                let hitbox = window.insert_hitbox(track, HitboxBehavior::BlockMouseExceptScroll);
                Some((hitbox, thumb, travel, overflow))
            }
        },
        move |track, layout, window, cx| {
            let Some((hitbox, thumb, travel, overflow)) = layout else {
                if drag.grab_offset.take().is_some() {
                    target.drag_ended();
                }
                return;
            };

            let hovered = hitbox.is_hovered(window);
            let dragging = drag.grab_offset.get().is_some();
            let active = hovered || dragging;
            if hovered {
                window.set_cursor_style(CursorStyle::Arrow, &hitbox);
            }

            let base = color.unwrap_or_else(|| cx.theme().colors().text);
            let thumb_thickness = px(if active {
                THUMB_ACTIVE_THICKNESS
            } else {
                THUMB_REST_THICKNESS
            });
            let thumb_opacity = if dragging {
                THUMB_DRAG_OPACITY
            } else if hovered {
                THUMB_HOVER_OPACITY
            } else {
                THUMB_REST_OPACITY
            };

            if active {
                let lane_thickness = px(THUMB_ACTIVE_THICKNESS + 2.);
                let lane_edge = px((EDGE_INSET - 1.).max(0.));
                let lane = match axis {
                    Axis::Vertical => Bounds::new(
                        point(track.right() - lane_edge - lane_thickness, track.top()),
                        size(lane_thickness, track.size.height),
                    ),
                    Axis::Horizontal => Bounds::new(
                        point(track.left(), track.bottom() - lane_edge - lane_thickness),
                        size(track.size.width, lane_thickness),
                    ),
                };
                window.paint_quad(
                    fill(lane, base.opacity(TRACK_ACTIVE_OPACITY))
                        .corner_radii(lane_thickness / 2.),
                );
            }

            let edge = px(EDGE_INSET);
            let grip = match axis {
                Axis::Vertical => Bounds::new(
                    point(track.right() - edge - thumb_thickness, thumb.top()),
                    size(thumb_thickness, thumb.size.height),
                ),
                Axis::Horizontal => Bounds::new(
                    point(thumb.left(), track.bottom() - edge - thumb_thickness),
                    size(thumb.size.width, thumb_thickness),
                ),
            };
            window.paint_quad(
                fill(grip, base.opacity(thumb_opacity)).corner_radii(thumb_thickness / 2.),
            );

            let scroll_to = move |target: &ScrollTarget,
                                  pointer: Pixels,
                                  grab: Pixels,
                                  window: &mut Window| {
                let track_start = match axis {
                    Axis::Vertical => track.top(),
                    Axis::Horizontal => track.left(),
                };
                let fraction = ((pointer - track_start - grab) / travel).clamp(0., 1.);
                target.set_axis_offset(axis, fraction, overflow);
                window.refresh();
            };

            window.on_mouse_event({
                let hitbox = hitbox.clone();
                let target = target.clone();
                let drag = drag.clone();
                move |event: &MouseDownEvent, phase, window, cx| {
                    if phase.bubble()
                        && event.button == MouseButton::Left
                        && hitbox.is_hovered(window)
                    {
                        let pointer = match axis {
                            Axis::Vertical => event.position.y,
                            Axis::Horizontal => event.position.x,
                        };
                        let grab = if thumb.contains(&event.position) {
                            let thumb_start = match axis {
                                Axis::Vertical => thumb.top(),
                                Axis::Horizontal => thumb.left(),
                            };
                            pointer - thumb_start
                        } else {
                            let thumb_len = match axis {
                                Axis::Vertical => thumb.size.height,
                                Axis::Horizontal => thumb.size.width,
                            };
                            thumb_len / 2.
                        };
                        target.drag_started();
                        drag.grab_offset.set(Some(grab));
                        scroll_to(&target, pointer, grab, window);
                        cx.stop_propagation();
                    }
                }
            });

            window.on_mouse_event({
                let hitbox = hitbox.clone();
                let target = target.clone();
                let drag = drag.clone();
                move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.capture() {
                        return;
                    }
                    if event.pressed_button == Some(MouseButton::Left) {
                        if let Some(grab) = drag.grab_offset.get() {
                            let pointer = match axis {
                                Axis::Vertical => event.position.y,
                                Axis::Horizontal => event.position.x,
                            };
                            scroll_to(&target, pointer, grab, window);
                            cx.stop_propagation();
                            return;
                        }
                    } else if drag.grab_offset.take().is_some() {
                        target.drag_ended();
                        window.refresh();
                    }
                    if hovered != hitbox.is_hovered(window) {
                        window.refresh();
                    }
                }
            });

            window.on_mouse_event({
                let target = target.clone();
                let drag = drag.clone();
                move |event: &MouseUpEvent, phase, window, cx| {
                    if phase.capture()
                        && event.button == MouseButton::Left
                        && drag.grab_offset.take().is_some()
                    {
                        target.drag_ended();
                        window.refresh();
                        cx.stop_propagation();
                    }
                }
            });
        },
    )
    .absolute();

    match axis {
        Axis::Vertical => canvas_el
            .top(margin)
            .bottom(margin)
            .right(inset)
            .w(px(TRACK_THICKNESS)),
        Axis::Horizontal => canvas_el
            .left(margin)
            .right(margin)
            .bottom(inset)
            .h(px(TRACK_THICKNESS)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, InteractiveElement, ListAlignment, Modifiers, Render, TestAppContext, Window, list,
    };

    #[derive(Default)]
    struct FlexTestView {
        clicks: Rc<Cell<usize>>,
    }

    impl Render for FlexTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div().size_full().flex().flex_col().child(
                v_scroll(
                    "flex-test-scroll",
                    div()
                        .id("flex-test-viewport")
                        .debug_selector(|| "flex-viewport".to_owned())
                        .on_click(move |_, _, _| {
                            clicks.set(clicks.get() + 1);
                        })
                        .child(
                            div()
                                .debug_selector(|| "flex-content".to_owned())
                                .w_full()
                                .h(px(900.)),
                        ),
                )
                .flex_1()
                .min_h_0(),
            )
        }
    }

    #[gpui::test]
    fn v_scroll_flex_layout_scrolls_drags_and_hides_when_fitted(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| FlexTestView {
            clicks: clicks.clone(),
        });
        cx.simulate_resize(size(px(300.), px(300.)));
        cx.run_until_parked();

        let viewport = cx.debug_bounds("flex-viewport").unwrap();
        let initial_content = cx.debug_bounds("flex-content").unwrap();
        assert!(initial_content.size.height > viewport.size.height);

        let bar_bottom = point(viewport.right() - px(6.), viewport.bottom() - px(8.));
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(bar_bottom, None, modifiers);
        cx.simulate_click(bar_bottom, modifiers);
        let scrolled_content = cx.debug_bounds("flex-content").unwrap();
        assert!((scrolled_content.bottom() - viewport.bottom()).abs() < px(1.));
        assert_eq!(clicks.get(), 0, "scrollbar click must not leak to viewport");

        // Drag thumb back to the top across multiple mouse moves to confirm persisted drag state.
        let bar_top = point(bar_bottom.x, viewport.top() - px(10.));
        let bar_mid = point(bar_bottom.x, viewport.center().y);
        cx.simulate_mouse_down(bar_bottom, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(bar_mid, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(bar_top, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(bar_top, MouseButton::Left, modifiers);
        assert_eq!(
            cx.debug_bounds("flex-content").unwrap().top(),
            initial_content.top()
        );

        // Resize taller than content: scrollbar disappears and clicks pass through to viewport.
        cx.simulate_resize(size(px(300.), px(1100.)));
        let fitted_viewport = cx.debug_bounds("flex-viewport").unwrap();
        let right_strip = point(fitted_viewport.right() - px(6.), fitted_viewport.bottom() - px(8.));
        cx.simulate_mouse_move(right_strip, None, modifiers);
        cx.simulate_click(right_strip, modifiers);
        assert_eq!(clicks.get(), 1);
    }

    struct MaxHeightTestView {
        content_height: Pixels,
    }

    impl Render for MaxHeightTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().items_start().child(
                div()
                    .debug_selector(|| "max-h-outer".to_owned())
                    .child(
                        v_scroll(
                            "max-h-scroll",
                            div()
                                .id("max-h-viewport")
                                .debug_selector(|| "max-h-viewport".to_owned())
                                .child(
                                    div()
                                        .debug_selector(|| "max-h-content".to_owned())
                                        .w(px(200.))
                                        .h(self.content_height),
                                ),
                        )
                        .w(px(200.))
                        .max_h(px(200.)),
                    ),
            )
        }
    }

    #[gpui::test]
    fn v_scroll_max_h_shrinks_when_short_and_scrolls_when_tall(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let (view, cx) = cx.add_window_view(|_, _| MaxHeightTestView {
            content_height: px(80.),
        });
        cx.simulate_resize(size(px(400.), px(600.)));
        cx.run_until_parked();

        let outer = cx.debug_bounds("max-h-outer").unwrap();
        let viewport = cx.debug_bounds("max-h-viewport").unwrap();
        assert_eq!(outer.size.height, px(80.));
        assert_eq!(viewport.size.height, px(80.));

        view.update(cx, |this, cx| {
            this.content_height = px(500.);
            cx.notify();
        });
        cx.run_until_parked();

        let outer = cx.debug_bounds("max-h-outer").unwrap();
        let viewport = cx.debug_bounds("max-h-viewport").unwrap();
        assert_eq!(outer.size.height, px(200.));
        assert_eq!(viewport.size.height, px(200.));

        let bar_bottom = point(viewport.right() - px(6.), viewport.bottom() - px(8.));
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(bar_bottom, None, modifiers);
        cx.simulate_click(bar_bottom, modifiers);
        let content = cx.debug_bounds("max-h-content").unwrap();
        assert!((content.bottom() - viewport.bottom()).abs() < px(1.));
    }

    struct InsetTestView {
        edge_clicks: Rc<Cell<usize>>,
    }

    impl Render for InsetTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let edge_clicks = self.edge_clicks.clone();
            div()
                .relative()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    v_scroll(
                        "inset-scroll",
                        div()
                            .id("inset-viewport")
                            .debug_selector(|| "inset-viewport".to_owned())
                            .child(
                                div()
                                    .debug_selector(|| "inset-content".to_owned())
                                    .w_full()
                                    .h(px(800.)),
                            ),
                    )
                    .inset(px(6.))
                    .flex_1()
                    .min_h_0(),
                )
                .child(
                    div()
                        .id("resize-strip")
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(6.))
                        .on_click(move |_, _, _| {
                            edge_clicks.set(edge_clicks.get() + 1);
                        }),
                )
        }
    }

    #[gpui::test]
    fn v_scroll_inset_leaves_edge_unobstructed(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let edge_clicks = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|_, _| InsetTestView {
            edge_clicks: edge_clicks.clone(),
        });
        cx.simulate_resize(size(px(300.), px(300.)));
        cx.run_until_parked();

        let viewport = cx.debug_bounds("inset-viewport").unwrap();
        let initial = cx.debug_bounds("inset-content").unwrap();
        let modifiers = Modifiers::default();

        // Clicking inside the 6 px right edge hits the resize strip, not the scrollbar.
        let edge_pt = point(viewport.right() - px(3.), viewport.bottom() - px(10.));
        cx.simulate_mouse_move(edge_pt, None, modifiers);
        cx.simulate_click(edge_pt, modifiers);
        assert_eq!(edge_clicks.get(), 1);
        assert_eq!(cx.debug_bounds("inset-content").unwrap().top(), initial.top());

        // Clicking left of the 6 px inset hits the scrollbar and scrolls.
        let bar_pt = point(viewport.right() - px(10.), viewport.bottom() - px(10.));
        cx.simulate_mouse_move(bar_pt, None, modifiers);
        cx.simulate_click(bar_pt, modifiers);
        assert_eq!(edge_clicks.get(), 1);
        assert!(cx.debug_bounds("inset-content").unwrap().top() < initial.top());
    }

    struct ListTestView {
        state: ListState,
    }

    impl Render for ListTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .debug_selector(|| "list-viewport".to_owned())
                .child(
                    list(self.state.clone(), |ix, _, _| {
                        div()
                            .h(px(40.))
                            .w_full()
                            .debug_selector(move || format!("list-row-{ix}"))
                            .into_any_element()
                    })
                    .size_full()
                    .pb_4(),
                )
                .child(VerticalScrollbar::for_list(
                    "list-scrollbar",
                    self.state.clone(),
                ))
        }
    }

    #[gpui::test]
    fn vertical_scrollbar_for_list_scrolls_to_bottom_and_top(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let state = ListState::new(40, ListAlignment::Top, px(400.)).measure_all();
        let (_, cx) = cx.add_window_view(|_, _| ListTestView { state });
        cx.simulate_resize(size(px(300.), px(240.)));
        cx.run_until_parked();

        let viewport = cx.debug_bounds("list-viewport").unwrap();
        assert!(cx.debug_bounds("list-row-0").is_some());
        assert!(cx.debug_bounds("list-row-39").is_none());

        let bar_bottom = point(viewport.right() - px(6.), viewport.bottom() - px(8.));
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(bar_bottom, None, modifiers);
        cx.simulate_click(bar_bottom, modifiers);
        let last = cx
            .debug_bounds("list-row-39")
            .expect("last list row should be revealed when scrolled to bottom");
        assert!((last.bottom() - (viewport.bottom() - px(16.))).abs() < px(2.));

        let bar_top = point(bar_bottom.x, viewport.top() - px(10.));
        cx.simulate_mouse_down(bar_bottom, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(bar_top, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(bar_top, MouseButton::Left, modifiers);
        let first = cx
            .debug_bounds("list-row-0")
            .expect("first list row should return to top");
        assert!((first.top() - viewport.top()).abs() < px(1.));
    }

    struct HorizontalTestView {
        scroll: ScrollHandle,
    }

    impl Render for HorizontalTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("h-viewport")
                        .debug_selector(|| "h-viewport".to_owned())
                        .size_full()
                        .overflow_x_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .debug_selector(|| "h-content".to_owned())
                                .w(px(900.))
                                .h_full(),
                        ),
                )
                .child(HorizontalScrollbar::for_handle("h-bar", self.scroll.clone()))
        }
    }

    #[gpui::test]
    fn horizontal_scrollbar_scrolls_right_and_left(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let (_, cx) = cx.add_window_view(|_, _| HorizontalTestView {
            scroll: ScrollHandle::new(),
        });
        cx.simulate_resize(size(px(300.), px(200.)));
        cx.run_until_parked();

        let viewport = cx.debug_bounds("h-viewport").unwrap();
        let initial = cx.debug_bounds("h-content").unwrap();
        let bar_right = point(viewport.right() - px(8.), viewport.bottom() - px(6.));
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(bar_right, None, modifiers);
        cx.simulate_click(bar_right, modifiers);
        let scrolled = cx.debug_bounds("h-content").unwrap();
        assert!((scrolled.right() - viewport.right()).abs() < px(1.));

        let bar_left = point(viewport.left() - px(10.), bar_right.y);
        cx.simulate_mouse_down(bar_right, MouseButton::Left, modifiers);
        cx.simulate_mouse_move(bar_left, MouseButton::Left, modifiers);
        cx.simulate_mouse_up(bar_left, MouseButton::Left, modifiers);
        assert_eq!(cx.debug_bounds("h-content").unwrap().left(), initial.left());
    }
}
