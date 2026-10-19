//! Fern's own title bar and window frame, drawn in the app's theme on every platform instead of
//! the system's: the window is opened without the system title bar (`window_options`), and this
//! module draws the bar, the window buttons and, on Linux, the frame's border, shadow and resize
//! edges. After Zed's `platform_title_bar` and `client_side_decorations`.
//!
//! - macOS keeps its traffic lights, which sit over the bar's left end.
//! - Windows draws nothing itself: the bar's areas are tagged with [`WindowControlArea`], so the
//!   system handles dragging, double clicks, snap layouts and the buttons' clicks.
//! - Linux asks the compositor for client-side decorations. When it draws its own (server-side),
//!   the bar stays but without window buttons.

use gpui::{
    Bounds, CursorStyle, Decorations, Global, HitboxBehavior, MouseButton, Point, ResizeEdge, Size,
    TitlebarOptions, Tiling, WindowButton, WindowButtonLayout, WindowControlArea, WindowDecorations,
    canvas, point, rgb, transparent_black,
};
use ui::prelude::*;

/// The radius of the window's corners with client-side decorations.
const ROUNDING: Pixels = px(10.);
/// The room around the window for its shadow, which is also where it is grabbed to resize.
const SHADOW: Pixels = px(10.);
/// macOS traffic lights: their position, and the room the bar leaves for them.
const TRAFFIC_LIGHTS: Point<Pixels> = point(px(12.), px(11.));
const TRAFFIC_LIGHT_PADDING: Pixels = px(80.);

/// The bar's height: Windows' caption height there, a bit more elsewhere.
pub fn height() -> Pixels {
    if cfg!(target_os = "windows") { px(32.) } else { px(36.) }
}

/// The window options that hide the system title bar.
pub fn window_options() -> (TitlebarOptions, WindowDecorations) {
    let titlebar = TitlebarOptions {
        title: Some("Fern".into()),
        appears_transparent: true,
        traffic_light_position: Some(TRAFFIC_LIGHTS),
    };
    (titlebar, WindowDecorations::Client)
}

pub struct TitleBar {
    /// Linux and macOS: a press on the bar turns into a window move once the mouse moves.
    should_move: bool,
}

impl TitleBar {
    pub fn new() -> Self {
        Self { should_move: false }
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        let decorations = window.window_decorations();
        let client_side = matches!(decorations, Decorations::Client { .. });
        let fullscreen = window.is_fullscreen();
        let linux = cfg!(any(target_os = "linux", target_os = "freebsd"));
        let mac = cfg!(target_os = "macos");
        let windows = cfg!(target_os = "windows");
        let buttons = if windows {
            Some(WindowButtonLayout { left: [None; 3], right: [Some(WindowButton::Minimize), Some(WindowButton::Maximize), Some(WindowButton::Close)] })
        } else if linux && client_side {
            Some(button_layout(cx))
        } else {
            None
        };
        let side = |buttons: [Option<WindowButton>; 3], id: &'static str| {
            buttons.iter().any(Option::is_some).then(|| WindowButtons { id, buttons }.into_any_element())
        };
        let left_buttons = buttons.and_then(|b| side(b.left, "left-window-buttons")).filter(|_| !fullscreen);
        let right_buttons = buttons.and_then(|b| side(b.right, "right-window-buttons")).filter(|_| !fullscreen);
        let background = if linux && !window.is_window_active() { colors.panel_background } else { colors.sidebar_background };

        h_flex()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .flex_none()
            .w_full()
            .h(height())
            .bg(background)
            .border_b_1()
            .border_color(colors.border)
            .map(|bar| match decorations {
                Decorations::Client { tiling } if !windows => bar
                    .when(!(tiling.top || tiling.left), |bar| bar.rounded_tl(ROUNDING))
                    .when(!(tiling.top || tiling.right), |bar| bar.rounded_tr(ROUNDING)),
                _ => bar,
            })
            .when(!windows, |bar| {
                bar.on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = true))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
                    .on_mouse_down_out(cx.listener(|this, _, _, _| this.should_move = false))
                    .on_mouse_move(cx.listener(|this, _, window, _| {
                        if this.should_move {
                            this.should_move = false;
                            window.start_window_move();
                        }
                    }))
                    .on_click(move |event, window, _| {
                        if event.click_count() != 2 {
                            return;
                        }
                        if mac {
                            window.titlebar_double_click();
                        } else if window.window_controls().maximize && window.is_resizable() {
                            window.zoom_window();
                        }
                    })
            })
            .when(linux && client_side && window.window_controls().window_menu, |bar| {
                bar.on_mouse_down(MouseButton::Right, |event, window, _| window.show_window_menu(event.position))
            })
            .children(left_buttons)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap_2()
                    .map(|title| if mac && !fullscreen { title.pl(TRAFFIC_LIGHT_PADDING) } else { title.pl_3() })
                    .child(gpui::img("icon/icon-64.png").size(px(16.)).flex_none())
                    .child(Label::new("Fern").size(LabelSize::Small).color(Color::Muted)),
            )
            .children(right_buttons)
    }
}

/// Linux: the desktop's order of the window buttons (GNOME's `button-layout`), or Minimize,
/// Maximize, Close on the right.
fn button_layout(cx: &App) -> WindowButtonLayout {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cx.button_layout().unwrap_or_else(WindowButtonLayout::linux_default)
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = cx;
        WindowButtonLayout { left: [None; 3], right: [Some(WindowButton::Minimize), Some(WindowButton::Maximize), Some(WindowButton::Close)] }
    }
}

/// The minimize, maximize and close buttons on one side of the bar.
#[derive(IntoElement)]
struct WindowButtons {
    id: &'static str,
    buttons: [Option<WindowButton>; 3],
}

impl RenderOnce for WindowButtons {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let controls = window.window_controls();
        let maximized = window.is_maximized();
        h_flex()
            .id(self.id)
            .h_full()
            .flex_none()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(self.buttons.into_iter().flatten().filter(|button| match button {
                WindowButton::Minimize => controls.minimize && window.is_minimizable(),
                WindowButton::Maximize => controls.maximize && window.is_resizable(),
                WindowButton::Close => true,
            }).map(|button| window_button(button, maximized, cx)))
    }
}

fn window_button(button: WindowButton, maximized: bool, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors();
    let (icon, area) = match button {
        WindowButton::Minimize => (IconName::WindowMinimize, WindowControlArea::Min),
        WindowButton::Maximize if maximized => (IconName::WindowRestore, WindowControlArea::Max),
        WindowButton::Maximize => (IconName::WindowMaximize, WindowControlArea::Max),
        WindowButton::Close => (IconName::WindowClose, WindowControlArea::Close),
    };
    let close = button == WindowButton::Close;
    let close_red: Hsla = rgb(0xe81123).into();
    h_flex()
        .id(button.id())
        .group(button.id())
        .h_full()
        .w(px(44.))
        .justify_center()
        .occlude()
        .window_control_area(area)
        .hover(|el| el.bg(if close { close_red } else { colors.ghost_element_hover }))
        .active(|el| el.bg(if close { close_red.opacity(0.8) } else { colors.element_hover }))
        .child(
            gpui::svg()
                .path(icon.path())
                .size(px(14.))
                .text_color(colors.icon_muted)
                .group_hover(button.id(), |el| el.text_color(if close { gpui::white() } else { colors.icon })),
        )
        // On Windows the system handles the click through the control area.
        .when(!cfg!(target_os = "windows"), |el| {
            el.on_mouse_move(|_, _, cx| cx.stop_propagation()).on_click(move |_, window, cx| {
                cx.stop_propagation();
                match button {
                    WindowButton::Minimize => window.minimize_window(),
                    WindowButton::Maximize => window.zoom_window(),
                    WindowButton::Close => window.remove_window(),
                }
            })
        })
}

/// Wraps the window's content in the frame Fern draws with client-side decorations on Linux:
/// rounded corners, a border, a shadow, and edges that resize the window. With server-side
/// decorations, and on macOS and Windows, the content is returned as is.
pub fn window_frame(content: impl IntoElement, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    let decorations = if cfg!(any(target_os = "linux", target_os = "freebsd")) { window.window_decorations() } else { Decorations::Server };
    let resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    window.set_client_inset(match decorations {
        Decorations::Client { .. } => SHADOW,
        Decorations::Server => px(0.),
    });

    /// The edge under the mouse, to repaint the cursor when it changes.
    struct HoveredEdge(ResizeEdge);
    impl Global for HoveredEdge {}

    let client = matches!(decorations, Decorations::Client { .. });
    div()
        .id("window-frame")
        .size_full()
        .bg(transparent_black())
        .when(client, |frame| {
            frame
                .when(!tiling.top, |frame| frame.pt(SHADOW))
                .when(!tiling.bottom, |frame| frame.pb(SHADOW))
                .when(!tiling.left, |frame| frame.pl(SHADOW))
                .when(!tiling.right, |frame| frame.pr(SHADOW))
                .when(resizable, |frame| {
                    frame
                        .on_mouse_move(move |event, window, cx| {
                            let edge = resize_edge(event.position, window.window_bounds().get_bounds().size, tiling);
                            if edge != cx.try_global::<HoveredEdge>().map(|edge| edge.0) {
                                window.refresh();
                            }
                        })
                        .on_mouse_down(MouseButton::Left, move |event, window, _| {
                            if let Some(edge) = resize_edge(event.position, window.window_bounds().get_bounds().size, tiling) {
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
                        .when(!(tiling.top || tiling.left), |el| el.rounded_tl(ROUNDING))
                        .when(!(tiling.top || tiling.right), |el| el.rounded_tr(ROUNDING))
                        .when(!(tiling.bottom || tiling.left), |el| el.rounded_bl(ROUNDING))
                        .when(!(tiling.bottom || tiling.right), |el| el.rounded_br(ROUNDING))
                        .when(!tiling.top, |el| el.border_t_1())
                        .when(!tiling.bottom, |el| el.border_b_1())
                        .when(!tiling.left, |el| el.border_l_1())
                        .when(!tiling.right, |el| el.border_r_1())
                        .when(!tiling.is_tiled(), |el| {
                            el.shadow(vec![gpui::BoxShadow::new(px(0.), px(0.), gpui::black().opacity(0.4)).blur_radius(SHADOW / 2.)])
                        })
                })
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
        .when(client && resizable, |frame| {
            frame.child(
                canvas(
                    |_, window, _| {
                        window.insert_hitbox(Bounds::new(point(px(0.), px(0.)), window.window_bounds().get_bounds().size), HitboxBehavior::Normal)
                    },
                    move |_, hitbox, window, cx| {
                        let Some(edge) = resize_edge(window.mouse_position(), window.window_bounds().get_bounds().size, tiling) else {
                            return;
                        };
                        cx.set_global(HoveredEdge(edge));
                        let cursor = match edge {
                            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                            ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
                            ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
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
    if Bounds::new(Point::default(), window).inset(SHADOW * 1.5).contains(&pos) {
        return None;
    }
    let corner = SHADOW * 1.5;
    let left = pos.x < corner;
    let right = pos.x > window.width - corner;
    let top = pos.y < corner;
    let bottom = pos.y > window.height - corner;
    match () {
        _ if !tiling.top && top && left => Some(ResizeEdge::TopLeft),
        _ if !tiling.top && top && right => Some(ResizeEdge::TopRight),
        _ if !tiling.bottom && bottom && left => Some(ResizeEdge::BottomLeft),
        _ if !tiling.bottom && bottom && right => Some(ResizeEdge::BottomRight),
        _ if !tiling.top && pos.y < SHADOW => Some(ResizeEdge::Top),
        _ if !tiling.bottom && pos.y > window.height - SHADOW => Some(ResizeEdge::Bottom),
        _ if !tiling.left && pos.x < SHADOW => Some(ResizeEdge::Left),
        _ if !tiling.right && pos.x > window.width - SHADOW => Some(ResizeEdge::Right),
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
        assert_eq!(resize_edge(point(px(2.), px(2.)), window, free), Some(ResizeEdge::TopLeft));
        assert_eq!(resize_edge(point(px(400.), px(598.)), window, free), Some(ResizeEdge::Bottom));
        assert_eq!(resize_edge(point(px(795.), px(300.)), window, free), Some(ResizeEdge::Right));
        // A tiled edge can't be dragged.
        let tiled = Tiling { right: true, ..Tiling::default() };
        assert_eq!(resize_edge(point(px(795.), px(300.)), window, tiled), None);
    }
}
