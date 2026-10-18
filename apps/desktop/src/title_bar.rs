//! The window's own title bar on Windows, as Zed's: the system one is hidden, the top
//! `TOP_INSET` of every column moves the window, and the minimize, maximize and close buttons
//! sit in the top-right corner. Elsewhere the system draws the title bar.

use std::sync::OnceLock;

use gpui::{Hsla, Rgba, WindowControlArea};
use ui::prelude::*;

/// Whether the app draws its own title bar.
pub const INTEGRATED: bool = cfg!(target_os = "windows");

/// The caption buttons' size, as Windows' own.
const BUTTON_WIDTH: f32 = 46.;
const BUTTON_HEIGHT: f32 = 32.;

/// The strip along the top that moves the window and maximizes it on a double click. It lies
/// under the columns, which keep their clickable parts below `TOP_INSET`.
pub fn drag_area() -> impl IntoElement {
    div().absolute().top_0().left_0().right_0().h(px(crate::TOP_INSET)).window_control_area(WindowControlArea::Drag)
}

/// Minimize, maximize (or restore) and close; Windows itself handles the clicks.
pub fn caption_buttons(window: &mut Window, cx: &App) -> impl IntoElement {
    let maximize = if window.is_maximized() { CaptionButton::Restore } else { CaptionButton::Maximize };
    h_flex()
        .id("caption-buttons")
        .absolute()
        .top_0()
        .right_0()
        .h(px(BUTTON_HEIGHT))
        .font_family(icon_font(window))
        .child(CaptionButton::Minimize.render(window, cx))
        .child(maximize.render(window, cx))
        .child(CaptionButton::Close.render(window, cx))
}

/// Windows 11 draws the caption glyphs with Segoe Fluent Icons; Windows 10 has the same glyphs
/// in Segoe MDL2 Assets.
fn icon_font(window: &Window) -> &'static str {
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
            // Keeps the drag area under the button out of Windows' hit test.
            .occlude()
            .w(px(BUTTON_WIDTH))
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
