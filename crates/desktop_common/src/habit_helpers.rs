use fern_core::Habit;
use gpui::FontWeight;
use ui::prelude::*;

use crate::colors::parse_color;

/// Setting key controlling whether today's habits are shown in the Today task list.
pub const IN_TODAY_SETTING: &str = "habits_in_today";

/// Emoji offered for habits and focus timers.
pub const HABIT_EMOJI: [&str; 35] = [
    "💧", "🏃", "🧘", "💪", "🚴", "🏊", "🚶", "📚", "✍️", "🎹", "🎨", "🧠", "🌅", "😴", "🛏️", "🦷", "🥗", "🍎", "☕",
    "🚭", "🍷", "💊", "🧹", "🪴", "🐕", "💰", "📵", "🙏", "❤️", "📞", "🎯", "⏰", "🌿", "🧴", "☀️",
];

/// Returns the mood emoji for a stored mood value (`10..=50`), or `None` for 0 / unknown.
pub fn mood_emoji(mood: u8) -> Option<&'static str> {
    match mood {
        10 => Some("😢"),
        20 => Some("🙁"),
        30 => Some("😐"),
        40 => Some("🙂"),
        50 => Some("😄"),
        _ => None,
    }
}

/// Paints a compact circular progress ring (for week strip days, partial check-ins, and stats).
pub fn progress_ring(
    size: Pixels,
    fraction: f32,
    fill_color: gpui::Hsla,
    track_color: gpui::Hsla,
    stroke: Pixels,
) -> gpui::Canvas<()> {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = (bounds.size.width.min(bounds.size.height) - stroke) * 0.5;
            if radius <= px(0.5) {
                return;
            }
            let draw_arc = |window: &mut gpui::Window, start_rad: f32, sweep_rad: f32, color: gpui::Hsla| {
                let steps = ((sweep_rad.abs() * 24.0).ceil() as usize).clamp(8, 64);
                let mut builder = gpui::PathBuilder::stroke(stroke);
                for i in 0..=steps {
                    let t = i as f32 / steps as f32;
                    let angle = start_rad + sweep_rad * t;
                    let pt = gpui::point(
                        center.x + radius * angle.cos(),
                        center.y + radius * angle.sin(),
                    );
                    if i == 0 {
                        builder.move_to(pt);
                    } else {
                        builder.line_to(pt);
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            };
            draw_arc(window, 0.0, std::f32::consts::TAU, track_color);
            let frac = fraction.clamp(0.0, 1.0);
            if frac > 0.005 {
                let start = -std::f32::consts::FRAC_PI_2;
                let sweep = std::f32::consts::TAU * frac;
                draw_arc(window, start, sweep, fill_color);
            }
        },
    )
    .size(size)
}

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
