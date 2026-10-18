use gpui::{App, Hsla};
use theme::ActiveTheme;

/// A semantic color, resolved against the active theme when rendered.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum Color {
    #[default]
    Default,
    Muted,
    Placeholder,
    Accent,
    Error,
    Warning,
    Info,
    Success,
    Custom(Hsla),
}

impl Color {
    pub fn color(&self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Color::Default => theme.colors().text,
            Color::Muted => theme.colors().text_muted,
            Color::Placeholder => theme.colors().text_placeholder,
            Color::Accent => theme.colors().text_accent,
            Color::Error => theme.status().error,
            Color::Warning => theme.status().warning,
            Color::Info => theme.status().info,
            Color::Success => theme.status().success,
            Color::Custom(color) => *color,
        }
    }
}

impl From<Hsla> for Color {
    fn from(color: Hsla) -> Self {
        Color::Custom(color)
    }
}
