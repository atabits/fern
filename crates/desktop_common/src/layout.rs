use std::collections::HashMap;

use gpui::{
    App, Bounds, CursorStyle, Decorations, Global, HitboxBehavior, MouseButton, Point, ResizeEdge,
    Size, Stateful, Tiling, Window, WindowButton, WindowButtonLayout, WindowId, canvas, point,
    transparent_black,
};
use ui::prelude::*;

/// The room around the window for its shadow, which is also where it is grabbed to resize.
pub const TITLE_BAR_SHADOW: Pixels = px(10.);
/// The radius of Linux client-side decorated windows' corners.
pub const LINUX_WINDOW_ROUNDING: Pixels = px(10.);
/// Total extra width/height added to fixed-size windows on Linux/FreeBSD to fit the CSD shadow
/// and 1 px frame border on all sides without clipping inner content.
const LINUX_CSD_FRAME_PAD: f32 = 22.;

/// Expands a fixed-size window's requested bounds to leave room for the Linux CSD frame.
pub fn with_linux_csd_pad(inner: Size<Pixels>) -> Size<Pixels> {
    if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        gpui::size(inner.width + px(LINUX_CSD_FRAME_PAD), inner.height + px(LINUX_CSD_FRAME_PAD))
    } else {
        inner
    }
}

#[cfg(target_os = "macos")]
pub const SIDEBAR_WIDTH: f32 = 72.;
#[cfg(not(target_os = "macos"))]
pub const SIDEBAR_WIDTH: f32 = 50.;

fn button_layout(cx: &App) -> WindowButtonLayout {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cx.button_layout().unwrap_or_else(WindowButtonLayout::linux_default)
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = cx;
        WindowButtonLayout {
            left: [None; 3],
            right: [
                Some(WindowButton::Minimize),
                Some(WindowButton::Maximize),
                Some(WindowButton::Close),
            ],
        }
    }
}

fn has_visible_caption_buttons(window: &Window, cx: &App) -> bool {
    let controls = window.window_controls();
    let layout = button_layout(cx);
    layout
        .left
        .into_iter()
        .chain(layout.right)
        .flatten()
        .any(|button| match button {
            WindowButton::Minimize => controls.minimize && window.is_minimizable(),
            WindowButton::Maximize => controls.maximize && window.is_resizable(),
            WindowButton::Close => true,
        })
}

/// Whether the window header on Linux/FreeBSD should collapse to its compact height
/// (under SSD window managers, fullscreen, or fully tiled compositors with no CSD buttons).
pub fn is_linux_compact_header(window: &Window, cx: &App) -> bool {
    if !cfg!(any(target_os = "linux", target_os = "freebsd")) {
        return false;
    }
    match window.window_decorations() {
        Decorations::Server => true,
        Decorations::Client { tiling } => {
            window.is_fullscreen() || (tiling.is_tiled() && !has_visible_caption_buttons(window, cx))
        }
    }
}

pub fn title_bar_height_for(compact_linux: bool) -> Pixels {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if compact_linux {
        return px(8.);
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let _ = compact_linux;
    px(32.)
}

/// The overlaid caption strip's height: Windows' standard 32 px caption button height,
/// collapsing to 8 px under Linux server-side decorations or fullscreen.
pub fn title_bar_height(window: &Window, cx: &App) -> Pixels {
    title_bar_height_for(is_linux_compact_header(window, cx))
}

pub fn header_inset_for(compact_linux: bool) -> Pixels {
    (title_bar_height_for(compact_linux) - px(8.)).max(px(0.))
}

/// The top margin workspace headers leave so their controls clear the overlaid window buttons.
pub fn header_inset(window: &Window, cx: &App) -> Pixels {
    header_inset_for(is_linux_compact_header(window, cx))
}

pub fn frame_vertical_inset(window: &Window) -> Pixels {
    if cfg!(any(target_os = "linux", target_os = "freebsd"))
        && let Decorations::Client { tiling } = window.window_decorations()
    {
        return (TITLE_BAR_SHADOW + px(1.))
            * (u8::from(!tiling.top) + u8::from(!tiling.bottom)) as f32;
    }
    px(0.)
}

/// Wraps a window's content in the frame Fern draws with client-side decorations on Linux:
/// rounded corners, a border, a shadow, and edges that resize the window. With server-side
/// decorations, and on macOS and Windows, the content is returned without CSD chrome.
pub fn linux_window_frame(
    content: impl IntoElement,
    rounding: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let decorations = if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        window.window_decorations()
    } else {
        Decorations::Server
    };
    let resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    window.set_client_inset(match decorations {
        Decorations::Client { .. } => TITLE_BAR_SHADOW,
        Decorations::Server => px(0.),
    });

    /// The edge under the mouse per window, to repaint the cursor when it changes.
    #[derive(Default)]
    struct HoveredEdges(HashMap<WindowId, ResizeEdge>);
    impl Global for HoveredEdges {}

    let window_id = window.window_handle().window_id();
    let client = matches!(decorations, Decorations::Client { .. });
    div()
        .id("window-frame")
        .size_full()
        .bg(transparent_black())
        .when(client, |frame| {
            frame
                .when(!tiling.top, |frame| frame.pt(TITLE_BAR_SHADOW))
                .when(!tiling.bottom, |frame| frame.pb(TITLE_BAR_SHADOW))
                .when(!tiling.left, |frame| frame.pl(TITLE_BAR_SHADOW))
                .when(!tiling.right, |frame| frame.pr(TITLE_BAR_SHADOW))
                .when(resizable, |frame| {
                    frame
                        .on_mouse_move(move |event, window, cx| {
                            let edge = resize_edge(
                                event.position,
                                window.window_bounds().get_bounds().size,
                                tiling,
                            );
                            let previous = cx
                                .try_global::<HoveredEdges>()
                                .and_then(|edges| edges.0.get(&window_id).copied());
                            if edge != previous {
                                window.refresh();
                            }
                        })
                        .on_mouse_down(MouseButton::Left, move |event, window, _| {
                            if let Some(edge) = resize_edge(
                                event.position,
                                window.window_bounds().get_bounds().size,
                                tiling,
                            ) {
                                window.start_window_resize(edge);
                            }
                        })
                })
        })
        .child(
            div()
                .size_full()
                .cursor(CursorStyle::Arrow)
                .when(client, |content| {
                    content
                        .overflow_hidden()
                        .border_color(cx.theme().colors().border)
                        .when(!(tiling.top || tiling.left), |el| el.rounded_tl(rounding))
                        .when(!(tiling.top || tiling.right), |el| el.rounded_tr(rounding))
                        .when(!(tiling.bottom || tiling.left), |el| el.rounded_bl(rounding))
                        .when(!(tiling.bottom || tiling.right), |el| el.rounded_br(rounding))
                        .when(!tiling.top, |el| el.border_t_1())
                        .when(!tiling.bottom, |el| el.border_b_1())
                        .when(!tiling.left, |el| el.border_l_1())
                        .when(!tiling.right, |el| el.border_r_1())
                        .when(!tiling.is_tiled(), |el| {
                            el.shadow(vec![
                                gpui::BoxShadow::new(px(0.), px(0.), gpui::black().opacity(0.4))
                                    .blur_radius(TITLE_BAR_SHADOW / 2.),
                            ])
                        })
                })
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
        .when(client && resizable, |frame| {
            frame.child(
                canvas(
                    |_, window, _| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.), px(0.)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_, hitbox, window, cx| {
                        let edge = resize_edge(
                            window.mouse_position(),
                            window.window_bounds().get_bounds().size,
                            tiling,
                        );
                        if !cx.has_global::<HoveredEdges>() {
                            cx.set_global(HoveredEdges::default());
                        }
                        let Some(edge) = edge else {
                            cx.global_mut::<HoveredEdges>().0.remove(&window_id);
                            return;
                        };
                        cx.global_mut::<HoveredEdges>().0.insert(window_id, edge);
                        let cursor = match edge {
                            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                            ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                CursorStyle::ResizeUpLeftDownRight
                            }
                            ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                CursorStyle::ResizeUpRightDownLeft
                            }
                        };
                        window.set_cursor_style(cursor, &hitbox);
                    },
                )
                .size_full()
                .absolute(),
            )
        })
}

/// The window edge or corner at `pos`, if it is in the shadow around the window.
fn resize_edge(pos: Point<Pixels>, window: Size<Pixels>, tiling: Tiling) -> Option<ResizeEdge> {
    if Bounds::new(Point::default(), window)
        .inset(TITLE_BAR_SHADOW * 1.5)
        .contains(&pos)
    {
        return None;
    }
    let corner = TITLE_BAR_SHADOW * 1.5;
    let left = pos.x < corner;
    let right = pos.x > window.width - corner;
    let top = pos.y < corner;
    let bottom = pos.y > window.height - corner;
    match () {
        _ if !tiling.top && top && left => Some(ResizeEdge::TopLeft),
        _ if !tiling.top && top && right => Some(ResizeEdge::TopRight),
        _ if !tiling.bottom && bottom && left => Some(ResizeEdge::BottomLeft),
        _ if !tiling.bottom && bottom && right => Some(ResizeEdge::BottomRight),
        _ if !tiling.top && pos.y < TITLE_BAR_SHADOW => Some(ResizeEdge::Top),
        _ if !tiling.bottom && pos.y > window.height - TITLE_BAR_SHADOW => Some(ResizeEdge::Bottom),
        _ if !tiling.left && pos.x < TITLE_BAR_SHADOW => Some(ResizeEdge::Left),
        _ if !tiling.right && pos.x > window.width - TITLE_BAR_SHADOW => Some(ResizeEdge::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn resize_edges() {
        let window = size(px(800.), px(600.));
        let free = Tiling::default();
        assert_eq!(resize_edge(point(px(400.), px(300.)), window, free), None);
        assert_eq!(
            resize_edge(point(px(2.), px(2.)), window, free),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(400.), px(598.)), window, free),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(
            resize_edge(point(px(795.), px(300.)), window, free),
            Some(ResizeEdge::Right)
        );
        let tiled = Tiling {
            right: true,
            ..Tiling::default()
        };
        assert_eq!(resize_edge(point(px(795.), px(300.)), window, tiled), None);
    }

    #[test]
    fn compact_header_height() {
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            assert_eq!(title_bar_height_for(true), px(8.));
            assert_eq!(header_inset_for(true), px(0.));
        } else {
            assert_eq!(title_bar_height_for(true), px(32.));
            assert_eq!(header_inset_for(true), px(24.));
        }
        assert_eq!(title_bar_height_for(false), px(32.));
        assert_eq!(header_inset_for(false), px(24.));
    }

    #[test]
    fn linux_csd_padding_matches_shadow_and_border() {
        assert_eq!(LINUX_CSD_FRAME_PAD, f32::from(TITLE_BAR_SHADOW) * 2. + 2.);
        let inner = size(px(300.), px(200.));
        let padded = with_linux_csd_pad(inner);
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            assert_eq!(padded, size(px(322.), px(222.)));
        } else {
            assert_eq!(padded, inner);
        }
    }
}
