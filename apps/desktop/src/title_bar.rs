//! The window's title bar, drawn by the app on every platform as Zed's: the system one is
//! hidden, the top `HEIGHT` of every column moves the window and maximizes it on a double click,
//! and the window's buttons sit in that strip: macOS keeps its own traffic lights on the left,
//! Windows gets its caption buttons in the top-right corner, and Linux gets the desktop's
//! buttons on the side it puts them, with the app's own window edges for resizing.

use std::cell::Cell;
use std::sync::OnceLock;

use gpui::{
    CursorStyle, Decorations, HitboxBehavior, Hsla, MouseButton, Point, ResizeEdge, Rgba, Size, Stateful, Tiling,
    TitlebarOptions, WindowButton, WindowButtonLayout, WindowControlArea, canvas, point, svg,
};
use icons::IconName;
use ui::prelude::*;

/// How tall the title bar is: every column starts this far down (`crate::TOP_INSET`).
pub const HEIGHT: f32 = 32.;

/// The room around a Linux window that draws its own frame: its shadow, and where the pointer
/// resizes it.
const SHADOW: Pixels = px(10.);

pub fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some("Fern".into()),
        appears_transparent: true,
        // The traffic lights in the middle of the title bar, at its left.
        traffic_light_position: Some(point(px(12.), px(9.))),
    }
}

thread_local! {
    /// Set by a press on the title bar: the next move of the pointer moves the window, so a
    /// double click still reaches the title bar.
    static SHOULD_MOVE: Cell<bool> = const { Cell::new(false) };
}

/// The strip along the top that moves the window, and the window's buttons in it. It lies over
/// the columns, which keep their clickable parts below `HEIGHT`, and under the dialogs.
pub fn render(window: &mut Window, cx: &App) -> impl IntoElement {
    let drag_area = div()
        .id("title-bar")
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(HEIGHT))
        // Windows moves the window itself: it asks the app what is under the pointer.
        .window_control_area(WindowControlArea::Drag)
        .when(!cfg!(target_os = "windows"), |el| {
            el.on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    double_click(window);
                } else {
                    SHOULD_MOVE.set(true);
                }
            })
            .on_mouse_up(MouseButton::Left, |_, _, _| SHOULD_MOVE.set(false))
            .on_mouse_down_out(|_, _, _| SHOULD_MOVE.set(false))
            .on_mouse_move(|_, window, _| {
                if SHOULD_MOVE.replace(false) {
                    window.start_window_move();
                }
            })
            .when(cfg!(any(target_os = "linux", target_os = "freebsd")), |el| {
                el.on_mouse_down(MouseButton::Right, |event, window, _| window.show_window_menu(event.position))
            })
        });

    div()
        .absolute()
        .inset_0()
        .child(drag_area)
        .when(cfg!(target_os = "windows"), |el| el.child(caption_buttons(window, cx)))
        .when(
            cfg!(any(target_os = "linux", target_os = "freebsd"))
                && matches!(window.window_decorations(), Decorations::Client { .. }),
            |el| {
                let layout = cx.button_layout().unwrap_or(WindowButtonLayout {
                    left: [None; 3],
                    right: [Some(WindowButton::Minimize), Some(WindowButton::Maximize), Some(WindowButton::Close)],
                });
                el.child(linux_buttons("title-bar-left", &layout.left, window, cx).left_0())
                    .child(linux_buttons("title-bar-right", &layout.right, window, cx).right_0())
            },
        )
}

fn double_click(window: &mut Window) {
    if cfg!(target_os = "macos") {
        // Zooms or minimizes, as the system settings say.
        window.titlebar_double_click();
    } else if window.is_resizable() && window.window_controls().maximize {
        window.zoom_window();
    }
}

/// Minimize, maximize (or restore) and close in the desktop's order.
fn linux_buttons(id: &'static str, buttons: &[Option<WindowButton>], window: &Window, cx: &App) -> Stateful<Div> {
    let controls = window.window_controls();
    let colors = cx.theme().colors();
    let maximized = window.is_maximized();
    h_flex()
        .id(id)
        .absolute()
        .top_0()
        .h(px(HEIGHT))
        .px_3()
        .gap_3()
        .children(
            buttons
                .iter()
                .flatten()
                .filter(|button| match button {
                    WindowButton::Minimize => controls.minimize,
                    WindowButton::Maximize => controls.maximize,
                    WindowButton::Close => true,
                })
                .map(|button| {
                    let (id, icon) = match button {
                        WindowButton::Minimize => ("caption-minimize", IconName::WindowMinimize),
                        WindowButton::Maximize if maximized => ("caption-restore", IconName::WindowRestore),
                        WindowButton::Maximize => ("caption-maximize", IconName::WindowMaximize),
                        WindowButton::Close => ("caption-close", IconName::WindowClose),
                    };
                    let button = *button;
                    h_flex()
                        .id(id)
                        .justify_center()
                        .size(px(20.))
                        .rounded_full()
                        .bg(colors.element_hover)
                        .hover(|style| style.bg(colors.element_selected))
                        .cursor_pointer()
                        .child(svg().path(icon.path()).size(px(16.)).flex_none().text_color(colors.icon))
                        .on_click(move |_, window, _| match button {
                            WindowButton::Minimize => window.minimize_window(),
                            WindowButton::Maximize => window.zoom_window(),
                            WindowButton::Close => window.remove_window(),
                        })
                }),
        )
        // Keeps the clicks from the title bar under the buttons.
        .occlude()
}

/// The caption buttons' size, as Windows' own.
const CAPTION_WIDTH: f32 = 46.;

/// Minimize, maximize (or restore) and close; Windows itself handles the clicks.
fn caption_buttons(window: &mut Window, cx: &App) -> impl IntoElement {
    let maximize = if window.is_maximized() { CaptionButton::Restore } else { CaptionButton::Maximize };
    h_flex()
        .id("caption-buttons")
        .absolute()
        .top_0()
        .right_0()
        .h(px(HEIGHT))
        .font_family(caption_font(window))
        .child(CaptionButton::Minimize.render(window, cx))
        .child(maximize.render(window, cx))
        .child(CaptionButton::Close.render(window, cx))
}

/// Windows 11 draws the caption glyphs with Segoe Fluent Icons; Windows 10 has the same glyphs
/// in Segoe MDL2 Assets.
fn caption_font(window: &Window) -> &'static str {
    static FONT: OnceLock<&'static str> = OnceLock::new();
    FONT.get_or_init(|| {
        if window.text_system().all_font_names().iter().any(|name| name == "Segoe Fluent Icons") {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        }
    })
}

#[derive(Clone, Copy)]
enum CaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl CaptionButton {
    fn render(self, window: &Window, cx: &App) -> impl IntoElement {
        let (id, glyph, area, enabled) = match self {
            Self::Minimize => ("caption-minimize", "\u{e921}", WindowControlArea::Min, window.is_minimizable()),
            Self::Maximize => ("caption-maximize", "\u{e922}", WindowControlArea::Max, window.is_resizable()),
            Self::Restore => ("caption-restore", "\u{e923}", WindowControlArea::Max, window.is_resizable()),
            Self::Close => ("caption-close", "\u{e8bb}", WindowControlArea::Close, true),
        };
        let colors = cx.theme().colors();
        let (hover_bg, hover_fg, active_bg, active_fg) = match self {
            Self::Close => {
                let red: Hsla = Rgba { r: 232. / 255., g: 17. / 255., b: 32. / 255., a: 1. }.into();
                (red, gpui::white(), red.opacity(0.8), gpui::white().opacity(0.8))
            }
            _ => (colors.element_hover, colors.text, colors.element_selected, colors.text),
        };
        h_flex()
            .id(id)
            .justify_center()
            // Keeps the title bar under the button out of Windows' hit test.
            .occlude()
            .w(px(CAPTION_WIDTH))
            .h_full()
            .text_size(px(10.))
            .text_color(if enabled { colors.text_muted } else { colors.text_placeholder })
            .when(enabled, |el| {
                el.hover(|style| style.bg(hover_bg).text_color(hover_fg))
                    .active(|style| style.bg(active_bg).text_color(active_fg))
            })
            .window_control_area(area)
            .child(glyph)
    }
}

/// The window's frame when the app draws it (Linux, unless the desktop draws it): a border and
/// a shadow around `content`, and edges that resize the window. Elsewhere `content` as it is.
pub fn client_side_decorations(content: impl IntoElement, window: &mut Window, cx: &App) -> Stateful<Div> {
    let decorations = window.window_decorations();
    let resizable = window.is_resizable();
    let tiling = match decorations {
        Decorations::Server => Tiling::default(),
        Decorations::Client { tiling } => tiling,
    };
    window.set_client_inset(match decorations {
        Decorations::Client { .. } => SHADOW,
        Decorations::Server => px(0.),
    });
    let client = matches!(decorations, Decorations::Client { .. });

    div()
        .id("window-backdrop")
        .size_full()
        .when(client, |el| {
            el.when(!tiling.top, |el| el.pt(SHADOW))
                .when(!tiling.bottom, |el| el.pb(SHADOW))
                .when(!tiling.left, |el| el.pl(SHADOW))
                .when(!tiling.right, |el| el.pr(SHADOW))
                .when(resizable, |el| {
                    el.on_mouse_move(move |event, window, _| {
                        let edge = resize_edge(event.position, window.window_bounds().get_bounds().size, tiling);
                        if RESIZE_EDGE.replace(edge) != edge {
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
                .when(client, |el| {
                    el.border_color(cx.theme().colors().border)
                        .when(!tiling.top, |el| el.border_t_1())
                        .when(!tiling.bottom, |el| el.border_b_1())
                        .when(!tiling.left, |el| el.border_l_1())
                        .when(!tiling.right, |el| el.border_r_1())
                        .when(!tiling.is_tiled(), |el| {
                            el.shadow(vec![
                                gpui::BoxShadow::new(px(0.), px(0.), gpui::black().opacity(0.4)).blur_radius(SHADOW / 2.),
                            ])
                        })
                })
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
        .when(client && resizable, |el| {
            // The resize cursor over the edges.
            el.child(
                canvas(
                    |_, window, _| {
                        let size = window.window_bounds().get_bounds().size;
                        window.insert_hitbox(gpui::Bounds::new(point(px(0.), px(0.)), size), HitboxBehavior::Normal)
                    },
                    move |_, hitbox, window, _| {
                        let Some(edge) = resize_edge(window.mouse_position(), window.window_bounds().get_bounds().size, tiling)
                        else {
                            return;
                        };
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

thread_local! {
    /// The edge under the pointer, so the frame is drawn again with its cursor when it changes.
    static RESIZE_EDGE: Cell<Option<ResizeEdge>> = const { Cell::new(None) };
}

/// The edge or corner of the window at `pos`, as Zed's: the shadow's width along the sides, a
/// bit more at the corners.
fn resize_edge(pos: Point<Pixels>, window_size: Size<Pixels>, tiling: Tiling) -> Option<ResizeEdge> {
    let bounds = gpui::Bounds::new(Point::default(), window_size).inset(SHADOW * 1.5);
    if bounds.contains(&pos) {
        return None;
    }
    let corner = gpui::size(SHADOW * 1.5, SHADOW * 1.5);
    let at = |x, y| gpui::Bounds::new(point(x, y), corner).contains(&pos);
    let (right, bottom) = (window_size.width - corner.width, window_size.height - corner.height);
    if !tiling.top && at(px(0.), px(0.)) {
        Some(ResizeEdge::TopLeft)
    } else if !tiling.top && at(right, px(0.)) {
        Some(ResizeEdge::TopRight)
    } else if !tiling.bottom && at(px(0.), bottom) {
        Some(ResizeEdge::BottomLeft)
    } else if !tiling.bottom && at(right, bottom) {
        Some(ResizeEdge::BottomRight)
    } else if !tiling.top && pos.y < SHADOW {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y > window_size.height - SHADOW {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x < SHADOW {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x > window_size.width - SHADOW {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}
