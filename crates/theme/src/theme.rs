//! The color theme. Views read colors through [`ActiveTheme`]: `cx.theme().colors().text`.
//! Colors are semantic (what an element is), not literal, so a light theme can be added later
//! without touching the views.

use std::sync::Arc;

use gpui::{App, Global, Hsla, rgb, rgba};

/// Colors of UI elements.
#[derive(Clone, Debug)]
pub struct ThemeColors {
    /// The main background: the task list and the details panel.
    pub background: Hsla,
    /// Background of the narrow sections sidebar.
    pub sidebar_background: Hsla,
    /// Background of the lists column.
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
    pub name: &'static str,
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

    /// The default dark theme, tuned to match TickTick's dark look.
    pub fn dark() -> Self {
        Self {
            name: "Fern Dark",
            colors: ThemeColors {
                background: rgb(0x1b1c20).into(),
                sidebar_background: rgb(0x16171a).into(),
                panel_background: rgb(0x18191d).into(),
                surface_background: rgb(0x24262b).into(),
                elevated_surface_background: rgb(0x24262b).into(),
                border: rgb(0x34373e).into(),
                border_variant: rgb(0x26282d).into(),
                element_hover: rgb(0x2c2f35).into(),
                element_selected: rgb(0x2a2d33).into(),
                ghost_element_hover: rgb(0x23252a).into(),
                ghost_element_selected: rgb(0x2a2d33).into(),
                text: rgb(0xe6e7ea).into(),
                text_muted: rgb(0x80848e).into(),
                text_placeholder: rgb(0x80848e).into(),
                text_accent: rgb(0x4772fa).into(),
                icon: rgb(0xe6e7ea).into(),
                icon_muted: rgb(0x80848e).into(),
                checkbox_border: rgb(0x9a9ca3).into(),
                accent: rgb(0x4772fa).into(),
                text_selection: rgba(0x4772fa55).into(),
            },
            status: StatusColors {
                error: rgb(0xe5484d).into(),
                warning: rgb(0xf5a524).into(),
                info: rgb(0x4772fa).into(),
                success: rgb(0x22c55e).into(),
            },
        }
    }
}

struct GlobalTheme(Arc<Theme>);

impl Global for GlobalTheme {}

/// Installs the default theme. Call once at startup.
pub fn init(cx: &mut App) {
    cx.set_global(GlobalTheme(Arc::new(Theme::dark())));
}

pub trait ActiveTheme {
    fn theme(&self) -> &Arc<Theme>;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Arc<Theme> {
        &self.global::<GlobalTheme>().0
    }
}
