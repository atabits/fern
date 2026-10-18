//! The Habit section, matched to TickTick's: a week strip with daily progress rings and habit
//! cards with totals, streaks and this week's check-ins; the selected habit's statistics and
//! month calendar on the right.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Checkin, CustomSection, Goal, Habit, HabitFrequency, HabitSection, Id, Store, habit};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n;
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

const CARD_WIDTH: f32 = 365.;

/// Emoji offered for habits; the original's illustrated icon set is its own artwork.
const HABIT_EMOJI: [&str; 35] = [
    "💧", "🏃", "🧘", "💪", "🚴", "🏊", "🚶", "📚", "✍️", "🎹", "🎨", "🧠", "🌅", "😴", "🛏️", "🦷", "🥗", "🍎", "☕",
    "🚭", "🍷", "💊", "🧹", "🪴", "🐕", "💰", "📵", "🙏", "❤️", "📞", "🎯", "⏰", "🌿", "🧴", "☀️",
];

/// A habit's round avatar: its emoji or text on its color, or the name's initial.
pub fn avatar(habit: &Habit, size: Pixels, cx: &App) -> Div {
    let theme = cx.theme();
    let label = habit
        .icon
        .clone()
        .unwrap_or_else(|| habit.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default());
    let bg = crate::lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent).opacity(0.35);
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
const CARD_HEIGHT: f32 = 107.;

/// The "Edit Habit" popover: name and daily goal.
struct HabitEdit {
    name: Entity<TextInput>,
    unit: Entity<TextInput>,
    /// `None`: "Achieve it all".
    amount: Option<u32>,
    reminders: Vec<chrono::NaiveTime>,
    frequency: HabitFrequency,
    section: HabitSection,
    /// The "Add Section" field, while it is shown.
    new_section: Option<Entity<TextInput>>,
    start_date: NaiveDate,
    goal_days: Option<u32>,
    /// The start date's calendar month, while it is open.
    start_calendar: Option<NaiveDate>,
    icon: Option<String>,
    color: Option<String>,
    /// The icon picker, with its tab (0 emoji, 1 text), while open.
    icon_picker: Option<usize>,
    icon_text: Entity<TextInput>,
    /// A field for adding a reminder time.
    reminder_input: Entity<TextInput>,
}

pub struct HabitView {
    store: Rc<Store>,
    habits: Vec<Habit>,
    checkins: HashMap<Id, HashSet<NaiveDate>>,
    /// Check-ins with amounts and notes, for the details panel.
    records: HashMap<Id, Vec<Checkin>>,
    /// The note field under the habit log, for today's check-in.
    note_input: Entity<TextInput>,
    editing: Option<HabitEdit>,
    new_habit: Option<Entity<TextInput>>,
    /// The habit shown in the details panel.
    selected: Option<Id>,
    /// The month shown in the details panel's calendar.
    month: NaiveDate,
    detail_menu_open: bool,
    /// The header's Active / Archived switch.
    show_archived: bool,
    header_menu_open: bool,
    /// Sections the user added.
    sections: Vec<CustomSection>,
    header_more: Option<HeaderMore>,
    /// The last export's result, shown under the header.
    export_message: Option<String>,
    /// The header's list button: habits as full-width rows instead of cards (kept in settings).
    list_layout: bool,
}

const LAYOUT_SETTING: &str = "habit_layout";
/// Whether habits show in the Today and Next 7 Days lists ("1" by default).
pub const IN_TODAY_SETTING: &str = "habits_in_today";
const SORT_SETTING: &str = "habits_sort_by_status";

/// The header's "…" menu and its Settings popover.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderMore {
    Menu,
    Settings,
}

impl HabitView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let note_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_note_placeholder, cx));
        cx.subscribe(&note_input, |this, input, _: &Submitted, cx| {
            let note = input.read(cx).text().to_owned();
            if let Some(habit) = this.selected {
                let today = Local::now().date_naive();
                if let Err(err) = this.store.set_checkin_note(habit, today, &note) {
                    eprintln!("failed to save the note: {err:#}");
                }
                input.update(cx, |input, cx| input.set_text("", cx));
                this.reload(cx);
            }
        })
        .detach();
        let mut this = Self {
            store,
            habits: Vec::new(),
            checkins: HashMap::new(),
            records: HashMap::new(),
            note_input,
            editing: None,
            new_habit: None,
            selected: None,
            month: calendar::month_start(Local::now().date_naive()),
            detail_menu_open: false,
            show_archived: false,
            header_menu_open: false,
            list_layout: false,
            sections: Vec::new(),
            header_more: None,
            export_message: None,
        };
        this.list_layout = this.store.setting(LAYOUT_SETTING).ok().flatten().as_deref() == Some("list");
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.habits = self.store.habits().unwrap_or_default();
        self.sections = self.store.habit_sections().unwrap_or_default();
        self.records =
            self.habits.iter().map(|h| (h.id, self.store.checkin_records(h.id).unwrap_or_default())).collect();
        self.checkins = self.habits.iter().map(|h| (h.id, self.store.checkins(h.id).unwrap_or_default())).collect();
        if self.selected.is_some_and(|id| !self.habits.iter().any(|h| h.id == id)) {
            self.selected = None;
        }
        cx.notify();
    }

    pub fn select(&mut self, habit: Id, cx: &mut Context<Self>) {
        if self.selected != Some(habit) {
            self.selected = Some(habit);
            self.month = calendar::month_start(Local::now().date_naive());
            self.detail_menu_open = false;
        }
        cx.notify();
    }

    fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(habit) = self.selected.and_then(|id| self.habits.iter().find(|h| h.id == id)).cloned() else {
            return;
        };
        let strings = i18n::t(cx);
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_name, cx);
            input.set_text(habit.name.clone(), cx);
            input
        });
        let unit = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_unit_placeholder, cx);
            input.set_text(habit.goal.as_ref().map(|g| g.unit.clone()).unwrap_or_default(), cx);
            input
        });
        let reminder_input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.time_placeholder, cx));
        cx.subscribe(&reminder_input, |this, input, Submitted(text), cx| {
            if let Some(time) = crate::due_picker::parse_time(text) {
                this.update_edit(cx, |e| {
                    if !e.reminders.contains(&time) {
                        e.reminders.push(time);
                        e.reminders.sort();
                    }
                });
                input.update(cx, |input, cx| input.set_text("", cx));
            }
        })
        .detach();
        let icon_text = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_text_placeholder, cx));
        cx.subscribe(&icon_text, |this, _, Submitted(text), cx| {
            let text: String = text.trim().chars().take(2).collect();
            this.update_edit(cx, |e| {
                e.icon = (!text.is_empty()).then_some(text);
                e.icon_picker = None;
            });
        })
        .detach();
        window.focus(&name.focus_handle(cx), cx);
        self.editing = Some(HabitEdit {
            icon: habit.icon.clone(),
            color: habit.color.clone(),
            start_date: habit.start_date.unwrap_or_else(|| Local::now().date_naive()),
            goal_days: habit.goal_days,
            start_calendar: None,
            icon_picker: None,
            icon_text,
            name,
            unit,
            amount: habit.goal.as_ref().map(|g| g.amount),
            reminders: habit.reminders.clone(),
            frequency: habit.frequency,
            section: habit.section,
            new_section: None,
            reminder_input,
        });
        self.detail_menu_open = false;
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let (Some(edit), Some(id)) = (self.editing.take(), self.selected) else { return };
        let name = edit.name.read(cx).text().to_owned();
        let goal = edit.amount.map(|amount| Goal { amount, unit: edit.unit.read(cx).text().to_owned() });
        if let Err(err) = self
            .store
            .update_habit(id, &name, goal.as_ref())
            .and_then(|()| self.store.set_habit_reminders(id, &edit.reminders))
            .and_then(|()| self.store.set_habit_frequency(id, edit.frequency))
            .and_then(|()| self.store.set_habit_look(id, edit.icon.as_deref(), edit.color.as_deref()))
            .and_then(|()| self.store.set_habit_span(id, Some(edit.start_date), edit.goal_days))
            .and_then(|()| self.store.set_habit_section(id, edit.section))
        {
            eprintln!("failed to update the habit: {err:#}");
        }
        self.reload(cx);
    }

    fn update_edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut HabitEdit)) {
        if let Some(edit) = self.editing.as_mut() {
            change(edit);
        }
        cx.notify();
    }

    fn render_edit(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let Some(preview) = self.selected.and_then(|id| self.habits.iter().find(|h| h.id == id)).cloned() else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let option = |id: &'static str, label: &'static str, on: bool| {
            h_flex()
                .id(id)
                .h(px(26.))
                .gap_2()
                .cursor_pointer()
                .child(Checkbox::new(SharedString::from(format!("{id}-box"))).toggle_state(on))
                .child(Label::new(label).size(LabelSize::Small))
        };
        let field = |child: AnyElement| {
            h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(child)
        };
        let amount = edit.amount;
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border))
                .child(Label::new(label).size(LabelSize::Small))
        };
        popover(cx)
            .w(px(300.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.editing = None;
                cx.notify();
            }))
            .child(Label::new(strings.edit_habit).weight(FontWeight::SEMIBOLD))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("habit-icon-button")
                            .cursor_pointer()
                            .child(avatar(
                                &Habit { icon: edit.icon.clone(), color: edit.color.clone(), ..preview.clone() },
                                px(34.),
                                cx,
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_edit(cx, |e| e.icon_picker = if e.icon_picker.is_some() { None } else { Some(0) })
                            })),
                    )
                    .child(div().flex_1().child(field(edit.name.clone().into_any_element()))),
            )
            .when_some(edit.icon_picker, |el, tab| el.child(self.render_icon_picker(edit, tab, cx)))
            .child(Label::new(strings.habit_frequency).size(LabelSize::Small).color(Color::Muted))
            .child(self.render_frequency(edit.frequency, cx))
            .child(self.render_span(edit, cx))
            .child(Label::new(strings.habit_section).size(LabelSize::Small).color(Color::Muted))
            .child(h_flex().flex_wrap().gap_1().children(HabitSection::ordered(&self.sections).into_iter().enumerate().map(|(i, section)| {
                let selected = edit.section == section;
                div()
                    .id(SharedString::from(format!("habit-section-{i}")))
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(12.))
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.colors().accent.opacity(0.25)))
                    .when(!selected, |el| el.bg(theme.colors().element_hover))
                    .child(Label::new(self.section_label(section, cx)).size(LabelSize::XSmall).color(if selected { Color::Accent } else { Color::Default }))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_edit(cx, |e| e.section = section)))
            })).child(
                div()
                    .id("habit-section-add")
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(12.))
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme.colors().border)
                    .child(Label::new(format!("+ {}", strings.habit_add_section)).size(LabelSize::XSmall))
                    .on_click(cx.listener(|this, _, window, cx| this.start_new_section(window, cx))),
            ))
            .when_some(edit.new_section.clone(), |el, input| {
                el.child(h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(input))
            })
            .child(Label::new(strings.habit_goal).size(LabelSize::Small).color(Color::Muted))
            .child(
                option("goal-all", strings.habit_goal_all, amount.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.update_edit(cx, |e| e.amount = None))),
            )
            .child(
                option("goal-amount", strings.habit_goal_amount, amount.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.update_edit(cx, |e| e.amount = e.amount.or(Some(1))))),
            )
            .when_some(amount, |el, amount| {
                el.child(
                    h_flex()
                        .gap_2()
                        .child(
                            IconButton::new("goal-fewer", IconName::Minus).on_click(cx.listener(|this, _, _, cx| {
                                this.update_edit(cx, |e| e.amount = e.amount.map(|a| a.saturating_sub(1).max(1)))
                            })),
                        )
                        .child(div().min_w(px(24.)).flex().justify_center().child(Label::new(amount.to_string())))
                        .child(
                            IconButton::new("goal-more", IconName::Plus).on_click(cx.listener(|this, _, _, cx| {
                                this.update_edit(cx, |e| e.amount = e.amount.map(|a| (a + 1).min(999)))
                            })),
                        )
                        .child(div().flex_1().child(field(edit.unit.clone().into_any_element())))
                        .child(Label::new(strings.habit_per_day).size(LabelSize::Small).color(Color::Muted)),
                )
            })
            .child(Label::new(strings.reminder).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(edit.reminders.iter().map(|&time| {
                        h_flex()
                            .id(SharedString::from(format!("habit-reminder-{time}")))
                            .h(px(24.))
                            .pl_2()
                            .gap_0p5()
                            .rounded(px(5.))
                            .bg(theme.colors().element_selected)
                            .child(Label::new(crate::calendar::format_time(time)).size(LabelSize::Small))
                            .child(
                                IconButton::new(SharedString::from(format!("habit-reminder-remove-{time}")), IconName::X)
                                    .size(px(20.))
                                    .icon_size(IconSize::XSmall)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_edit(cx, |e| e.reminders.retain(|t| *t != time))
                                    })),
                            )
                    }))
                    .child(div().flex_1().min_w(px(120.)).child(field(edit.reminder_input.clone().into_any_element()))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(button("edit-cancel", strings.cancel, false).on_click(cx.listener(|this, _, _, cx| {
                        this.editing = None;
                        cx.notify();
                    })))
                    .child(button("edit-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save_edit(cx)))),
            )
            .into_any_element()
    }

    /// "Start Date" with a calendar, and "Goal Days": Forever, the presets or a custom number.
    fn render_span(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let start = edit.start_date;
        let calendar_view = edit.start_calendar.map(|month| {
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .child(div().flex_1().child(Label::new(calendar::month_title(month, strings)).size(LabelSize::Small)))
                        .child(IconButton::new("start-prev", IconName::ChevronLeft).size(px(22.)).on_click(cx.listener(
                            |this, _, _, cx| this.update_edit(cx, |e| e.start_calendar = e.start_calendar.map(|m| calendar::add_months(m, -1))),
                        )))
                        .child(IconButton::new("start-next", IconName::ChevronRight).size(px(22.)).on_click(cx.listener(
                            |this, _, _, cx| this.update_edit(cx, |e| e.start_calendar = e.start_calendar.map(|m| calendar::add_months(m, 1))),
                        ))),
                )
                .children(calendar::month_grid(month).chunks(7).map(|week| {
                    h_flex().children(week.iter().map(|&date| {
                        let selected = date == start;
                        div()
                            .id(SharedString::from(format!("start-day-{date}")))
                            .flex_1()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .when(selected, |el| el.bg(theme.colors().accent))
                            .when(!selected, |el| el.hover(|s| s.bg(theme.colors().element_hover)))
                            .child(Label::new(date.day().to_string()).size(LabelSize::XSmall).color(
                                if selected || date.month() == month.month() { Color::Default } else { Color::Muted },
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    e.start_date = date;
                                    e.start_calendar = None;
                                })
                            }))
                    }))
                }))
        });
        let chip = |id: SharedString, label: String, selected: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(12.))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.colors().accent.opacity(0.25)))
                .when(!selected, |el| el.bg(theme.colors().element_hover))
                .child(Label::new(label).size(LabelSize::XSmall).color(if selected { Color::Accent } else { Color::Default }))
        };
        let custom = edit.goal_days.filter(|n| !habit::GOAL_DAYS_PRESETS.contains(n));
        let start_label = if start == today {
            strings.today.to_owned()
        } else {
            format!("{}, {}", crate::task_rows::short_date(start, strings), start.year())
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Label::new(strings.habit_start_date).size(LabelSize::Small).color(Color::Muted)))
                    .child(
                        div()
                            .id("habit-start")
                            .h(px(26.))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if edit.start_calendar.is_some() { theme.colors().accent } else { theme.colors().border })
                            .cursor_pointer()
                            .child(Label::new(start_label).size(LabelSize::Small))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    e.start_calendar = if e.start_calendar.is_some() { None } else { Some(calendar::month_start(start)) }
                                })
                            })),
                    ),
            )
            .children(calendar_view)
            .child(Label::new(strings.habit_goal_days).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(chip("goal-days-forever".into(), strings.habit_forever.to_owned(), edit.goal_days.is_none()).on_click(
                        cx.listener(|this, _, _, cx| this.update_edit(cx, |e| e.goal_days = None)),
                    ))
                    .children(habit::GOAL_DAYS_PRESETS.into_iter().map(|n| {
                        chip(SharedString::from(format!("goal-days-{n}")), n.to_string(), edit.goal_days == Some(n))
                            .on_click(cx.listener(move |this, _, _, cx| this.update_edit(cx, |e| e.goal_days = Some(n))))
                    }))
                    .child(chip("goal-days-custom".into(), strings.habit_custom.to_owned(), custom.is_some()).on_click(
                        cx.listener(|this, _, _, cx| this.update_edit(cx, |e| e.goal_days = Some(50))),
                    )),
            )
            .when_some(custom, |el, days| {
                el.child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Label::new(format!("{days} {}", strings.habit_days)).size(LabelSize::Small)))
                        .child(IconButton::new("goal-days-fewer", IconName::Minus).on_click(cx.listener(|this, _, _, cx| {
                            this.update_edit(cx, |e| e.goal_days = e.goal_days.map(|n| n.saturating_sub(1).max(1)))
                        })))
                        .child(IconButton::new("goal-days-more", IconName::Plus).on_click(cx.listener(|this, _, _, cx| {
                            this.update_edit(cx, |e| e.goal_days = e.goal_days.map(|n| (n + 1).min(9999)))
                        }))),
                )
            })
            .into_any_element()
    }

    /// The icon picker: emoji or text, and the background color.
    fn render_icon_picker(&self, edit: &HabitEdit, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tabs = h_flex().p_0p5().rounded(px(6.)).bg(theme.colors().background).children(
            strings.habit_icon_tabs.iter().enumerate().map(|(i, label)| {
                div()
                    .id(SharedString::from(format!("icon-tab-{i}")))
                    .flex_1()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(i == tab, |el| el.bg(theme.colors().element_selected))
                    .child(Label::new(*label).size(LabelSize::Small))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_edit(cx, |e| e.icon_picker = Some(i))))
            }),
        );
        let body: AnyElement = if tab == 0 {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(HABIT_EMOJI.iter().map(|emoji| {
                    let selected = edit.icon.as_deref() == Some(*emoji);
                    div()
                        .id(SharedString::from(format!("emoji-{emoji}")))
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_size(px(18.))
                        .when(selected, |el| el.bg(theme.colors().element_selected))
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .child(*emoji)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_edit(cx, |e| {
                                e.icon = Some((*emoji).to_owned());
                                e.icon_picker = None;
                            })
                        }))
                }))
                .into_any_element()
        } else {
            h_flex()
                .h(px(30.))
                .px_2()
                .rounded(px(6.))
                .bg(theme.colors().background)
                .child(edit.icon_text.clone())
                .into_any_element()
        };
        let colors = h_flex().gap_1p5().children(
            std::iter::once(None).chain(crate::lists_panel::LIST_COLORS.iter().map(|c| Some(*c))).map(|color| {
                let selected = edit.color.as_deref() == color;
                let fill = crate::lists_panel::parse_color(color).unwrap_or(theme.colors().accent);
                div()
                    .id(SharedString::from(format!("habit-color-{}", color.unwrap_or("none"))))
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(fill.opacity(if color.is_none() { 0.35 } else { 1. }))
                    .when(selected, |el| el.border_2().border_color(theme.colors().text))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_edit(cx, |e| e.color = color.map(str::to_owned))))
            }),
        );
        v_flex()
            .gap_2()
            .p_2()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().border)
            .child(tabs)
            .child(body)
            .child(colors)
            .into_any_element()
    }

    /// The original's Frequency menu: daily on chosen weekdays, N days a week, or every N days.
    fn render_frequency(&self, frequency: HabitFrequency, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let kind = match frequency {
            HabitFrequency::Daily { .. } => 0,
            HabitFrequency::Weekly { .. } => 1,
            HabitFrequency::Interval { .. } => 2,
        };
        let circle = |id: SharedString, label: SharedString, on: bool| {
            div()
                .id(id)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.colors().accent))
                .when(!on, |el| el.bg(theme.colors().element_hover))
                .child(Label::new(label).size(LabelSize::XSmall))
        };
        let tabs = h_flex().gap_1().children(strings.habit_frequency_kinds.iter().enumerate().map(|(i, label)| {
            div()
                .id(SharedString::from(format!("frequency-kind-{i}")))
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(12.))
                .cursor_pointer()
                .when(i == kind, |el| el.bg(theme.colors().accent.opacity(0.25)))
                .when(i != kind, |el| el.bg(theme.colors().element_hover))
                .child(Label::new(*label).size(LabelSize::XSmall).color(if i == kind { Color::Accent } else { Color::Default }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_edit(cx, |e| {
                        e.frequency = match i {
                            0 => HabitFrequency::default(),
                            1 => HabitFrequency::Weekly { days: 3 },
                            _ => HabitFrequency::Interval { days: 2 },
                        }
                    })
                }))
        }));
        let detail: AnyElement = match frequency {
            HabitFrequency::Daily { weekdays } => h_flex()
                .justify_between()
                .children((0..7u8).map(|i| {
                    circle(
                        SharedString::from(format!("frequency-day-{i}")),
                        strings.weekdays_min[i as usize].into(),
                        weekdays & (1 << i) != 0,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_edit(cx, |e| {
                            if let HabitFrequency::Daily { weekdays } = &mut e.frequency {
                                // At least one day stays selected.
                                let toggled = *weekdays ^ (1 << i);
                                if toggled != 0 {
                                    *weekdays = toggled;
                                }
                            }
                        })
                    }))
                }))
                .into_any_element(),
            HabitFrequency::Weekly { days } => v_flex()
                .gap_1()
                .child(Label::new(strings.habit_days_per_week.replace("{n}", &days.to_string())).size(LabelSize::XSmall).color(Color::Muted))
                .child(h_flex().justify_between().children((1..=7u8).map(|n| {
                    circle(SharedString::from(format!("frequency-week-{n}")), n.to_string().into(), n == days).on_click(
                        cx.listener(move |this, _, _, cx| this.update_edit(cx, |e| e.frequency = HabitFrequency::Weekly { days: n })),
                    )
                })))
                .into_any_element(),
            HabitFrequency::Interval { days } => h_flex()
                .gap_2()
                .child(div().flex_1().child(Label::new(strings.habit_every_n_days.replace("{n}", &days.to_string())).size(LabelSize::Small)))
                .child(IconButton::new("interval-fewer", IconName::Minus).on_click(cx.listener(|this, _, _, cx| {
                    this.update_edit(cx, |e| {
                        if let HabitFrequency::Interval { days } = &mut e.frequency {
                            *days = days.saturating_sub(1).max(2);
                        }
                    })
                })))
                .child(IconButton::new("interval-more", IconName::Plus).on_click(cx.listener(|this, _, _, cx| {
                    this.update_edit(cx, |e| {
                        if let HabitFrequency::Interval { days } = &mut e.frequency {
                            *days = (*days + 1).min(365);
                        }
                    })
                })))
                .into_any_element(),
        };
        v_flex().gap_2().child(tabs).child(detail).into_any_element()
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.month = calendar::add_months(self.month, delta);
        cx.notify();
    }

    /// `goal=<amount> <unit>` and `note=<text>` for the selected habit, `edit` opens the editor.
    #[cfg(debug_assertions)]
    pub fn dev_apply(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let name = self.habits.iter().find(|h| h.id == id).map(|h| h.name.clone()).unwrap_or_default();
        if let Some(goal) = command.strip_prefix("goal=") {
            let (amount, unit) = goal.split_once(' ').unwrap_or((goal, ""));
            let goal = amount.parse().ok().map(|amount| Goal { amount, unit: unit.to_owned() });
            self.store.update_habit(id, &name, goal.as_ref()).ok();
        } else if let Some(note) = command.strip_prefix("note=") {
            let (days_ago, note) = note.split_once('>').unwrap_or(("0", note));
            let date = Local::now().date_naive() - chrono::Duration::days(days_ago.parse().unwrap_or(0));
            self.store.set_checkin_note(id, date, note).ok();
        } else if let Some(step) = command.strip_prefix("step=") {
            // `step=<days ago>x<times>`: check in an amount, one unit at a time.
            let (ago, times) = step.split_once('x').unwrap_or((step, "1"));
            let date = Local::now().date_naive() - chrono::Duration::days(ago.parse().unwrap_or(0));
            for _ in 0..times.parse().unwrap_or(1) {
                self.store.step_checkin(id, date).ok();
            }
        } else if let Some(times) = command.strip_prefix("remind=") {
            let times: Vec<chrono::NaiveTime> = times.split(',').filter_map(crate::due_picker::parse_time).collect();
            self.store.set_habit_reminders(id, &times).ok();
        } else if let Some(frequency) = command.strip_prefix("freq=") {
            self.store.set_habit_frequency(id, HabitFrequency::from_storage(frequency)).ok();
        } else if let Some(look) = command.strip_prefix("look=") {
            let (icon, color) = look.split_once('|').unwrap_or((look, ""));
            self.store.set_habit_look(id, Some(icon), (!color.is_empty()).then_some(color)).ok();
        } else if let Some(span) = command.strip_prefix("span=") {
            // `span=<start days ago>,<goal days or 0>`.
            let (ago, days) = span.split_once(',').unwrap_or((span, "0"));
            let start = Local::now().date_naive() - chrono::Duration::days(ago.parse().unwrap_or(0));
            self.store.set_habit_span(id, Some(start), days.parse().ok()).ok();
        } else if let Some(section) = command.strip_prefix("section=") {
            let section = match section.strip_prefix("new:") {
                Some(name) => self.store.add_habit_section(name).map(|s| HabitSection::Custom(s.id)).unwrap_or_default(),
                None => HabitSection::from_key(section),
            };
            self.store.set_habit_section(id, section).ok();
        } else if command == "archive" {
            self.store.set_habit_archived(id, true).ok();
        } else if command == "settings" {
            self.header_more = Some(HeaderMore::Settings);
            return;
        } else if command == "list" {
            self.set_list_layout(true, cx);
            return;
        } else if command == "show-archived" {
            self.show_archived = true;
        } else if command == "picker" {
            self.update_edit(cx, |e| e.icon_picker = Some(0));
            return;
        } else if command == "edit" {
            self.reload(cx);
            self.start_edit(window, cx);
            return;
        }
        self.reload(cx);
    }

    #[cfg(debug_assertions)]
    pub fn dev_select(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.habits.iter().find(|h| h.name == name).map(|h| h.id) {
            self.select(id, cx);
        }
    }

    /// A click on a check-in dot: toggles the day, or for a habit with an amount goal adds one
    /// unit (a completed day starts over).
    fn toggle(&mut self, habit: Id, date: NaiveDate, cx: &mut Context<Self>) {
        let has_goal = self.habits.iter().any(|h| h.id == habit && h.goal.is_some());
        let result =
            if has_goal { self.store.step_checkin(habit, date).map(|_| ()) } else { self.store.toggle_checkin(habit, date).map(|_| ()) };
        if let Err(err) = result {
            eprintln!("failed to check in: {err:#}");
        }
        self.reload(cx);
    }

    /// The amount checked in on a day.
    fn amount(&self, habit: Id, date: NaiveDate) -> u32 {
        self.records.get(&habit).and_then(|r| r.iter().find(|c| c.date == date)).map_or(0, |c| c.value)
    }

    /// A check-in dot: filled when the day is done, lighter for a partial amount.
    fn dot_color(&self, habit: Id, date: NaiveDate, cx: &App) -> Hsla {
        let colors = cx.theme().colors();
        let empty = HashSet::new();
        let done = self.checkins.get(&habit).unwrap_or(&empty);
        let due = self.habits.iter().find(|h| h.id == habit).is_none_or(|h| h.is_due(date, done));
        if done.contains(&date) {
            colors.accent
        } else if self.amount(habit, date) > 0 {
            colors.accent.opacity(0.4)
        } else if !due {
            // Not scheduled that day, or before the habit starts.
            colors.element_hover.opacity(0.35)
        } else {
            colors.element_hover
        }
    }

    /// TickTick's "Daily Goals" chart: the amount per day over the last days, with the goal line.
    fn render_goal_chart(&self, habit: &Habit, goal: u32, today: NaiveDate, cx: &App) -> AnyElement {
        const DAYS: i64 = 11;
        const HEIGHT: f32 = 110.;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let days: Vec<(NaiveDate, u32)> = (0..DAYS)
            .rev()
            .map(|ago| today - chrono::Duration::days(ago))
            .map(|date| (date, self.amount(habit.id, date)))
            .collect();
        let top = days.iter().map(|(_, v)| *v).max().unwrap_or(0).max(goal).max(1) as f32;
        let unit = habit.goal.as_ref().map(|g| g.unit.clone()).unwrap_or_default();
        v_flex()
            .p_3()
            .gap_2()
            .rounded(px(8.))
            .bg(colors.surface_background)
            .child(
                h_flex()
                    .gap_1()
                    .child(Label::new(strings.habit_daily_goals).weight(FontWeight::SEMIBOLD))
                    .when(!unit.is_empty(), |el| el.child(Label::new(format!("({unit})")).size(LabelSize::Small).color(Color::Muted))),
            )
            .child(
                div()
                    .relative()
                    .h(px(HEIGHT))
                    .child(
                        h_flex().absolute().inset_0().items_end().gap_1().children(days.iter().map(|(date, value)| {
                            let done = *value >= goal;
                            div()
                                .flex_1()
                                .h(px(HEIGHT * *value as f32 / top))
                                .rounded_t(px(3.))
                                .bg(if done { colors.accent } else { colors.accent.opacity(0.4) })
                                .when(*date == today, |el| el.border_1().border_color(colors.text_muted))
                        })),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom(px(HEIGHT * goal as f32 / top))
                            .h(px(1.))
                            .bg(colors.accent),
                    ),
            )
            .child(h_flex().gap_1().children(days.iter().map(|(date, _)| {
                div().flex_1().flex().justify_center().child(
                    Label::new(date.day().to_string())
                        .size(LabelSize::XSmall)
                        .color(if *date == today { Color::Accent } else { Color::Muted }),
                )
            })))
            .into_any_element()
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
        let active: Vec<&Habit> = self.habits.iter().filter(|h| !h.archived).collect();
        if active.is_empty() {
            return 0.;
        }
        let done = active.iter().filter(|h| self.checkins.get(&h.id).is_some_and(|c| c.contains(&date))).count();
        done as f32 / active.len() as f32
    }

    /// A section's name: the default ones are translated, custom ones are the user's.
    fn section_label(&self, section: HabitSection, cx: &App) -> String {
        let strings = i18n::t(cx);
        match section {
            HabitSection::Morning => strings.habit_sections[0].to_owned(),
            HabitSection::Afternoon => strings.habit_sections[1].to_owned(),
            HabitSection::Night => strings.habit_sections[2].to_owned(),
            HabitSection::Others => strings.habit_sections[3].to_owned(),
            HabitSection::Custom(id) => {
                self.sections.iter().find(|s| s.id == id).map_or_else(|| strings.habit_sections[3].to_owned(), |s| s.name.clone())
            }
        }
    }

    fn start_new_section(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_section_name, cx));
        cx.subscribe(&input, |this, _, Submitted(name), cx| match this.store.add_habit_section(name) {
            Ok(section) => {
                this.sections = this.store.habit_sections().unwrap_or_default();
                this.update_edit(cx, |e| {
                    e.section = HabitSection::Custom(section.id);
                    e.new_section = None;
                });
            }
            Err(err) => eprintln!("failed to add the section: {err:#}"),
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.update_edit(cx, |e| e.new_section = Some(input));
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        self.store.setting(key).ok().flatten().map_or(default, |v| v == "1")
    }

    fn set_flag(&mut self, key: &str, value: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting(key, if value { "1" } else { "0" }) {
            eprintln!("failed to save the setting: {err:#}");
        }
        cx.notify();
    }

    /// Writes habits and check-ins as CSV to the downloads folder.
    fn export(&mut self, cx: &mut Context<Self>) {
        self.header_more = None;
        let strings = i18n::t(cx);
        let data: Vec<(Habit, Vec<Checkin>)> = self
            .habits
            .iter()
            .map(|h| (h.clone(), self.records.get(&h.id).cloned().unwrap_or_default()))
            .collect();
        let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| ".".into());
        let path = dir.join(format!("fern-habits-{}.csv", Local::now().format("%Y-%m-%d")));
        self.export_message = Some(match std::fs::write(&path, habit::export_csv(&data)) {
            Ok(()) => strings.exported_to.replace("{path}", &path.display().to_string()),
            Err(err) => format!("{err}"),
        });
        cx.notify();
    }

    fn render_header_more(&self, more: HeaderMore, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let close = cx.listener(|this, _, _, cx| {
            this.header_more = None;
            cx.notify();
        });
        match more {
            HeaderMore::Menu => popover(cx)
                .w(px(180.))
                .on_mouse_down_out(close)
                .child(
                    ListItem::new("habit-settings")
                        .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                        .child(Label::new(strings.settings))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.header_more = Some(HeaderMore::Settings);
                            cx.notify();
                        })),
                )
                .child(
                    ListItem::new("habit-export")
                        .start_slot(Icon::new(IconName::Share).color(Color::Muted))
                        .child(Label::new(strings.export))
                        .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                )
                .into_any_element(),
            HeaderMore::Settings => {
                let in_today = self.flag(IN_TODAY_SETTING, true);
                let by_status = self.flag(SORT_SETTING, false);
                popover(cx)
                    .w(px(300.))
                    .p_3()
                    .gap_3()
                    .on_mouse_down_out(close)
                    .child(Label::new(strings.habit_settings).weight(FontWeight::SEMIBOLD))
                    .child(
                        h_flex()
                            .id("setting-in-today")
                            .gap_2()
                            .cursor_pointer()
                            .child(div().flex_1().child(Label::new(strings.habits_in_today).size(LabelSize::Small)))
                            .child(Switch::new().toggle_state(in_today))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_flag(IN_TODAY_SETTING, !in_today, cx))),
                    )
                    .child(
                        h_flex()
                            .id("setting-sort")
                            .gap_2()
                            .items_start()
                            .cursor_pointer()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(Label::new(strings.habits_sort_by_status).size(LabelSize::Small))
                                    .child(
                                        div()
                                            .w_full()
                                            .text_size(px(11.))
                                            .text_color(theme.colors().text_muted)
                                            .child(strings.habits_sort_hint),
                                    ),
                            )
                            .child(Switch::new().toggle_state(by_status))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_flag(SORT_SETTING, !by_status, cx))),
                    )
                    .into_any_element()
            }
        }
    }

    fn set_list_layout(&mut self, list: bool, cx: &mut Context<Self>) {
        self.list_layout = list;
        if let Err(err) = self.store.set_setting(LAYOUT_SETTING, if list { "list" } else { "grid" }) {
            eprintln!("failed to save the habit layout: {err:#}");
        }
        cx.notify();
    }

    fn set_archived(&mut self, habit: Id, archived: bool, cx: &mut Context<Self>) {
        self.detail_menu_open = false;
        if let Err(err) = self.store.set_habit_archived(habit, archived) {
            eprintln!("failed to archive the habit: {err:#}");
        }
        self.selected = None;
        self.reload(cx);
    }

    fn render_card(&self, habit: &Habit, week: [NaiveDate; 7], today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let empty = HashSet::new();
        let checked = self.checkins.get(&id).unwrap_or(&empty);
        let stats = habit::stats_for(&habit.frequency, checked, today);
        let group = SharedString::from(format!("habit-{id}"));
        let list = self.list_layout;

        div()
            .id(group)
            .relative()
            .flex()
            .map(|el| {
                if list {
                    el.flex_row().items_center().w_full().h(px(72.)).gap_4()
                } else {
                    el.flex_col().w(px(CARD_WIDTH)).h(px(CARD_HEIGHT)).gap_2()
                }
            })
            .flex_none()
            .p_3()
            .rounded(px(8.))
            .border_1()
            .border_color(if self.selected == Some(id) { theme.colors().accent } else { gpui::transparent_black() })
            .bg(theme.colors().surface_background)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .when(list, |el| el.flex_1().min_w_0())
                    .child(
                        avatar(habit, px(30.), cx),
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
                                                Label::new(match habit.goal_days {
                                                    Some(goal) => format!("{}/{goal} {}", stats.total_days, strings.habit_days),
                                                    None => format!("{} {}", stats.total_days, strings.habit_days),
                                                })
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
            .child(h_flex().justify_between().px_1().when(list, |el| el.w(px(260.)).flex_none()).children(week.map(|date| {
                div()
                    .id(SharedString::from(format!("checkin-{id}-{date}")))
                    .size(px(16.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(self.dot_color(id, date, cx))
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
        let stats = habit::stats_for(&habit.frequency, checked, today);
        let month = habit::month_stats_for(&habit.frequency, checked, self.month, today);
        let status = theme.status().clone();
        let no_records = Vec::new();
        let records = self.records.get(&id).unwrap_or(&no_records);
        let (monthly_done, total_done) = habit::completion(records, self.month);
        let in_month = |date: NaiveDate| date.year() == self.month.year() && date.month() == self.month.month();
        let notes: Vec<(NaiveDate, String)> = records
            .iter()
            .rev()
            .filter(|c| in_month(c.date))
            .filter_map(|c| c.note.clone().map(|note| (c.date, note)))
            .collect();
        let checked_today = checked.contains(&today);
        let goal_unit: SharedString = habit.goal.as_ref().map_or(SharedString::default(), |g| g.unit.clone().into());

        let card = |icon: IconName, color: Hsla, label: &'static str, value: String, unit: SharedString| {
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

        let weekdays = h_flex().children(crate::calendar::weekday_labels(&strings.weekdays_short).iter().map(|day| {
            div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
        }));
        let grid: Vec<AnyElement> = calendar::month_grid(self.month)
            .chunks(7)
            .map(|week| {
                h_flex()
                    .children(week.iter().map(|&date| {
                        let in_month = date.month() == self.month.month();
                        let dot = self.dot_color(id, date, cx);
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
                                    .bg(dot)
                                    .when(!future, |el| {
                                        el.cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, date, cx)))
                                    }),
                            )
                    }))
                    .into_any_element()
            })
            .collect();

        v_flex()
            .id("habit-detail")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .gap_2()
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_none()
                    .gap_2()
                    .child(
                        avatar(habit, px(30.), cx),
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
                                            ListItem::new("habit-edit")
                                                .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                                .child(Label::new(strings.edit))
                                                .on_click(cx.listener(|this, _, window, cx| this.start_edit(window, cx))),
                                        )
                                        .child({
                                            let archived = habit.archived;
                                            ListItem::new("habit-archive")
                                                .start_slot(Icon::new(IconName::Archive).color(Color::Muted))
                                                .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                                                .on_click(cx.listener(move |this, _, _, cx| this.set_archived(id, !archived, cx)))
                                        })
                                        .child(
                                            ListItem::new("habit-delete")
                                                .start_slot(Icon::new(IconName::Trash).color(Color::Muted))
                                                .child(Label::new(strings.delete))
                                                .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
                                        ),
                                ))
                            })
                            .when_some(self.editing.as_ref(), |el, edit| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_edit(edit, cx)))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(IconName::CircleCheck, status.success, strings.habit_monthly_checkins, month.days.to_string(), strings.habit_unit_days.into()))
                    .child(card(IconName::Zap, theme.colors().accent, strings.habit_total_checkins, stats.total_days.to_string(), strings.habit_unit_days.into())),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(IconName::Percent, status.warning, strings.habit_monthly_rate, month.rate.to_string(), "%".into()))
                    .child(card(IconName::Flame, status.error, strings.habit_current_streak, stats.streak.to_string(), strings.habit_unit_days.into())),
            )
            .when(habit.goal.is_some(), |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .child(card(IconName::ChartNoAxesColumn, theme.colors().accent, strings.habit_monthly_completion, monthly_done.to_string(), goal_unit.clone()))
                        .child(card(IconName::ChartNoAxesColumn, theme.colors().accent, strings.habit_total_completion, total_done.to_string(), goal_unit.clone())),
                )
            })
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
            .when_some(habit.goal.as_ref().map(|g| g.amount), |el, goal| el.child(self.render_goal_chart(habit, goal, today, cx)))
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .rounded(px(8.))
                    .bg(theme.colors().surface_background)
                    .child(
                        Label::new(format!("{}{}", strings.habit_log_on, strings.months[self.month.month0() as usize]))
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .when(notes.is_empty(), |el| {
                        el.child(Label::new(strings.habit_log_empty).size(LabelSize::Small).color(Color::Muted))
                    })
                    .children(notes.into_iter().map(|(date, note)| {
                        v_flex()
                            .child(
                                Label::new(crate::task_rows::day_header(date, today, strings))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(note).size(LabelSize::Small))
                    }))
                    .when(checked_today && in_month(today), |el| {
                        el.child(
                            h_flex()
                                .h(px(30.))
                                .px_2()
                                .rounded(px(6.))
                                .bg(theme.colors().background)
                                .text_size(px(13.))
                                .child(self.note_input.clone()),
                        )
                    }),
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
        // Cards of the chosen side (active or archived), grouped by section once sections are used.
        let visible: Vec<&Habit> = habits.iter().filter(|h| h.archived == self.show_archived).collect();
        let grouped = visible.iter().any(|h| h.section != HabitSection::Others);
        let mut cards: Vec<AnyElement> = Vec::new();
        for (i, section) in HabitSection::ordered(&self.sections).into_iter().enumerate() {
            let mut in_section: Vec<&Habit> =
                visible.iter().copied().filter(|h| !grouped || h.section == section).collect();
            if self.flag(SORT_SETTING, false) {
                // Unchecked habits first, keeping their order otherwise.
                in_section.sort_by_key(|h| self.checkins.get(&h.id).is_some_and(|c| c.contains(&today)));
            }
            if in_section.is_empty() || (!grouped && i > 0) {
                continue;
            }
            let title = self.section_label(section, cx);
            let section_cards: Vec<AnyElement> = in_section.iter().map(|h| self.render_card(h, week, today, cx)).collect();
            cards.push(
                v_flex()
                    .w_full()
                    .gap_2()
                    .when(grouped, |el| {
                        el.child(
                            h_flex()
                                .gap_1()
                                .pt_2()
                                .child(Label::new(title).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                                .child(Label::new(in_section.len().to_string()).size(LabelSize::XSmall).color(Color::Muted)),
                        )
                    })
                    .child(div().flex().flex_wrap().gap(px(12.)).children(section_cards))
                    .into_any_element(),
            );
        }
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
                    .child(
                        h_flex()
                            .id("habit-title")
                            .relative()
                            .gap_1()
                            .cursor_pointer()
                            .child(
                                Label::new(if self.show_archived { strings.habit_archived } else { strings.section_habits })
                                    .size(LabelSize::Large)
                                    .weight(FontWeight::BOLD),
                            )
                            .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.header_menu_open = true;
                                cx.notify();
                            }))
                            .when(self.header_menu_open, |el| {
                                let archived = self.show_archived;
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    popover(cx)
                                        .w(px(180.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu_open = false;
                                            cx.notify();
                                        }))
                                        .children([(false, strings.habit_active), (true, strings.habit_archived)].map(|(value, label)| {
                                            ListItem::new(if value { "habits-archived" } else { "habits-active" })
                                                .child(Label::new(label).color(if value == archived { Color::Accent } else { Color::Default }))
                                                .when(value == archived, |item| item.end_slot(Label::new("✓").color(Color::Accent)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.show_archived = value;
                                                    this.header_menu_open = false;
                                                    this.selected = None;
                                                    cx.notify();
                                                }))
                                        })),
                                ))
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        IconButton::new("habit-list", if self.list_layout { IconName::LayoutGrid } else { IconName::List })
                            .on_click(cx.listener(|this, _, _, cx| this.set_list_layout(!this.list_layout, cx))),
                    )
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
                    .child(
                        div()
                            .relative()
                            .child(IconButton::new("habit-more", IconName::Ellipsis).on_click(cx.listener(|this, _, _, cx| {
                                this.header_more = Some(HeaderMore::Menu);
                                cx.notify();
                            })))
                            .when_some(self.header_more, |el, more| {
                                el.child(PopoverAttachment::new(Placement::BelowRight, self.render_header_more(more, cx)))
                            }),
                    ),
            )
            .when_some(self.export_message.clone(), |el, message| {
                el.child(div().px(px(22.)).pb_2().child(Label::new(message).size(LabelSize::XSmall).color(Color::Muted)))
            })
            .child(strip)
            .child(
                div()
                    .id("habit-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(22.))
                    .when(cards.is_empty(), |el| {
                        el.child(div().pt_8().child(
                            Label::new(if self.show_archived { strings.habit_no_archived } else { strings.habit_empty }).color(Color::Muted),
                        ))
                    })
                    .child(v_flex().gap_2().children(cards)),
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
