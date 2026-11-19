//! The narrow sidebar with app sections, modeled on TickTick's.

use fern_core::RailSection;
use gpui::{AnyElement, ClickEvent};
use ui::prelude::*;

use crate::drag::DraggedRailSection;
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

impl From<RailSection> for Section {
    fn from(section: RailSection) -> Self {
        match section {
            RailSection::Tasks => Section::Tasks,
            RailSection::Calendar => Section::Calendar,
            RailSection::Pomodoro => Section::Pomodoro,
            RailSection::Habits => Section::Habits,
            RailSection::Search => Section::Search,
            RailSection::Matrix => Section::Matrix,
            RailSection::Countdown => Section::Countdown,
        }
    }
}

impl From<Section> for RailSection {
    fn from(section: Section) -> Self {
        match section {
            Section::Tasks => RailSection::Tasks,
            Section::Calendar => RailSection::Calendar,
            Section::Pomodoro => RailSection::Pomodoro,
            Section::Habits => RailSection::Habits,
            Section::Search => RailSection::Search,
            Section::Matrix => RailSection::Matrix,
            Section::Countdown => RailSection::Countdown,
        }
    }
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

    pub fn icon(self) -> IconName {
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

#[cfg(target_os = "macos")]
pub const WIDTH: f32 = 72.;
#[cfg(not(target_os = "macos"))]
pub const WIDTH: f32 = 50.;

/// Height of one rail icon slot (`40 px` icon + `5 px` vertical padding on each side, zero dead gap),
/// matching TickTick's `px-[4px] py-[5px]` + `w-[40px] h-[40px]` rail entries.
pub const SLOT_HEIGHT: f32 = 50.;

/// Distance from the top of the sidebar to the first rail icon slot (`avatar 32 + mb 7 + gap 4`).
pub const SECTIONS_TOP_OFFSET: f32 = 43.;

type ClickCallback = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The notification bell: a red badge with the unread count, as in the original.
pub struct Bell {
    pub unread: usize,
    pub on_click: ClickCallback,
    /// The notification list or the update notice attached to the bell.
    pub menu: Option<AnyElement>,
}

/// The sync button: its tooltip says how the last sync went; a failed one shows in red. Fern
/// works locally until a server is set, and until then there is no button.
pub struct SyncButton {
    pub tooltip: String,
    pub failed: bool,
    pub on_click: ClickCallback,
}

/// `account_menu` is rendered next to the avatar while the account menu is open.
#[allow(clippy::too_many_arguments)]
pub fn render(
    active: Section,
    hovered: Option<Section>,
    order: &[RailSection],
    dragging: Option<Section>,
    on_hover: impl Fn(Section, bool, &mut App) + Clone + 'static,
    on_select: impl Fn(Section, &mut Window, &mut App) + Clone + 'static,
    on_start_drag: impl Fn(Section, &mut App) + Clone + 'static,
    on_avatar: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    account_menu: Option<AnyElement>,
    bell: Bell,
    sync: Option<SyncButton>,
    features: fern_core::Features,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.theme().colors();
    let strings = i18n::t(cx);
    let active_drag = cx.has_active_drag();
    let dragging = dragging.filter(|_| active_drag);
    let button = |id: &'static str, icon: IconName| {
        IconButton::new(id, icon)
            .size(px(36.))
            .icon_size(IconSize::XLarge)
            .icon_color(colors.sidebar_icon_bottom)
            .hover_icon_color(colors.sidebar_icon_hover)
            .hover_color(gpui::transparent_black())
    };
    let ordered_sections: Vec<Section> = if order.is_empty() {
        Section::ALL.to_vec()
    } else {
        order.iter().copied().map(Section::from).collect()
    };

    v_flex()
        .debug_selector(|| "sidebar".to_owned())
        .flex_none()
        .w(px(WIDTH))
        .h_full()
        .items_center()
        .pt(crate::title_bar::height())
        .pb_3()
        .gap_1()
        .bg(colors.sidebar_background)
        .border_r_1()
        .border_color(colors.sidebar_border)
        .child(
            div()
                .id("avatar")
                .relative()
                .flex_none()
                .mb(px(7.))
                .size(px(32.))
                .rounded(px(6.))
                .bg(colors.avatar_background)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_click(on_avatar)
                .child(Icon::new(IconName::User).color(colors.avatar_icon))
                .children(account_menu),
        )
        // Scroll the section icons in a short window so the controls below stay reachable.
        .child(
            v_flex()
                .id("sidebar-sections")
                .flex_1()
                .min_h_0()
                .w_full()
                .items_center()
                .overflow_y_scroll()
                .children(ordered_sections.into_iter().filter(|s| s.enabled(features)).map(|section| {
                    let on_select = on_select.clone();
                    let on_hover = on_hover.clone();
                    let on_start_drag = on_start_drag.clone();
                    let is_dragged = dragging == Some(section);
                    let payload = DraggedRailSection {
                        section: RailSection::from(section),
                        icon: section.icon(),
                        active: section == active,
                    };
                    // Each entry occupies a contiguous 50 px slot (40 px icon + 5 px top/bottom)
                    // with zero dead gaps; while dragged, it leaves a vacant slot as neighbors slide.
                    div()
                        .id(section.id())
                        .debug_selector(move || section.id().to_owned())
                        .relative()
                        .flex_none()
                        .w(px(48.))
                        .h(px(SLOT_HEIGHT))
                        .on_hover(move |hovered: &bool, _, cx| on_hover(section, *hovered, cx))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(move |_, window, cx| on_select(section, window, cx))
                        .on_drag(payload, move |dragged, _, _, cx| {
                            on_start_drag(section, cx);
                            cx.new(|_| dragged.clone())
                        })
                        .child(if is_dragged {
                            div()
                                .size(px(40.))
                                .rounded(px(8.))
                                .bg(colors.sidebar_icon_muted.opacity(0.10))
                                .into_any_element()
                        } else {
                            div()
                                .size(px(40.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(section.icon()).size(IconSize::Custom(px(24.))).color(
                                    if section == active {
                                        colors.sidebar_icon
                                    } else if hovered == Some(section) && !active_drag {
                                        colors.sidebar_icon_hover
                                    } else {
                                        colors.sidebar_icon_muted
                                    },
                                ))
                                .into_any_element()
                        })
                        .when(hovered == Some(section) && !active_drag, |el| {
                            el.child(side_tooltip(section.title(strings), cx))
                        })
                })),
        )
        .children(sync.map(|sync| {
            button("sync", IconName::RefreshCw)
                .when(sync.failed, |b| b.icon_color(Color::Error).hover_icon_color(Color::Error))
                .tooltip(Tooltip::text(sync.tooltip))
                .on_click(sync.on_click)
        }))
        .child(
            div()
                .id("notification-bell")
                .debug_selector(|| "notification-bell".to_owned())
                .relative()
                .flex_none()
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

/// A tooltip at the right of a sidebar icon, level with it, shown while it is hovered, as the
/// original's sidebar shows them.
fn side_tooltip(text: &'static str, cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors();
    div().absolute().left(px(46.)).top_0().h_full().flex().items_center().child(gpui::deferred(
        div()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .text_color(colors.text)
            .text_size(px(12.))
            .whitespace_nowrap()
            .child(text),
    ))
}
