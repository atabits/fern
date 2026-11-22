use gpui::{
    App, Hsla, IntoElement, ParentElement, RenderOnce, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use theme::{ActiveTheme, Theme};

use crate::Toggleable;

/// An on/off switch. It has no click handler of its own: the row it sits in toggles it, as in
/// the original's settings.
#[derive(IntoElement, Default)]
pub struct Switch {
    on: bool,
}

impl Switch {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Toggleable for Switch {
    fn toggle_state(mut self, on: bool) -> Self {
        self.on = on;
        self
    }
}

/// Computes contrasting `(track_bg, track_border, thumb_bg)` for the OFF state so the switch
/// remains crisp and readable across both Light and Dark themes.
fn switch_off_colors(theme: &Theme) -> (Hsla, Hsla, Hsla) {
    let colors = theme.colors();
    if theme.is_dark() {
        (colors.element_hover, colors.border, colors.icon)
    } else {
        (
            colors.element_hover,
            colors.checkbox_border.opacity(0.55),
            colors.icon_muted,
        )
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors();
        let (off_track_bg, off_track_border, off_thumb_bg) = switch_off_colors(theme);
        div()
            .w(px(32.))
            .h(px(18.))
            .px(px(2.))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .when(self.on, |el| el.justify_end().bg(colors.accent))
            .when(!self.on, |el| {
                el.bg(off_track_bg)
                    .border_1()
                    .border_color(off_track_border)
            })
            .child(div().size(px(12.)).rounded_full().bg(if self.on {
                colors.icon_on_accent
            } else {
                off_thumb_bg
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::{DARK_PRESETS, LIGHT_PRESETS};

    fn relative_luminance(color: Hsla) -> f32 {
        let rgb = color.to_rgb();
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b)
    }

    fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
        let la = relative_luminance(a);
        let lb = relative_luminance(b);
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn off_switch_thumb_has_visible_contrast_in_all_light_and_dark_presets() {
        for preset in LIGHT_PRESETS.iter().chain(DARK_PRESETS.iter()) {
            let theme = Theme::from_preset(preset);
            let (track_bg, _track_border, thumb_bg) = switch_off_colors(&theme);
            let ratio = contrast_ratio(thumb_bg, track_bg);
            assert!(
                ratio >= 3.0,
                "OFF switch thumb/track contrast too low in preset {:?}: {ratio:.2}",
                preset.id
            );
            assert_ne!(
                thumb_bg, track_bg,
                "OFF switch thumb must not match track in preset {:?}",
                preset.id
            );
        }
    }
}
