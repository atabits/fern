use gpui::{App, Hsla};
use theme::ActiveTheme;

/// A semantic color, resolved against the active theme when rendered.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum Color {
    #[default]
    Default,
    Muted,
    Placeholder,
    Icon,
    IconMuted,
    Accent,
    OnAccent,
    IconOnAccent,
    Error,
    OnError,
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
            Color::Icon => theme.colors().icon,
            Color::IconMuted => theme.colors().icon_muted,
            Color::Accent => theme.colors().text_accent,
            Color::OnAccent => theme.colors().on_accent,
            Color::IconOnAccent => theme.colors().icon_on_accent,
            Color::Error => theme.status().error,
            Color::OnError => theme.status().on_error,
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
