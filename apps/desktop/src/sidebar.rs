//! The narrow sidebar with app sections, modeled on TickTick's.

use gpui::{
    AnyElement, App, ClickEvent, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::*, px, svg,
};

use crate::i18n::Strings;
use crate::theme;

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
        Section::Habits,
        Section::Pomodoro,
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

    fn icon(self) -> &'static str {
        match self {
            Section::Tasks => "icons/square-check.svg",
            Section::Calendar => "icons/calendar-days.svg",
            Section::Habits => "icons/circle-dot.svg",
            Section::Pomodoro => "icons/clock.svg",
            Section::Search => "icons/search.svg",
            Section::Matrix => "icons/layout-grid.svg",
            Section::Countdown => "icons/calendar-heart.svg",
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
) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .py_3()
        .gap_1()
        .bg(theme::sidebar())
        .border_r_1()
        .border_color(theme::border())
        .child(
            div()
                .id("avatar")
                .relative()
                .mb_3()
                .size(px(30.))
                .rounded_full()
                .bg(theme::accent())
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_click(on_avatar)
                .child(svg().path("icons/user.svg").size(px(16.)).text_color(theme::text()))
                .children(account_menu),
        )
        .children(Section::ALL.into_iter().map(|section| {
            let on_select = on_select.clone();
            icon_button(section.id().into(), section.icon(), section == active, move |_, window, cx| {
                on_select(section, window, cx)
            })
        }))
        .child(div().flex_1())
        .child(icon_button("sync".into(), "icons/refresh-cw.svg", false, |_, _, _| {}))
        .child(icon_button("notifications".into(), "icons/bell.svg", false, |_, _, _| {}))
        .child(icon_button("help".into(), "icons/circle-question-mark.svg", false, |_, _, _| {}))
}

fn icon_button(
    id: SharedString,
    icon: &'static str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let color = if active { theme::text() } else { theme::text_muted() };
    div()
        .id(id)
        .size(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .when(active, |el| el.bg(theme::surface_hover()))
        .hover(|s| s.bg(theme::surface_hover()))
        .on_click(on_click)
        .child(svg().path(icon).size(px(20.)).text_color(color))
}
