//! The narrow sidebar with app sections, modeled on TickTick's.

use gpui::{AnyElement, ClickEvent};
use ui::prelude::*;

use crate::i18n::{self, Strings};

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
    /// Whether Settings → Features leaves the section on; Tasks and Search always are.
    pub fn enabled(self, features: fern_core::Features) -> bool {
        match self {
            Section::Tasks | Section::Search => true,
            Section::Calendar => features.calendar,
            Section::Matrix => features.matrix,
            Section::Habits => features.habits,
            Section::Pomodoro => features.focus,
            Section::Countdown => features.countdown,
        }
    }

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
            // Filled glyphs as in the original, drawn for Fern (`assets/icons/fern_*.svg`): a target
            // for Focus and a clock for Habit.
            Section::Tasks => IconName::FernTasks,
            Section::Calendar => IconName::FernCalendar,
            Section::Habits => IconName::FernHabit,
            Section::Pomodoro => IconName::FernFocus,
            Section::Search => IconName::Search,
            Section::Matrix => IconName::FernMatrix,
            Section::Countdown => IconName::FernCountdown,
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

/// The notification bell: a red badge with the unread count, as in the original.
pub struct Bell {
    pub unread: usize,
    pub on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    /// The notification list, while it is open.
    pub menu: Option<AnyElement>,
}

/// The sync button: its tooltip says how the last sync went; a failed one shows in red.
pub struct SyncButton {
    pub tooltip: String,
    pub failed: bool,
    pub on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

/// `account_menu` is rendered next to the avatar while the account menu is open.
pub fn render(
    active: Section,
    on_select: impl Fn(Section, &mut Window, &mut App) + Clone + 'static,
    on_avatar: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    account_menu: Option<AnyElement>,
    bell: Bell,
    sync: SyncButton,
    features: fern_core::Features,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let strings = i18n::t(cx);
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
        // The sections, spaced and sized as in the original (50px apart, 24px glyphs).
        .child(v_flex().gap(px(10.)).children(Section::ALL.into_iter().filter(|s| s.enabled(features)).map(|section| {
            let on_select = on_select.clone();
            // As in the original, the open section's icon turns bright, with no background.
            IconButton::new(section.id(), section.icon())
                .size(px(40.))
                .icon_size(IconSize::Custom(px(24.)))
                .tooltip(Tooltip::text(section.title(strings)))
                .icon_color(if section == active { Color::Default } else { Color::Muted })
                .on_click(move |_, window, cx| on_select(section, window, cx))
        })))
        .child(div().flex_1())
        .child(
            button("sync", IconName::RefreshCw)
                .when(sync.failed, |b| b.icon_color(Color::Error))
                .tooltip(Tooltip::text(sync.tooltip))
                .on_click(sync.on_click),
        )
        .child(
            div()
                .relative()
                .child(button("notifications", IconName::Bell).tooltip(Tooltip::text(strings.notifications)).on_click(bell.on_click))
                .when(bell.unread > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .min_w(px(14.))
                            .h(px(14.))
                            .px(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(cx.theme().status().error)
                            .text_color(gpui::white())
                            .text_size(px(9.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(if bell.unread > 99 { "99+".to_owned() } else { bell.unread.to_string() }),
                    )
                })
                .children(bell.menu),
        )
        .child(
            button("help", IconName::CircleQuestionMark)
                .tooltip(Tooltip::text(strings.shortcuts))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::ShowShortcuts), cx)),
        )
}
