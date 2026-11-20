use gpui::{Decorations, Window};
use ui::prelude::*;

/// The room around the window for its shadow, which is also where it is grabbed to resize.
pub const TITLE_BAR_SHADOW: Pixels = px(10.);

#[cfg(target_os = "macos")]
pub const SIDEBAR_WIDTH: f32 = 72.;
#[cfg(not(target_os = "macos"))]
pub const SIDEBAR_WIDTH: f32 = 50.;

/// The overlaid caption strip's height: Windows' standard 32 px caption button height.
pub fn title_bar_height() -> Pixels {
    px(32.)
}

/// The top margin workspace headers leave so their controls clear the overlaid window buttons.
pub fn header_inset() -> Pixels {
    title_bar_height() - px(8.)
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
