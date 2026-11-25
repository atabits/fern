//! Fern's integrated caption strip and window frame: the window is opened without the system title
//! bar (`window_options`), every workspace column extends to the top edge, and this module overlays
//! a transparent drag strip, the window buttons and, on Linux, the frame's border, shadow and
//! resize edges. After Zed's `platform_title_bar` and `client_side_decorations`.
//!
//! - macOS keeps its traffic lights over the top-left corner.
//! - Windows draws nothing itself: the strip's areas are tagged with [`WindowControlArea`], so the
//!   system handles dragging, double clicks, snap layouts and the buttons' clicks.
//! - Linux asks the compositor for client-side decorations. When it draws its own (server-side),
//!   the drag strip stays without duplicate window buttons.

use std::rc::Rc;

use gpui::{
    Decorations, EventEmitter, MouseButton, Point, TitlebarOptions, WindowButton, WindowButtonLayout,
    WindowControlArea, WindowDecorations, point, rgb,
};
use ui::prelude::*;

use crate::actions::CloseWindow;
use crate::release_notes::ReleaseNotes;
use crate::updater::Status;

pub use desktop_common::layout::{
    LINUX_WINDOW_ROUNDING as ROUNDING, is_linux_compact_header as should_compact_linux_header,
    title_bar_height as height,
};
/// macOS traffic lights: centered inside the 72 px sections rail.
const TRAFFIC_LIGHTS: Point<Pixels> = point(px(10.), px(11.));

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
    /// Optional contrast override for caption buttons (e.g., Life Clock Classic B&W white right half).
    caption_appearance: Option<theme::Appearance>,
    _updates: Option<gpui::Subscription>,
}

pub struct ShowReleaseNotes(pub Rc<ReleaseNotes>);

impl EventEmitter<ShowReleaseNotes> for TitleBar {}

impl TitleBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let updates = crate::updater::updater(cx).map(|updater| cx.observe(&updater, |_, _, cx| cx.notify()));
        Self {
            should_move: false,
            caption_appearance: None,
            _updates: updates,
        }
    }

    pub fn set_caption_appearance(
        &mut self,
        appearance: Option<theme::Appearance>,
        cx: &mut Context<Self>,
    ) {
        if self.caption_appearance != appearance {
            self.caption_appearance = appearance;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn caption_appearance(&self) -> Option<theme::Appearance> {
        self.caption_appearance
    }

    /// "Restart to Update" once a new version is downloaded, or "Update to …" when it is out
    /// but this installation is updated some other way.
    fn update_buttons(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let updater = crate::updater::updater(cx)?;
        let strings = crate::i18n::t(cx);
        let (notes, label) = match updater.read(cx).status() {
            Status::Ready(notes) => (notes.clone(), strings.restart_to_update.to_owned()),
            Status::Available(notes) => (notes.clone(), strings.update_to.replace("{version}", &notes.version)),
            _ => return None,
        };
        Some(
            h_flex()
                .flex_none()
                .mr_1p5()
                .gap_1()
                .child(
                    div()
                        .debug_selector(|| "title-bar-update-notes".to_owned())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            ui::Button::new("title-bar-update-notes", strings.whats_new)
                                .variant(ui::ButtonVariant::Secondary)
                                .size(ui::ButtonSize::Sm)
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(ShowReleaseNotes(notes.clone()));
                                })),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "title-bar-restart-update".to_owned())
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            ui::Button::new("update-button", label)
                                .variant(ui::ButtonVariant::Primary)
                                .size(ui::ButtonSize::Sm)
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    updater.update(cx, |updater, cx| updater.restart_to_update(cx));
                                }),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let update_buttons = self.update_buttons(cx);
        let decorations = window.window_decorations();
        let client_side = matches!(decorations, Decorations::Client { .. });
        let fullscreen = window.is_fullscreen();
        let linux = cfg!(any(target_os = "linux", target_os = "freebsd"));
        let compact = should_compact_linux_header(window, cx);
        let mac = cfg!(target_os = "macos");
        let windows = cfg!(target_os = "windows");
        let strip_height = if compact && update_buttons.is_none() {
            height(window, cx)
        } else {
            px(32.)
        };
        let buttons = if windows {
            Some(WindowButtonLayout {
                left: [None; 3],
                right: [Some(WindowButton::Minimize), Some(WindowButton::Maximize), Some(WindowButton::Close)],
            })
        } else if linux && client_side {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                Some(cx.button_layout().unwrap_or_else(WindowButtonLayout::linux_default))
            }
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            {
                None
            }
        } else {
            None
        };
        let caption_appearance = self.caption_appearance;
        let side = |buttons: [Option<WindowButton>; 3], id: &'static str, appearance| {
            buttons.iter().any(Option::is_some).then(|| {
                WindowButtons {
                    id,
                    buttons,
                    caption_appearance: appearance,
                }
                .into_any_element()
            })
        };
        // Only right-side buttons sit over the white future half of Life Clock Classic B&W;
        // left-side buttons (Linux GNOME/macOS-style layout) sit over the rail / dark past half.
        let left_buttons = buttons.and_then(|b| side(b.left, "left-window-buttons", None)).filter(|_| !fullscreen);
        let right_buttons = buttons.and_then(|b| side(b.right, "right-window-buttons", caption_appearance)).filter(|_| !fullscreen);

        h_flex()
            .id("title-bar")
            .debug_selector(|| "title-bar".to_owned())
            .window_control_area(WindowControlArea::Drag)
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(strip_height)
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
            .child(div().flex_1().min_w_0().h_full())
            .children(update_buttons)
            .children(right_buttons)
    }
}

/// The minimize, maximize and close buttons on one side of the bar.
#[derive(IntoElement)]
struct WindowButtons {
    id: &'static str,
    buttons: [Option<WindowButton>; 3],
    caption_appearance: Option<theme::Appearance>,
}

impl RenderOnce for WindowButtons {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let controls = window.window_controls();
        let maximized = window.is_maximized();
        let active = window.is_window_active();
        let caption_appearance = self.caption_appearance;
        h_flex()
            .id(self.id)
            .h_full()
            .flex_none()
            .when(cfg!(any(target_os = "linux", target_os = "freebsd")), |el| {
                el.gap_1()
                    .when(self.id == "left-window-buttons", |el| el.pl_1p5())
                    .when(self.id == "right-window-buttons", |el| el.pr_1p5())
            })
            .when(!cfg!(target_os = "windows"), |el| {
                el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .children(self.buttons.into_iter().flatten().filter(|button| match button {
                WindowButton::Minimize => controls.minimize && window.is_minimizable(),
                WindowButton::Maximize => controls.maximize && window.is_resizable(),
                WindowButton::Close => true,
            }).map(|button| window_button(button, maximized, active, caption_appearance, cx)))
    }
}

/// Windows 11 ships "Segoe Fluent Icons" and Windows 10 "Segoe MDL2 Assets"; Zed renders caption
/// buttons from those system fonts so DirectWrite hints them identically to native Windows chrome.
#[cfg(target_os = "windows")]
fn windows_caption_font(cx: &App) -> &'static str {
    static FONT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        if cx.text_system().all_font_names().iter().any(|name| name == "Segoe Fluent Icons") {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        }
    })
}

fn window_button(
    button: WindowButton,
    maximized: bool,
    active: bool,
    caption_appearance: Option<theme::Appearance>,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let area = match button {
        WindowButton::Minimize => WindowControlArea::Min,
        WindowButton::Maximize => WindowControlArea::Max,
        WindowButton::Close => WindowControlArea::Close,
    };
    let close = button == WindowButton::Close;
    let close_red: Hsla = rgb(0xe81123).into();
    let (resting, hover_fg, hover_bg, active_bg): (Hsla, Hsla, Hsla, Hsla) =
        if caption_appearance == Some(theme::Appearance::Light) {
            (
                if active { rgb(0x1f1f1f).into() } else { rgb(0x5c5c5c).into() },
                if close { gpui::white() } else { rgb(0x111111).into() },
                gpui::black().opacity(0.08),
                gpui::black().opacity(0.14),
            )
        } else {
            (
                if active { colors.text } else { colors.icon_muted },
                if close { gpui::white() } else { colors.text },
                colors.ghost_element_hover,
                colors.element_hover,
            )
        };
    let el = h_flex()
        .id(button.id())
        .group(button.id())
        .justify_center()
        .occlude()
        .window_control_area(area)
        .text_color(resting);
    #[cfg(not(target_os = "windows"))]
    let hover_icon = if cfg!(any(target_os = "linux", target_os = "freebsd"))
        && close
        && caption_appearance != Some(theme::Appearance::Light)
    {
        close_red
    } else {
        hover_fg
    };
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let el = {
        let close_hover_bg = close_red.opacity(0.16);
        let close_active_bg = close_red.opacity(0.24);
        el.size(px(26.))
            .rounded_full()
            .my_auto()
            .hover(|el| el.bg(if close { close_hover_bg } else { hover_bg }).text_color(hover_icon))
            .active(|el| el.bg(if close { close_active_bg } else { active_bg }).text_color(hover_icon))
    };
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let el = el
        .h_full()
        .w(px(46.))
        .hover(|el| el.bg(if close { close_red } else { hover_bg }).text_color(hover_fg))
        .active(|el| el.bg(if close { close_red.opacity(0.8) } else { active_bg }).text_color(hover_fg));
    #[cfg(target_os = "windows")]
    let el = {
        let glyph = match button {
            WindowButton::Minimize => "\u{e921}",
            WindowButton::Maximize if maximized => "\u{e923}",
            WindowButton::Maximize => "\u{e922}",
            WindowButton::Close => "\u{e8bb}",
        };
        el.font_family(windows_caption_font(cx))
            .text_size(px(10.))
            .line_height(px(10.))
            .child(glyph)
    };
    #[cfg(not(target_os = "windows"))]
    let el = {
        let icon = match button {
            WindowButton::Minimize => IconName::WindowMinimize,
            WindowButton::Maximize if maximized => IconName::WindowRestore,
            WindowButton::Maximize => IconName::WindowMaximize,
            WindowButton::Close => IconName::WindowClose,
        };
        el.child(
            gpui::svg()
                .path(icon.path())
                .size(px(if cfg!(any(target_os = "linux", target_os = "freebsd")) { 15. } else { 14. }))
                .text_color(resting)
                .group_hover(button.id(), move |svg| svg.text_color(hover_icon)),
        )
    };
    el
        // On Windows the system handles the click through the control area.
        .when(!cfg!(target_os = "windows"), |el| {
            el.on_mouse_move(|_, _, cx| cx.stop_propagation()).on_click(move |_, window, cx| {
                cx.stop_propagation();
                match button {
                    WindowButton::Minimize => {
                        if !crate::tray::hide_to_tray_if_enabled() {
                            window.minimize_window();
                        }
                    }
                    WindowButton::Maximize => window.zoom_window(),
                    WindowButton::Close => window.dispatch_action(Box::new(CloseWindow), cx),
                }
            })
        })
}

/// Wraps the window's content in the frame Fern draws with client-side decorations on Linux:
/// rounded corners, a border, a shadow, and edges that resize the window. With server-side
/// decorations, and on macOS and Windows, the content is returned as is.
pub fn window_frame(content: impl IntoElement, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    desktop_common::layout::linux_window_frame(content, ROUNDING, window, cx)
}
