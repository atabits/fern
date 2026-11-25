//! Shared `WindowOptions` constructors and frameless window drag-strip chrome helpers.

use gpui::{
    App, Bounds, Context, Decorations, DisplayId, Div, InteractiveElement, MouseButton, Pixels,
    Point, SharedString, Size, Stateful, TitlebarOptions, Window, WindowBounds,
    WindowControlArea, WindowDecorations, WindowKind, WindowOptions, point, px,
};
use ui::prelude::*;

use crate::layout;
use crate::paths;

/// Per-view drag state for Linux/macOS custom title-bar drag strips.
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowDragState {
    pub should_move: bool,
}

/// Parameters for building [`WindowOptions`] for custom-chromed desktop windows.
pub struct CsdWindowOptions<'a> {
    pub title: Option<SharedString>,
    pub size: Size<Pixels>,
    pub min_size: Option<Size<Pixels>>,
    pub resizable: bool,
    pub focus: bool,
    pub show: bool,
    pub popup_on_non_linux: bool,
    pub y_fraction: Option<f32>,
    pub traffic_light_position: Option<Point<Pixels>>,
    pub display_id: Option<DisplayId>,
    pub bounds_override: Option<Bounds<Pixels>>,
    pub cx: &'a App,
}

impl<'a> CsdWindowOptions<'a> {
    pub fn new(size: Size<Pixels>, cx: &'a App) -> Self {
        Self {
            title: None,
            size,
            min_size: None,
            resizable: true,
            focus: true,
            show: true,
            popup_on_non_linux: false,
            y_fraction: None,
            traffic_light_position: None,
            display_id: None,
            bounds_override: None,
            cx,
        }
    }
}

/// Builds consistent [`WindowOptions`] with Linux CSD padding fallback, `app_id`, and custom
/// client decorations.
pub fn csd_window_options(opts: CsdWindowOptions<'_>) -> WindowOptions {
    let linux = cfg!(any(target_os = "linux", target_os = "freebsd"));
    let bounds = opts.bounds_override.unwrap_or_else(|| {
        let mut centered = Bounds::centered(opts.display_id, opts.size, opts.cx);
        if let Some(frac) = opts.y_fraction
            && let Some(display) = opts.cx.primary_display()
        {
            let screen = display.bounds();
            centered.origin = point(centered.origin.x, screen.origin.y + screen.size.height * frac);
        }
        centered
    });
    WindowOptions {
        focus: opts.focus,
        show: opts.show,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: opts.title,
            appears_transparent: true,
            traffic_light_position: opts.traffic_light_position,
        }),
        window_decorations: Some(WindowDecorations::Client),
        app_owns_titlebar_drag: true,
        is_resizable: opts.resizable,
        kind: if opts.popup_on_non_linux && !linux {
            WindowKind::PopUp
        } else {
            WindowKind::Normal
        },
        window_min_size: opts.min_size,
        app_id: Some(paths::app_id()),
        ..Default::default()
    }
}

/// Computes centered launcher bounds padded for Linux client-side shadows.
pub fn launcher_bounds(width: f32, height: f32, y_fraction: f32, cx: &App) -> Bounds<Pixels> {
    let mut bounds = Bounds::centered(
        None,
        layout::with_linux_csd_pad(gpui::size(px(width), px(height))),
        cx,
    );
    if let Some(display) = cx.primary_display() {
        let screen = display.bounds();
        bounds.origin = point(bounds.origin.x, screen.origin.y + screen.size.height * y_fraction);
    }
    bounds
}

/// Attaches native/CSD window drag, double-click zoom/maximize, and Linux CSD system menu chrome
/// to a header drag strip element.
pub fn apply_drag_strip<V: 'static>(
    el: Stateful<Div>,
    drag_state: fn(&mut V) -> &mut WindowDragState,
    window: &Window,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let linux = cfg!(any(target_os = "linux", target_os = "freebsd"));
    let mac = cfg!(target_os = "macos");
    let windows = cfg!(target_os = "windows");
    let client_side = matches!(window.window_decorations(), Decorations::Client { .. });

    el.window_control_area(WindowControlArea::Drag)
        .when(!windows, |bar| {
            bar.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| drag_state(this).should_move = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| drag_state(this).should_move = false),
            )
            .on_mouse_down_out(
                cx.listener(move |this, _, _, _| drag_state(this).should_move = false),
            )
            .on_mouse_move(cx.listener(move |this, _, window, _| {
                let state = drag_state(this);
                if state.should_move {
                    state.should_move = false;
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
            bar.on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position)
            })
        })
}
