use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Bounds, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, RenderOnce, ScrollHandle, Styled, Window, canvas, fill, point, px, size,
};
use theme::ActiveTheme as _;

const TRACK_WIDTH: f32 = 18.;
const THUMB_WIDTH: f32 = 6.;
const MIN_THUMB_HEIGHT: f32 = 24.;

/// An overlay at the right edge of a scroll viewport. Add it after the viewport as a child of
/// the same relative wrapper, and use its handle to track the viewport's scroll position.
#[derive(Clone, Default, IntoElement)]
pub struct VerticalScrollbar {
    scroll: ScrollHandle,
    grab_offset: Rc<Cell<Option<Pixels>>>,
}

impl VerticalScrollbar {
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }
}

impl RenderOnce for VerticalScrollbar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let scroll = self.scroll.clone();
        canvas(
            move |track, window, _| {
                // The viewport is prepainted first, so these reflect the current layout.
                let overflow = scroll.max_offset().y;
                let viewport_height = scroll.bounds().size.height;
                if overflow <= px(0.) || track.size.height <= px(MIN_THUMB_HEIGHT) {
                    return None;
                }
                let thumb_height = (track.size.height
                    * (viewport_height / (viewport_height + overflow)))
                    .max(px(MIN_THUMB_HEIGHT));
                let travel = track.size.height - thumb_height;
                let thumb_top = track.top() + travel * (-scroll.offset().y / overflow);
                let thumb = Bounds::new(
                    point(track.left(), thumb_top),
                    size(track.size.width, thumb_height),
                );
                let hitbox = window.insert_hitbox(track, HitboxBehavior::BlockMouseExceptScroll);
                Some((hitbox, thumb, travel, overflow))
            },
            move |track, layout, window, cx| {
                let Some((hitbox, thumb, travel, overflow)) = layout else {
                    self.grab_offset.set(None);
                    return;
                };
                let hovered = hitbox.is_hovered(window);
                let colors = cx.theme().colors();
                let lane = Bounds::new(
                    point(track.center().x - px(THUMB_WIDTH / 2.), track.top()),
                    size(px(THUMB_WIDTH), track.size.height),
                );
                let grip = Bounds::new(
                    point(lane.left(), thumb.top()),
                    size(lane.size.width, thumb.size.height),
                );
                let color = if self.grab_offset.get().is_some() || hovered {
                    colors.text
                } else {
                    colors.text_muted
                };
                window.paint_quad(
                    fill(lane, colors.element_hover).corner_radii(px(THUMB_WIDTH / 2.)),
                );
                window.paint_quad(fill(grip, color).corner_radii(px(THUMB_WIDTH / 2.)));

                let scroll_to = move |scroll: &ScrollHandle,
                                      pointer: Pixels,
                                      grab: Pixels,
                                      window: &mut Window| {
                    let fraction = ((pointer - track.top() - grab) / travel).clamp(0., 1.);
                    scroll.set_offset(point(scroll.offset().x, -overflow * fraction));
                    window.refresh();
                };
                window.on_mouse_event({
                    let hitbox = hitbox.clone();
                    let bar = self.clone();
                    move |event: &MouseDownEvent, phase, window, cx| {
                        if phase.bubble()
                            && event.button == MouseButton::Left
                            && hitbox.is_hovered(window)
                        {
                            let grab = if thumb.contains(&event.position) {
                                event.position.y - thumb.top()
                            } else {
                                thumb.size.height / 2.
                            };
                            bar.grab_offset.set(Some(grab));
                            scroll_to(&bar.scroll, event.position.y, grab, window);
                            cx.stop_propagation();
                        }
                    }
                });
                window.on_mouse_event({
                    let bar = self.clone();
                    move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.capture() {
                            return;
                        }
                        if event.pressed_button == Some(MouseButton::Left) {
                            if let Some(grab) = bar.grab_offset.get() {
                                scroll_to(&bar.scroll, event.position.y, grab, window);
                                cx.stop_propagation();
                                return;
                            }
                        } else if bar.grab_offset.take().is_some() {
                            window.refresh();
                        }
                        if hovered != hitbox.is_hovered(window) {
                            window.refresh();
                        }
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase.capture()
                        && event.button == MouseButton::Left
                        && self.grab_offset.take().is_some()
                    {
                        window.refresh();
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .top(px(4.))
        .bottom(px(4.))
        .right_0()
        .w(px(TRACK_WIDTH))
    }
}
