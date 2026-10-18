//! The narrow sidebar with app sections, modeled on TickTick's.

use gpui::{AnyElement, ClickEvent};
use ui::prelude::*;

use crate::i18n::Strings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Tasks,
    Calendar,
    Habits,
    Pomodoro,
    Search,
    Matrix,
    Countdown,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::Tasks,
        Section::Calendar,
        Section::Pomodoro,
        Section::Habits,
        Section::Search,
        Section::Matrix,
        Section::Countdown,
    ];

    pub fn title(self, strings: &Strings) -> &'static str {
        match self {
            Section::Tasks => strings.section_tasks,
            Section::Calendar => strings.section_calendar,
            Section::Habits => strings.section_habits,
            Section::Pomodoro => strings.section_pomodoro,
            Section::Search => strings.section_search,
            Section::Matrix => strings.section_matrix,
            Section::Countdown => strings.section_countdown,
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::Tasks => IconName::SquareCheck,
            Section::Calendar => IconName::CalendarDays,
            // The original uses a target for Focus and a clock-like glyph for Habit.
            Section::Habits => IconName::Clock,
            Section::Pomodoro => IconName::CircleDot,
            Section::Search => IconName::Search,
            Section::Matrix => IconName::LayoutGrid,
            Section::Countdown => IconName::CalendarHeart,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Section::Tasks => "section-tasks",
            Section::Calendar => "section-calendar",
            Section::Habits => "section-habits",
            Section::Pomodoro => "section-pomodoro",
            Section::Search => "section-search",
            Section::Matrix => "section-matrix",
            Section::Countdown => "section-countdown",
        }
    }
}

pub const WIDTH: f32 = 50.;

/// `account_menu` is rendered next to the avatar while the account menu is open.
pub fn render(
    active: Section,
    on_select: impl Fn(Section, &mut Window, &mut App) + Clone + 'static,
    on_avatar: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    account_menu: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let button = |id: &'static str, icon: IconName| {
        IconButton::new(id, icon).size(px(36.)).icon_size(IconSize::XLarge)
    };

    v_flex()
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .items_center()
        .py_3()
        .gap_1()
        .bg(colors.sidebar_background)
        .border_r_1()
        .border_color(colors.border)
        .child(
            div()
                .id("avatar")
                .relative()
                .mb_3()
                .size(px(30.))
                .rounded_full()
                .bg(colors.accent)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_click(on_avatar)
                .child(Icon::new(IconName::User).color(Color::Default))
                .children(account_menu),
        )
        .children(Section::ALL.into_iter().map(|section| {
            let on_select = on_select.clone();
            button(section.id(), section.icon())
                .toggle_state(section == active)
                .on_click(move |_, window, cx| on_select(section, window, cx))
        }))
        .child(div().flex_1())
        .child(button("sync", IconName::RefreshCw))
        .child(button("notifications", IconName::Bell))
        .child(button("help", IconName::CircleQuestionMark))
}
