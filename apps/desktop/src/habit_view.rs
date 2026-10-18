//! The Habit section, matched to TickTick's: a week strip with daily progress rings and habit
//! cards with totals, streaks and this week's check-ins.

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
}

impl HabitView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let mut this = Self { store, habits: Vec::new(), checkins: HashMap::new(), new_habit: None };
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
        cx.notify();
    }

    fn toggle(&mut self, habit: Id, date: NaiveDate, cx: &mut Context<Self>) {
        if let Err(err) = self.store.toggle_checkin(habit, date) {
            eprintln!("failed to check in: {err:#}");
        }
        self.reload(cx);
    }

    fn delete(&mut self, habit: Id, cx: &mut Context<Self>) {
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
            .id(group.clone())
            .group(group.clone())
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .flex_none()
            .p_3()
            .gap_2()
            .rounded(px(8.))
            .bg(theme.colors().surface_background)
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
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, date, cx)))
            })))
            .child(
                div().absolute().top(px(6.)).right(px(6.)).opacity(0.).group_hover(group, |s| s.opacity(1.)).child(
                    IconButton::new("delete-habit", IconName::X)
                        .size(px(22.))
                        .icon_size(IconSize::Small)
                        .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
                ),
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
            .child(div().flex_1().h_full())
    }
}
