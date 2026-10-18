//! The Habit section, matched to TickTick's: a week strip with daily progress rings and habit
//! cards with totals, streaks and this week's check-ins; the selected habit's statistics and
//! month calendar on the right.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Habit, Id, Store, habit};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n;
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

const CARD_WIDTH: f32 = 365.;
const CARD_HEIGHT: f32 = 107.;

pub struct HabitView {
    store: Rc<Store>,
    habits: Vec<Habit>,
    checkins: HashMap<Id, HashSet<NaiveDate>>,
    new_habit: Option<Entity<TextInput>>,
    /// The habit shown in the details panel.
    selected: Option<Id>,
    /// The month shown in the details panel's calendar.
    month: NaiveDate,
    detail_menu_open: bool,
}

impl HabitView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            store,
            habits: Vec::new(),
            checkins: HashMap::new(),
            new_habit: None,
            selected: None,
            month: calendar::month_start(Local::now().date_naive()),
            detail_menu_open: false,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.habits = self.store.habits().unwrap_or_default();
        self.checkins = self
            .habits
            .iter()
            .map(|h| (h.id, self.store.checkins(h.id).unwrap_or_default()))
            .collect();
        if self.selected.is_some_and(|id| !self.habits.iter().any(|h| h.id == id)) {
            self.selected = None;
        }
        cx.notify();
    }

    fn select(&mut self, habit: Id, cx: &mut Context<Self>) {
        if self.selected != Some(habit) {
            self.selected = Some(habit);
            self.month = calendar::month_start(Local::now().date_naive());
            self.detail_menu_open = false;
        }
        cx.notify();
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.month = calendar::add_months(self.month, delta);
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_select(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.habits.iter().find(|h| h.name == name).map(|h| h.id) {
            self.select(id, cx);
        }
    }

    fn toggle(&mut self, habit: Id, date: NaiveDate, cx: &mut Context<Self>) {
        if let Err(err) = self.store.toggle_checkin(habit, date) {
            eprintln!("failed to check in: {err:#}");
        }
        self.reload(cx);
    }

    fn delete(&mut self, habit: Id, cx: &mut Context<Self>) {
        self.detail_menu_open = false;
        if let Err(err) = self.store.delete_habit(habit) {
            eprintln!("failed to delete the habit: {err:#}");
        }
        self.reload(cx);
    }

    pub fn start_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_name, cx));
        cx.subscribe(&input, |this, input, _: &Submitted, cx| {
            let name = input.read(cx).text().to_owned();
            this.new_habit = None;
            if let Err(err) = this.store.add_habit(&name) {
                eprintln!("failed to add the habit: {err:#}");
            }
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.new_habit = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.new_habit = Some(input);
        cx.notify();
    }

    #[cfg(debug_assertions)]
    pub fn dev_add(&mut self, name: &str, checked_days_ago: &[i64], cx: &mut Context<Self>) {
        if let Ok(habit) = self.store.add_habit(name) {
            let today = Local::now().date_naive();
            for days in checked_days_ago {
                self.store.toggle_checkin(habit.id, today - chrono::Duration::days(*days)).ok();
            }
        }
        self.reload(cx);
    }

    /// The share of habits checked in on `date`, for the week strip's rings.
    fn day_progress(&self, date: NaiveDate) -> f32 {
        if self.habits.is_empty() {
            return 0.;
        }
        let done = self.habits.iter().filter(|h| self.checkins.get(&h.id).is_some_and(|c| c.contains(&date))).count();
        done as f32 / self.habits.len() as f32
    }

    fn render_card(&self, habit: &Habit, week: [NaiveDate; 7], today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let empty = HashSet::new();
        let checked = self.checkins.get(&id).unwrap_or(&empty);
        let stats = habit::stats(checked, today);
        let initial: String = habit.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let group = SharedString::from(format!("habit-{id}"));

        v_flex()
            .id(group)
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .flex_none()
            .p_3()
            .gap_2()
            .rounded(px(8.))
            .border_1()
            .border_color(if self.selected == Some(id) { theme.colors().accent } else { gpui::transparent_black() })
            .bg(theme.colors().surface_background)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .size(px(30.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.colors().accent.opacity(0.35))
                            .child(Label::new(initial).weight(FontWeight::BOLD)),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(Label::new(habit.name.clone()).weight(FontWeight::SEMIBOLD).truncate())
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .gap_0p5()
                                            .child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Accent))
                                            .child(
                                                Label::new(format!("{} {}", stats.total_days, strings.habit_days))
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_0p5()
                                            .child(Icon::new(IconName::Flame).size(IconSize::XSmall))
                                            .child(
                                                Label::new(format!("{} {}", stats.streak, strings.habit_streak))
                                                    .size(LabelSize::XSmall)
                                                    .color(Color::Muted),
                                            ),
                                    ),
                            ),
                    ),
            )
            .child(h_flex().justify_between().px_1().children(week.map(|date| {
                let done = checked.contains(&date);
                div()
                    .id(SharedString::from(format!("checkin-{id}-{date}")))
                    .size(px(16.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if done { theme.colors().accent } else { theme.colors().element_hover })
                    .when(date > today, |el| el.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle(id, date, cx);
                    }))
            })))
            .into_any_element()
    }

    /// The details panel: name, statistics cards and a month calendar of check-ins.
    fn render_detail(&self, habit: &Habit, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let empty = HashSet::new();
        let checked = self.checkins.get(&id).unwrap_or(&empty);
        let stats = habit::stats(checked, today);
        let month = habit::month_stats(checked, self.month, today);
        let status = theme.status().clone();

        let card = |icon: IconName, color: Hsla, label: &'static str, value: String, unit: &'static str| {
            v_flex()
                .flex_1()
                .p_3()
                .gap_1()
                .rounded(px(8.))
                .bg(theme.colors().surface_background)
                .child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(icon).size(IconSize::Small).color(Color::Custom(color)))
                        .child(Label::new(label).size(LabelSize::Small)),
                )
                .child(
                    h_flex()
                        .items_baseline()
                        .gap_1()
                        .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(value))
                        .child(Label::new(unit).size(LabelSize::XSmall)),
                )
        };

        let weekdays = h_flex().children(strings.weekdays_short.iter().map(|day| {
            div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
        }));
        let grid: Vec<AnyElement> = calendar::month_grid(self.month)
            .chunks(7)
            .map(|week| {
                h_flex()
                    .children(week.iter().map(|&date| {
                        let in_month = date.month() == self.month.month();
                        let done = checked.contains(&date);
                        let future = date > today;
                        v_flex()
                            .flex_1()
                            .items_center()
                            .gap_1()
                            .py_1()
                            .when(!in_month || future, |el| el.opacity(0.45))
                            .child(
                                Label::new(date.day().to_string())
                                    .size(LabelSize::Small)
                                    .color(if date == today { Color::Accent } else { Color::Default }),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("detail-checkin-{date}")))
                                    .size(px(26.))
                                    .rounded_full()
                                    .bg(if done { theme.colors().accent } else { theme.colors().element_hover })
                                    .when(!future, |el| {
                                        el.cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, date, cx)))
                                    }),
                            )
                    }))
                    .into_any_element()
            })
            .collect();

        let initial: String = habit.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        v_flex()
            .size_full()
            .px_4()
            .gap_2()
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_none()
                    .gap_2()
                    .child(
                        div()
                            .size(px(30.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(theme.colors().accent.opacity(0.35))
                            .child(Label::new(initial).weight(FontWeight::BOLD)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(habit.name.clone()).size(LabelSize::Large).weight(FontWeight::SEMIBOLD).truncate()),
                    )
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("habit-detail-more", IconName::Ellipsis).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.detail_menu_open = true;
                                    cx.notify();
                                },
                            )))
                            .when(self.detail_menu_open, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .w(px(180.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.detail_menu_open = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("habit-delete")
                                                .start_slot(Icon::new(IconName::Trash).color(Color::Muted))
                                                .child(Label::new(strings.delete))
                                                .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
                                        ),
                                ))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(IconName::CircleCheck, status.success, strings.habit_monthly_checkins, month.days.to_string(), strings.habit_unit_days))
                    .child(card(IconName::Zap, theme.colors().accent, strings.habit_total_checkins, stats.total_days.to_string(), strings.habit_unit_days)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(IconName::Percent, status.warning, strings.habit_monthly_rate, month.rate.to_string(), "%"))
                    .child(card(IconName::Flame, status.error, strings.habit_current_streak, stats.streak.to_string(), strings.habit_unit_days)),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .rounded(px(8.))
                    .bg(theme.colors().surface_background)
                    .child(
                        h_flex()
                            .h(px(32.))
                            .child(
                                IconButton::new("habit-prev-month", IconName::ChevronLeft)
                                    .on_click(cx.listener(|this, _, _, cx| this.shift_month(-1, cx))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .justify_center()
                                    .child(Label::new(calendar::month_title(self.month, strings)).weight(FontWeight::SEMIBOLD)),
                            )
                            .child(
                                IconButton::new("habit-next-month", IconName::ChevronRight)
                                    .on_click(cx.listener(|this, _, _, cx| this.shift_month(1, cx))),
                            ),
                    )
                    .child(weekdays)
                    .children(grid),
            )
            .into_any_element()
    }
}

impl Render for HabitView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let week = calendar::week_of(today);
        let habits = self.habits.clone();
        let cards: Vec<AnyElement> = habits.iter().map(|h| self.render_card(h, week, today, cx)).collect();
        let detail = self
            .selected
            .and_then(|id| habits.iter().find(|h| h.id == id))
            .map(|habit| self.render_detail(habit, today, cx));

        let strip = h_flex().px(px(22.)).pb_3().children(week.map(|date| {
            let progress = self.day_progress(date);
            let is_today = date == today;
            let ring = if progress >= 1. {
                theme.colors().accent
            } else if progress > 0. {
                theme.colors().accent.opacity(0.6)
            } else {
                theme.colors().text_muted
            };
            v_flex()
                .flex_1()
                .items_center()
                .gap_0p5()
                .child(
                    Label::new(strings.weekdays_short[date.weekday().num_days_from_monday() as usize])
                        .size(LabelSize::XSmall)
                        .color(if is_today { Color::Accent } else { Color::Muted }),
                )
                .child(
                    Label::new(date.day().to_string())
                        .size(LabelSize::Small)
                        .color(if is_today { Color::Accent } else { Color::Default }),
                )
                .child(
                    div()
                        .size(px(22.))
                        .rounded_full()
                        .border_2()
                        .border_color(ring)
                        .when(progress >= 1., |el| el.bg(theme.colors().accent.opacity(0.3))),
                )
        }));

        // As in the original, the habits take a left column; the right side is left for details.
        let column = v_flex()
            .w(px(790.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(theme.colors().border)
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_none()
                    .px(px(22.))
                    .gap_1()
                    .child(Label::new(strings.section_habits).size(LabelSize::Large).weight(FontWeight::BOLD))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                    .child(div().flex_1())
                    .child(IconButton::new("habit-list", IconName::List))
                    .child(
                        div()
                            .relative()
                            .child(
                                IconButton::new("habit-add", IconName::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| this.start_new(window, cx))),
                            )
                            .when_some(self.new_habit.clone(), |el, input| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    popover(cx)
                                        .w(px(240.))
                                        .p_2()
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.new_habit = None;
                                            cx.notify();
                                        }))
                                        .child(h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(input)),
                                ))
                            }),
                    )
                    .child(IconButton::new("habit-more", IconName::Ellipsis)),
            )
            .child(strip)
            .child(
                div()
                    .id("habit-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(22.))
                    .when(cards.is_empty(), |el| {
                        el.child(div().pt_8().child(Label::new(strings.habit_empty).color(Color::Muted)))
                    })
                    .child(div().flex().flex_wrap().gap(px(12.)).children(cards)),
            );

        h_flex()
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(column)
            .child(div().flex_1().min_w_0().h_full().map(|el| match detail {
                Some(detail) => el.child(detail),
                None => el
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Label::new(strings.habit_select).color(Color::Muted)),
            }))
    }
}
