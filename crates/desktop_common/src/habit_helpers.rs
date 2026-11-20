use fern_core::Habit;
use gpui::FontWeight;
use ui::prelude::*;

use crate::colors::parse_color;

/// Setting key controlling whether today's habits are shown in the Today task list.
pub const IN_TODAY_SETTING: &str = "habits_in_today";

/// Emoji offered for habits and focus timers; the original's illustrated icon set is its own
/// artwork.
pub const HABIT_EMOJI: [&str; 35] = [
    "💧", "🏃", "🧘", "💪", "🚴", "🏊", "🚶", "📚", "✍️", "🎹", "🎨", "🧠", "🌅", "😴", "🛏️", "🦷", "🥗", "🍎", "☕",
    "🚭", "🍷", "💊", "🧹", "🪴", "🐕", "💰", "📵", "🙏", "❤️", "📞", "🎯", "⏰", "🌿", "🧴", "☀️",
];

/// A habit's round avatar: its emoji or text on its color, or the name's initial.
pub fn avatar(habit: &Habit, size: Pixels, cx: &App) -> Div {
    icon_avatar(habit.icon.as_deref(), habit.color.as_deref(), &habit.name, size, cx)
}

/// A round avatar of an emoji or text on a color (the accent by default), or the name's initial;
/// habits and focus timers have one.
pub fn icon_avatar(icon: Option<&str>, color: Option<&str>, name: &str, size: Pixels, cx: &App) -> Div {
    let theme = cx.theme();
    let label = icon
        .map(str::to_owned)
        .unwrap_or_else(|| name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default());
    let bg = parse_color(color).unwrap_or(theme.colors().accent).opacity(0.35);
    div()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(bg)
        .text_size(size * 0.5)
        .font_weight(FontWeight::BOLD)
        .child(label)
}
