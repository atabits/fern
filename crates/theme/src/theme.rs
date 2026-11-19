//! The color theme. Views read colors through [`ActiveTheme`]: `cx.theme().colors().text`.
//! Views consume semantic colors and surfaces from the same resolved palette.

mod presets;
pub use presets::{DARK_PRESETS, LIGHT_PRESETS, ThemePreset};

use std::sync::Arc;

use gpui::{App, Background, Global, Hsla, black, rgb, transparent_black, white};
use presets::Base;

/// Colors of UI elements.
#[derive(Clone, Debug)]
pub struct ThemeColors {
    /// The main background: the task list and the details panel.
    pub background: Hsla,
    /// Background of the narrow sections sidebar.
    pub sidebar_background: Background,
    pub sidebar_border: Hsla,
    /// Active section icon on the narrow sidebar.
    pub sidebar_icon: Hsla,
    /// Hovered inactive icon on the narrow sidebar.
    pub sidebar_icon_hover: Hsla,
    /// Resting top section icons on the narrow sidebar.
    pub sidebar_icon_muted: Hsla,
    /// Resting bottom utility icons (Sync, Notifications, Help) on the narrow sidebar.
    pub sidebar_icon_bottom: Hsla,
    /// Neutral default avatar tile on the narrow sidebar.
    pub avatar_background: Hsla,
    pub avatar_icon: Hsla,
    pub lists_background: Background,
    pub lists_border: Hsla,
    pub list_text: Hsla,
    pub list_text_muted: Hsla,
    pub list_count: Hsla,
    pub list_hover: Hsla,
    pub list_selected: Hsla,
    pub calendar_background: Background,
    pub matrix_background: Background,
    /// Neutral secondary surface (settings navigation, time axis and cards).
    pub panel_background: Hsla,
    /// Inputs and other raised areas inside a panel.
    pub surface_background: Hsla,
    /// Popovers and menus.
    pub elevated_surface_background: Hsla,
    pub border: Hsla,
    /// Subtle borders, such as separators between rows.
    pub border_variant: Hsla,
    pub element_hover: Hsla,
    pub element_selected: Hsla,
    /// Hover background for elements that are transparent at rest (rows, icon buttons).
    pub ghost_element_hover: Hsla,
    pub ghost_element_selected: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub text_accent: Hsla,
    pub icon: Hsla,
    pub icon_muted: Hsla,
    /// Checkbox borders at rest.
    pub checkbox_border: Hsla,
    pub accent: Hsla,
    /// Foregrounds on a solid accent fill.
    pub on_accent: Hsla,
    pub icon_on_accent: Hsla,
    pub text_selection: Hsla,
}

/// Colors that carry a meaning, also used for priorities.
#[derive(Clone, Debug)]
pub struct StatusColors {
    pub error: Hsla,
    pub warning: Hsla,
    pub info: Hsla,
    pub success: Hsla,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub preset: &'static ThemePreset,
    colors: ThemeColors,
    status: StatusColors,
}

impl Theme {
    pub fn colors(&self) -> &ThemeColors {
        &self.colors
    }

    pub fn status(&self) -> &StatusColors {
        &self.status
    }

    pub fn from_preset(preset: &'static ThemePreset) -> Self {
        let accent: Hsla = rgb(preset.accent).into();
        let (background, panel, title_bar, surface, elevated, ink) = match preset.base {
            Base::Light => (0xffffff, 0xf7f7f8, 0xf5f5f5, 0xeff0f2, 0xffffff, 0x191919),
            Base::Dark { main, screen } => (screen, main, screen, main, main, 0xffffff),
        };
        let [background, panel, title_bar, surface, elevated, ink] =
            [background, panel, title_bar, surface, elevated, ink].map(|c| Hsla::from(rgb(c)));
        let dark = preset.is_dark();
        let border_variant = background.blend(ink.opacity(if dark { 0.07 } else { 0.08 }));
        let (
            sidebar,
            sidebar_border,
            sidebar_icon,
            sidebar_icon_hover,
            sidebar_icon_muted,
            sidebar_icon_bottom,
            lists,
            lists_border,
            calendar,
            matrix,
        ) = match preset.gradient {
            Some(gradient) => (
                gradient.surface(background, 1.),
                transparent_black(),
                white(),
                white().opacity(if dark { 0.60 } else { 0.70 }),
                white().opacity(gradient.rail_icons[0]),
                white().opacity(gradient.rail_icons[1]),
                gradient.surface(background, gradient.lists),
                if dark { border_variant } else { transparent_black() },
                gradient.surface(background, gradient.calendar),
                gradient.surface(if dark { panel } else { background }, gradient.matrix),
            ),
            None => (
                if dark { panel.into() } else { title_bar.into() },
                border_variant,
                if dark { white() } else { accent },
                ink.opacity(0.60),
                ink.opacity(0.40),
                ink.opacity(0.40),
                background.into(),
                border_variant,
                background.into(),
                panel.into(),
            ),
        };
        let (avatar_background, avatar_icon) = if dark {
            (rgb(0x323232).into(), rgb(0x6e6e6e).into())
        } else {
            (rgb(0xefefef).into(), rgb(0xb8b8b8).into())
        };
        let text_muted = background.blend(ink.opacity(if dark { 0.60 } else { 0.62 }));
        let list_ink: Hsla = rgb(preset.list_ink).into();
        let hover = ink.opacity(if dark { 0.06 } else { 0.04 });
        let selected = accent.opacity(if dark { 0.22 } else { 0.13 });
        let ghost_selected = if dark {
            ink.opacity(0.08)
        } else if preset.gradient.is_none() {
            ink.opacity(0.06)
        } else if preset.id == "dark" {
            accent.opacity(0.08)
        } else {
            accent.opacity(0.11)
        };
        let list_hover = if dark {
            ink.opacity(if preset.gradient.is_some() { 0.05 } else { 0.04 })
        } else if matches!(preset.id, "grey" | "classic") {
            ink.opacity(0.04)
        } else if preset.id == "dark" {
            accent.opacity(0.04)
        } else {
            accent.opacity(0.08)
        };
        let list_selected = if dark {
            ink.opacity(0.08)
        } else if preset.gradient.is_none() {
            ink.opacity(0.06)
        } else if preset.id == "dark" {
            accent.opacity(0.08)
        } else {
            accent.opacity(0.12)
        };
        Self {
            preset,
            colors: ThemeColors {
                background,
                sidebar_background: sidebar,
                sidebar_border,
                sidebar_icon,
                sidebar_icon_hover,
                sidebar_icon_muted,
                sidebar_icon_bottom,
                avatar_background,
                avatar_icon,
                lists_background: lists,
                lists_border,
                list_text: list_ink.opacity(if dark { 0.90 } else { 1. }),
                list_text_muted: list_ink.opacity(0.45),
                list_count: if !dark && preset.gradient.is_some() { list_ink } else { list_ink.opacity(0.45) },
                list_hover,
                list_selected,
                calendar_background: calendar,
                matrix_background: matrix,
                panel_background: panel,
                surface_background: surface,
                elevated_surface_background: elevated,
                border: background.blend(ink.opacity(if dark { 0.14 } else { 0.15 })),
                border_variant,
                element_hover: surface.blend(hover),
                element_selected: background.blend(selected),
                ghost_element_hover: hover,
                ghost_element_selected: ghost_selected,
                text: if dark { background.blend(ink.opacity(0.90)) } else { ink },
                text_muted,
                text_placeholder: text_muted,
                text_accent: readable_accent(accent, background, ink),
                icon: background.blend(ink.opacity(0.85)),
                icon_muted: text_muted,
                checkbox_border: background.blend(ink.opacity(0.48)),
                accent,
                on_accent: foreground_on(&[accent], 3.),
                icon_on_accent: foreground_on(&[accent], 3.),
                text_selection: accent.opacity(if dark { 0.34 } else { 0.25 }),
            },
            status: StatusColors {
                error: rgb(if dark { 0xe5484d } else { 0xe13e39 }).into(),
                warning: rgb(0xfaa80c).into(),
                info: rgb(0x4772fa).into(),
                success: rgb(if dark { 0x22c55e } else { 0x2f9e44 }).into(),
            },
        }
    }

    pub fn dark() -> Self {
        Self::from_preset(&DARK_PRESETS[0])
    }

    pub fn light() -> Self {
        Self::from_preset(&LIGHT_PRESETS[0])
    }

    pub fn is_light(&self) -> bool {
        !self.preset.is_dark()
    }
}

// Relative luminance in linear sRGB.
fn luminance(color: Hsla) -> f32 {
    let color = color.to_rgb();
    let linear = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn foreground_on(stops: &[Hsla], target: f32) -> Hsla {
    let minimum = |ink| stops.iter().map(|&stop| contrast(ink, stop)).fold(f32::INFINITY, f32::min);
    let dark = rgb(0x191919).into();
    if minimum(white()) >= target {
        white()
    } else if minimum(dark) >= target {
        dark
    } else if minimum(black()) > minimum(white()) {
        black()
    } else {
        white()
    }
}

/// Keep the original fill color for controls while small accent text remains readable.
fn readable_accent(accent: Hsla, surface: Hsla, ink: Hsla) -> Hsla {
    if contrast(accent, surface) >= 4.5 {
        return accent;
    }
    let (mut low, mut high) = (0., 1.);
    for _ in 0..8 {
        let mid = (low + high) * 0.5;
        if contrast(accent.blend(ink.opacity(mid)), surface) < 4.5 {
            low = mid;
        } else {
            high = mid;
        }
    }
    accent.blend(ink.opacity(high))
}

struct GlobalTheme(Arc<Theme>);

impl Global for GlobalTheme {}

/// Installs the default theme. Call once at startup.
pub fn init(cx: &mut App) {
    cx.set_global(GlobalTheme(Arc::new(Theme::dark())));
}

/// Replaces the active theme; call `cx.refresh_windows()` afterwards.
pub fn set(theme: Theme, cx: &mut App) {
    cx.set_global(GlobalTheme(Arc::new(theme)));
}

pub trait ActiveTheme {
    fn theme(&self) -> &Arc<Theme>;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Arc<Theme> {
        &self.global::<GlobalTheme>().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_presets_resolve_within_their_appearance_and_have_legible_accents() {
        for preset in LIGHT_PRESETS.iter().chain(&DARK_PRESETS) {
            assert!(std::ptr::eq(ThemePreset::resolve(preset.is_dark(), Some(preset.id)), preset));
            let theme = Theme::from_preset(preset);
            let colors = theme.colors();
            assert!(contrast(colors.text_accent, colors.background) >= 4.49, "{}", preset.id);
            assert!(contrast(colors.on_accent, colors.accent) >= 3.0, "{}", preset.id);
            assert_eq!(colors.on_accent, colors.icon_on_accent, "{}", preset.id);
            if let Some(gradient) = preset.gradient {
                assert_eq!(colors.sidebar_icon, white(), "sidebar_icon {}", preset.id);
                assert_eq!(
                    colors.sidebar_icon_hover,
                    white().opacity(if preset.is_dark() { 0.60 } else { 0.70 }),
                    "sidebar_icon_hover {}",
                    preset.id,
                );
                assert_eq!(colors.sidebar_icon_muted, white().opacity(gradient.rail_icons[0]), "{}", preset.id);
                assert_eq!(colors.sidebar_icon_bottom, white().opacity(gradient.rail_icons[1]), "{}", preset.id);
                for stop in gradient.stops {
                    let stop: Hsla = rgb(stop).into();
                    let list_bg = colors.background.blend(stop.opacity(gradient.lists));
                    let list_fg = list_bg.blend(colors.list_text);
                    assert!(contrast(list_fg, list_bg) >= 4.49, "list_text {}", preset.id);
                }
            }
        }
        let light = Theme::light();
        assert_eq!(light.colors().on_accent, white());
        assert_eq!(light.colors().sidebar_icon, light.colors().accent);
        assert_eq!(light.colors().sidebar_icon_hover, Hsla::from(rgb(0x191919)).opacity(0.60));
        assert_eq!(light.colors().sidebar_icon_muted, Hsla::from(rgb(0x191919)).opacity(0.40));
        let dark = Theme::dark();
        assert_eq!(dark.colors().on_accent, white());
        assert_eq!(dark.colors().sidebar_icon, white());
        assert_eq!(dark.colors().sidebar_icon_hover, white().opacity(0.60));
        assert_eq!(dark.colors().sidebar_icon_muted, white().opacity(0.40));
        assert_eq!(ThemePreset::resolve(true, Some("dark")).id, "night");
        assert_eq!(ThemePreset::resolve(false, Some("missing")).id, "grey");
    }
}
