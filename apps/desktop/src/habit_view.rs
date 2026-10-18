//! The Habit section, matched to TickTick's: a week strip with daily progress rings and habit
//! cards with totals, streaks and this week's check-ins; the selected habit's statistics and
//! month calendar on the right.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Checkin, CustomSection, Goal, Habit, HabitFrequency, HabitSection, Id, Store, habit};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::calendar;
use crate::i18n;
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

const CARD_WIDTH: f32 = 365.;

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
    let bg = crate::lists_panel::parse_color(color).unwrap_or(theme.colors().accent).opacity(0.35);
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

/// The items of the details' "…" menu that open a choice, as in the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailSubmenu {
    StartFocus,
    CheckinStyle,
}

/// "Start Focus" on a habit: a Pomodoro or a stopwatch named after it.
pub struct StartFocus {
    pub title: String,
    pub stopwatch: bool,
}

impl gpui::EventEmitter<StartFocus> for HabitView {}

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
    /// The row whose dropdown is open.
    open: Option<EditRow>,
    /// The time field after the reminders, shown by their "+" as in the original.
    adding_reminder: bool,
    constant_reminder: bool,
}

/// The Edit Habit rows that open a dropdown, as in the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditRow {
    Frequency,
    Goal,
    StartDate,
    GoalDays,
    Section,
}

/// The Frequency field's text: "Daily", the chosen weekdays, "3 days per week" or "Every 2 days".
fn frequency_label(frequency: HabitFrequency, strings: &i18n::Strings) -> String {
    match frequency {
        HabitFrequency::Daily { weekdays } if weekdays == HabitFrequency::EVERY_DAY => strings.habit_frequency_kinds[0].to_owned(),
        HabitFrequency::Daily { weekdays } => (0..7)
            .filter(|i| weekdays & (1 << i) != 0)
            .map(|i| strings.weekdays_short[i])
            .collect::<Vec<_>>()
            .join(", "),
        HabitFrequency::Weekly { days } => strings.habit_days_per_week.replace("{n}", &days.to_string()),
        HabitFrequency::Interval { days } => strings.habit_every_n_days.replace("{n}", &days.to_string()),
    }
}

pub struct HabitView {
    store: Rc<Store>,
    habits: Vec<Habit>,
    checkins: HashMap<Id, HashSet<NaiveDate>>,
    /// Check-ins with amounts and notes, for the details panel.
    records: HashMap<Id, Vec<Checkin>>,
    /// The note field under the habit log, for today's check-in.
    note_input: Entity<TextInput>,
    /// Today's amount of a goal habit, typed in the details panel.
    amount_input: Entity<TextInput>,
    editing: Option<HabitEdit>,
    new_habit: Option<Entity<TextInput>>,
    /// The habit shown in the details panel.
    selected: Option<Id>,
    /// The month shown in the details panel's calendar.
    month: NaiveDate,
    detail_menu_open: bool,
    /// The expanded item of the details' "…" menu.
    detail_submenu: Option<DetailSubmenu>,
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
    /// The custom section whose header "…" menu is open.
    section_menu: Option<Id>,
    /// A custom section being renamed in its header.
    renaming_section: Option<(Id, Entity<TextInput>)>,
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
        let amount_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "0", cx));
        cx.subscribe(&amount_input, |this, _, _: &Submitted, cx| this.record_amount(cx)).detach();
        let mut this = Self {
            store,
            amount_input,
            habits: Vec::new(),
            checkins: HashMap::new(),
            records: HashMap::new(),
            note_input,
            editing: None,
            new_habit: None,
            selected: None,
            month: calendar::month_start(Local::now().date_naive()),
            detail_menu_open: false,
            detail_submenu: None,
            show_archived: false,
            header_menu_open: false,
            list_layout: false,
            sections: Vec::new(),
            section_menu: None,
            renaming_section: None,
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

    /// Saves the typed amount for today: the original's check-in with a number.
    fn record_amount(&mut self, cx: &mut Context<Self>) {
        let Some(habit) = self.selected else { return };
        let text = self.amount_input.read(cx).text().trim().to_owned();
        let Ok(amount) = text.parse::<u32>() else { return };
        if let Err(err) = self.store.set_checkin_amount(habit, Local::now().date_naive(), amount) {
            eprintln!("failed to record the amount: {err:#}");
        }
        self.amount_input.update(cx, |input, cx| input.set_text("", cx));
        self.reload(cx);
    }

    /// "Record" in a reminder: the habit's details with today's amount field focused.
    pub fn record(&mut self, habit: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.select(habit, cx);
        let focus = self.amount_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
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
                    e.adding_reminder = false;
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
            open: None,
            adding_reminder: false,
            constant_reminder: habit.constant_reminder,
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
            .and_then(|()| self.store.set_habit_constant_reminder(id, edit.constant_reminder))
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
        let picker = edit.icon_picker.map(|tab| self.render_icon_picker(edit, tab, cx));
        let frequency_menu = self.render_frequency(edit.frequency, cx);
        let (start_text, start_menu, goal_days_text, goal_days_menu) = self.render_span(edit, cx);
        let section_menu = v_flex()
            .gap_2()
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
            .into_any_element();
        let goal_menu = v_flex()
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
            .into_any_element();
        let goal_text = match amount {
            None => strings.habit_goal_all.to_owned(),
            Some(n) => format!("{n} {}{}", edit.unit.read(cx).text(), strings.habit_per_day),
        };
        let section_text = self.section_label(edit.section, cx).to_string();
        // The original's rows: a label on the left and the value in a field that opens its choices.
        let row_with = |label: AnyElement, content: AnyElement| {
            // With the dialog's 8 px gap, a row every 40 px as in the original.
            h_flex()
                .min_h(px(32.))
                .gap_2()
                .child(div().w(px(101.)).flex_none().child(label))
                .child(div().flex_1().min_w_0().child(content))
        };
        let row = |label: &'static str, content: AnyElement| row_with(Label::new(label).into_any_element(), content);
        // The original's "?" beside Goal Days, explaining it on hover.
        let goal_days_label = h_flex()
            .gap_1()
            .child(Label::new(strings.habit_goal_days))
            .child(
                div()
                    .id("habit-goal-days-hint")
                    .child(Icon::new(IconName::CircleQuestionMark).size(IconSize::XSmall).color(Color::Muted))
                    .tooltip(ui::Tooltip::text(strings.habit_goal_days_hint)),
            )
            .into_any_element();
        let open = edit.open;
        let dropdown = |which: EditRow, text: String, menu: AnyElement| -> AnyElement {
            let on = open == Some(which);
            div()
                .relative()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("habit-row-{which:?}")))
                        .h(px(32.))
                        .px_3()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if on { theme.colors().accent } else { theme.colors().border })
                        .cursor_pointer()
                        .child(div().flex_1().min_w_0().child(Label::new(text).truncate()))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_edit(cx, |e| e.open = if e.open == Some(which) { None } else { Some(which) })
                        })),
                )
                .when(on, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .w(px(294.))
                            .p_3()
                            .gap_2()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update_edit(cx, |e| e.open = None)))
                            .child(menu),
                    ))
                })
                .into_any_element()
        };

        // The original's dialog: in the middle of the view, its width, closed with × or Cancel.
        popover(cx)
            .w(px(425.))
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .child(div().flex_1().child(Label::new(strings.edit_habit).weight(FontWeight::BOLD)))
                    .child(IconButton::new("habit-edit-x", IconName::X).icon_color(Color::Muted).on_click(cx.listener(|this, _, _, cx| {
                        this.editing = None;
                        cx.notify();
                    }))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .relative()
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
                            // As in the original, the picker hangs under the icon over the form.
                            .children(picker.map(|picker| PopoverAttachment::new(Placement::BelowLeft, picker))),
                    )
                    .child(div().flex_1().child(field(edit.name.clone().into_any_element()))),
            )
            .child(row(strings.habit_frequency, dropdown(EditRow::Frequency, frequency_label(edit.frequency, strings), frequency_menu)))
            .child(row(strings.habit_goal, dropdown(EditRow::Goal, goal_text, goal_menu)))
            .child(row(strings.habit_start_date, dropdown(EditRow::StartDate, start_text, start_menu)))
            .child(row_with(goal_days_label, dropdown(EditRow::GoalDays, goal_days_text, goal_days_menu)))
            .child(row(strings.habit_section, dropdown(EditRow::Section, section_text, section_menu)))
            .child(row(
                strings.reminder,
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(edit.reminders.iter().map(|&time| {
                        // The original's time chips: bordered, 32 px high.
                        h_flex()
                            .id(SharedString::from(format!("habit-reminder-{time}")))
                            .h(px(32.))
                            .pl_2()
                            .gap_0p5()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
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
                    .when(edit.adding_reminder || edit.reminders.is_empty(), |el| {
                        el.child(div().flex_1().min_w(px(120.)).child(field(edit.reminder_input.clone().into_any_element())))
                    })
                    .when(!edit.adding_reminder && !edit.reminders.is_empty(), |el| {
                        el.child(
                            div()
                                .id("habit-reminder-add")
                                .size(px(32.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.colors().border)
                                .cursor_pointer()
                                .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let input = this.editing.as_ref().map(|e| e.reminder_input.clone());
                                    this.update_edit(cx, |e| e.adding_reminder = true);
                                    if let Some(input) = input {
                                        window.focus(&input.focus_handle(cx), cx);
                                    }
                                })),
                        )
                    })
                    .into_any_element(),
            ))
            .child(row(
                strings.habit_constant_reminder,
                h_flex()
                    .id("habit-constant-reminder")
                    .cursor_pointer()
                    .child(Switch::new().toggle_state(edit.constant_reminder))
                    .on_click(cx.listener(|this, _, _, cx| this.update_edit(cx, |e| e.constant_reminder = !e.constant_reminder)))
                    .into_any_element(),
            ))
            // The original's footer: a divider, then Cancel and Save on the right.
            .child(div().mt_2().h(px(1.)).bg(theme.colors().border))
            .child(
                h_flex()
                    .gap(px(10.))
                    .pt_2()
                    .justify_end()
                    .child(button("edit-cancel", strings.cancel, false).w(px(100.)).flex_none().on_click(cx.listener(|this, _, _, cx| {
                        this.editing = None;
                        cx.notify();
                    })))
                    .child(button("edit-save", strings.save, true).w(px(100.)).flex_none().on_click(cx.listener(|this, _, _, cx| this.save_edit(cx)))),
            )
            .into_any_element()
    }

    /// "Start Date" with a calendar, and "Goal Days": Forever, the presets or a custom number.
    /// The Start Date and Goal Days rows' texts and dropdowns: a month to pick the start in, and
    /// the original's Goal Days choices.
    fn render_span(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> (String, AnyElement, String, AnyElement) {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let start = edit.start_date;
        let calendar_view = Some(edit.start_calendar.unwrap_or(calendar::month_start(start))).map(|month| {
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
                                    e.open = None;
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
        let goal_menu = v_flex()
            .gap_2()
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
            .into_any_element();
        let goal_days_text = match edit.goal_days {
            None => strings.habit_forever.to_owned(),
            Some(n) => format!("{n} {}", strings.habit_days),
        };
        (start_label, v_flex().children(calendar_view).into_any_element(), goal_days_text, goal_menu)
    }

    /// The icon picker, as the original's: Icon, Emoji and Text tabs on top; Icon is a grid of
    /// seven colored circles a row (emoji on the palette's colors stand in for the original's
    /// artwork, and picking one takes its color too), Emoji the plain emoji with a background
    /// color, Text a short text on that color.
    fn render_icon_picker(&self, edit: &HabitEdit, tab: usize, cx: &mut Context<Self>) -> AnyElement {
        const COLORS: [&str; 8] = crate::lists_panel::LIST_COLORS;
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let tabs = h_flex()
            .w(px(242.))
            .h(px(28.))
            .p_0p5()
            .rounded(px(6.))
            .bg(theme.colors().element_hover)
            .children(strings.habit_icon_tabs.iter().enumerate().map(|(i, label)| {
                div()
                    .id(SharedString::from(format!("icon-tab-{i}")))
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(i == tab, |el| el.bg(theme.colors().element_selected))
                    .child(Label::new(*label).size(LabelSize::Small).color(if i == tab { Color::Default } else { Color::Muted }))
                    .on_click(cx.listener(move |this, _, _, cx| this.update_edit(cx, |e| e.icon_picker = Some(i))))
            }));
        let colors = h_flex().gap_1p5().children(std::iter::once(None).chain(COLORS.iter().map(|c| Some(*c))).map(|color| {
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
        }));
        let body: AnyElement = match tab {
            0 => div()
                .id("habit-icon-grid")
                .h(px(225.))
                .overflow_y_scroll()
                .child(div().flex().flex_wrap().gap_y(px(15.)).children(HABIT_EMOJI.iter().enumerate().map(|(i, emoji)| {
                    let color = COLORS[i % COLORS.len()];
                    let selected = edit.icon.as_deref() == Some(*emoji) && edit.color.as_deref() == Some(color);
                    div()
                        .id(SharedString::from(format!("icon-{emoji}")))
                        .w(px(45.))
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(crate::lists_panel::parse_color(Some(color)).unwrap_or(theme.colors().accent).opacity(0.45))
                                .when(selected, |el| el.border_2().border_color(theme.colors().accent))
                                .text_size(px(16.))
                                .child(*emoji),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_edit(cx, |e| {
                                e.icon = Some((*emoji).to_owned());
                                e.color = Some(color.to_owned());
                                e.icon_picker = None;
                            })
                        }))
                })))
                .into_any_element(),
            1 => v_flex()
                .gap_2()
                .child(div().flex().flex_wrap().gap_1().children(HABIT_EMOJI.iter().map(|emoji| {
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
                })))
                .child(colors)
                .into_any_element(),
            _ => v_flex()
                .gap_2()
                .child(h_flex().h(px(30.)).px_2().rounded(px(6.)).bg(theme.colors().background).child(edit.icon_text.clone()))
                .child(colors)
                .into_any_element(),
        };
        popover(cx)
            .w(px(371.))
            .px(px(16.))
            .py(px(16.))
            .gap(px(16.))
            .child(h_flex().justify_center().child(tabs))
            .child(body)
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
        // `section-menu=<name>` / `section-rename=<name>`: a custom section's header menu.
        let custom = |name: &str| self.sections.iter().find(|s| s.name == name).map(|s| s.id);
        if let Some(id) = command.strip_prefix("section-menu=").and_then(custom) {
            self.section_menu = Some(id);
            cx.notify();
            return;
        }
        if let Some(id) = command.strip_prefix("section-rename=").and_then(custom) {
            self.start_rename_section(id, window, cx);
            return;
        }
        let Some(id) = self.selected else { return };
        // `menu=focus|style`: the details' "…" menu with a choice open; `icon-checkins`: the
        // "Habit icon" checked-in style.
        if let Some(submenu) = command.strip_prefix("menu=") {
            self.detail_menu_open = true;
            self.detail_submenu = match submenu {
                "focus" => Some(DetailSubmenu::StartFocus),
                "style" => Some(DetailSubmenu::CheckinStyle),
                _ => None,
            };
            cx.notify();
            return;
        }
        if command == "icon-checkins" {
            self.store.set_habit_icon_checkins(id, true).ok();
            self.reload(cx);
            return;
        }
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
        } else if command == "more" {
            // The header's "…" menu.
            self.header_more = Some(HeaderMore::Menu);
            cx.notify();
            return;
        } else if command == "header" {
            // The header's "Habit ▾" menu.
            self.header_menu_open = true;
            cx.notify();
            return;
        } else if command == "list" {
            self.set_list_layout(true, cx);
            return;
        } else if command == "show-archived" {
            self.show_archived = true;
        } else if let Some(which) = command.strip_prefix("row=") {
            // `row=frequency|goal|section`: Edit Habit with that row's dropdown open.
            let which = match which {
                "goal" => EditRow::Goal,
                "section" => EditRow::Section,
                _ => EditRow::Frequency,
            };
            self.update_edit(cx, |e| e.open = Some(which));
            return;
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

    /// An item of the details' "…" menu with a side menu that opens on hover.
    fn side_menu(&self, id: &'static str, icon: IconName, label: &'static str, submenu: DetailSubmenu, cx: &mut Context<Self>) -> SubmenuItem {
        SubmenuItem::new(id, label).icon(icon).open(self.detail_submenu == Some(submenu)).on_hover(cx.listener(
            move |this, hovered: &bool, _, cx| {
                if *hovered && this.detail_submenu != Some(submenu) {
                    this.detail_submenu = Some(submenu);
                    cx.notify();
                }
            },
        ))
    }

    /// A plain item of the details' "…" menu: hovering it closes an open side menu.
    fn menu_entry(&self, id: &'static str, item: ListItem, cx: &mut Context<Self>) -> impl IntoElement {
        div().id(id).child(item).on_hover(cx.listener(
            |this, hovered: &bool, _, cx| {
                if *hovered && this.detail_submenu.is_some() {
                    this.detail_submenu = None;
                    cx.notify();
                }
            },
        ))
    }

    /// A day's check-in circle: a checkmark on a filled circle when done, or the habit's own icon
    /// with "Checked-in Style" set to "Habit icon", as in the original.
    fn checkin_mark(&self, habit: Id, date: NaiveDate, size: Pixels, cx: &App) -> Div {
        let done = self.checkins.get(&habit).is_some_and(|days| days.contains(&date));
        let found = self.habits.iter().find(|h| h.id == habit);
        if done && let Some(habit) = found.filter(|h| h.icon_checkins) {
            return avatar(habit, size, cx);
        }
        div()
            .size(size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(self.dot_color(habit, date, cx))
            .when(done, |el| {
                el.child(Icon::new(IconName::Check).size(if size > px(20.) { IconSize::Small } else { IconSize::XSmall }).color(Color::Custom(gpui::white())))
            })
    }

    /// TickTick's "Daily Goals" chart, as the original's: the last 15 days' amounts as a line over
    /// a grid with its scale on the left, and the goal as a blue line.
    fn render_goal_chart(&self, habit: &Habit, goal: u32, today: NaiveDate, cx: &App) -> AnyElement {
        const DAYS: i64 = 15;
        const HEIGHT: f32 = 140.;
        const AXIS: f32 = 18.;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        let days: Vec<(NaiveDate, u32)> = (0..DAYS)
            .rev()
            .map(|ago| today - chrono::Duration::days(ago))
            .map(|date| (date, self.amount(habit.id, date)))
            .collect();
        let top = fern_core::habit::chart_top(goal, days.iter().map(|(_, v)| *v).max().unwrap_or(0));
        let unit = habit.goal.as_ref().map(|g| g.unit.clone()).unwrap_or_default();
        let (accent, grid) = (colors.accent, colors.border_variant);
        let values: Vec<u32> = days.iter().map(|(_, v)| *v).collect();
        let plot = gpui::canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let (left, top_y) = (bounds.origin.x, bounds.origin.y);
                let (width, height) = (bounds.size.width, bounds.size.height);
                let y = |v: u32| top_y + height * (1. - v as f32 / top as f32);
                for tick in 0..=top {
                    window.paint_quad(gpui::fill(gpui::Bounds::new(gpui::point(left, y(tick)), gpui::size(width, px(1.))), grid));
                }
                let n = values.len().max(1) as f32;
                let points: Vec<gpui::Point<Pixels>> =
                    values.iter().enumerate().map(|(i, v)| gpui::point(left + width * ((i as f32 + 0.5) / n), y(*v))).collect();
                if let Some(first) = points.first() {
                    let mut line = gpui::PathBuilder::stroke(px(2.));
                    line.move_to(*first);
                    for p in &points[1..] {
                        line.line_to(*p);
                    }
                    if let Ok(path) = line.build() {
                        window.paint_path(path, accent.opacity(0.6));
                    }
                }
                window.paint_quad(gpui::fill(gpui::Bounds::new(gpui::point(left, y(goal) - px(1.)), gpui::size(width, px(2.))), accent));
            },
        )
        .flex_1()
        .h(px(HEIGHT));
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
                h_flex()
                    .items_start()
                    .child(
                        div().relative().w(px(AXIS)).h(px(HEIGHT)).children((0..=top).map(|tick| {
                            div()
                                .absolute()
                                .left_0()
                                .top(px(HEIGHT * (1. - tick as f32 / top as f32) - 7.))
                                .child(Label::new(tick.to_string()).size(LabelSize::XSmall).color(Color::Muted))
                        })),
                    )
                    .child(plot),
            )
            .child(h_flex().pl(px(AXIS)).children(days.iter().map(|(date, _)| {
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

    /// A section's header in the grouped cards: its name and count, and for custom sections a
    /// "…" (shown on hover) to rename or delete it.
    fn render_section_header(&self, section: HabitSection, title: String, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let custom = match section {
            HabitSection::Custom(id) => Some(id),
            _ => None,
        };
        let renaming = custom.and_then(|id| self.renaming_section.as_ref().filter(|(r, _)| *r == id)).map(|(_, input)| input.clone());
        let group = SharedString::from(format!("habit-section-header-{}", section.key()));
        h_flex()
            .id(group.clone())
            .group(group.clone())
            .h(px(30.))
            .gap_1()
            .pt_2()
            .map(|el| match renaming {
                Some(input) => el.child(
                    h_flex().w(px(240.)).h(px(26.)).px_2().rounded(px(6.)).bg(cx.theme().colors().surface_background).child(input),
                ),
                None => el
                    .child(Label::new(title).size(LabelSize::Small).weight(FontWeight::SEMIBOLD))
                    .child(Label::new(count.to_string()).size(LabelSize::XSmall).color(Color::Muted)),
            })
            .when_some(custom, |el, id| {
                let open = self.section_menu == Some(id);
                el.child(
                    div()
                        .relative()
                        .opacity(if open { 1. } else { 0. })
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            IconButton::new(SharedString::from(format!("habit-section-more-{id}")), IconName::Ellipsis)
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.section_menu = Some(id);
                                    cx.notify();
                                })),
                        )
                        .when(open, |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowLeft,
                                popover(cx)
                                    .w(px(200.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.section_menu = None;
                                        cx.notify();
                                    }))
                                    .child(
                                        ListItem::new("rename-habit-section")
                                            .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                            .child(Label::new(strings.rename_section))
                                            .on_click(cx.listener(move |this, _, window, cx| this.start_rename_section(id, window, cx))),
                                    )
                                    .child(
                                        ListItem::new("delete-habit-section")
                                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                            .child(Label::new(strings.delete_section).color(Color::Error))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.section_menu = None;
                                                if let Err(err) = this.store.delete_habit_section(id) {
                                                    eprintln!("failed to delete the section: {err:#}");
                                                }
                                                this.reload(cx);
                                            })),
                                    ),
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    fn start_rename_section(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let name = self.sections.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_section_name, cx);
            input.set_text(name, cx);
            input
        });
        cx.subscribe(&input, move |this, _, Submitted(name), cx| {
            if let Err(err) = this.store.rename_habit_section(id, name) {
                eprintln!("failed to rename the section: {err:#}");
            }
            this.renaming_section = None;
            this.reload(cx);
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.renaming_section = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.section_menu = None;
        self.renaming_section = Some((id, input));
        cx.notify();
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
        let data: Vec<(Habit, Vec<Checkin>)> = self
            .habits
            .iter()
            .map(|h| (h.clone(), self.records.get(&h.id).cloned().unwrap_or_default()))
            .collect();
        let name = format!("fern-habits-{}.csv", Local::now().format("%Y-%m-%d"));
        crate::export::save_as(name, habit::export_csv(&data).into_bytes(), cx, |this, result, cx| {
            if let Some(message) = crate::export::outcome(result, i18n::t(cx)) {
                this.export_message = Some(message);
            }
            cx.notify();
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
            // The original's measures: 162 px wide, a row every 34 px.
            HeaderMore::Menu => popover(cx)
                .w(px(162.))
                .on_mouse_down_out(close)
                .child(
                    ListItem::new("habit-settings")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                        .child(Label::new(strings.settings))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.header_more = Some(HeaderMore::Settings);
                            cx.notify();
                        })),
                )
                .child(
                    ListItem::new("habit-export")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::SquareArrowOutUpRight).color(Color::Muted))
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
                self.checkin_mark(id, date, px(16.), cx)
                    .id(SharedString::from(format!("checkin-{id}-{date}")))
                    .cursor_pointer()
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
            // The original's 68 px cards: the label, then the value with its unit beside it.
            v_flex()
                .flex_1()
                .h(px(68.))
                .px_3()
                .justify_center()
                .gap_0p5()
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
                        .items_end()
                        .gap_1()
                        .child(div().text_size(px(20.)).line_height(px(24.)).font_weight(FontWeight::MEDIUM).child(value))
                        .child(div().pb(px(3.)).child(Label::new(unit).size(LabelSize::Small))),
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
                                self.checkin_mark(id, date, px(26.), cx)
                                    .id(SharedString::from(format!("detail-checkin-{date}")))
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
                                    // The original's measures: 178 px wide, a row every 34 px.
                                    popover(cx)
                                        .w(px(178.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.detail_submenu = None;
                                            this.detail_menu_open = false;
                                            cx.notify();
                                        }))
                                        .child(self.menu_entry(
                                            "habit-edit-entry",
                                            ListItem::new("habit-edit")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                                .child(Label::new(strings.edit))
                                                .on_click(cx.listener(|this, _, window, cx| this.start_edit(window, cx))),
                                            cx,
                                        ))
                                        .child({
                                            let title = habit.name.clone();
                                            self.side_menu("habit-start-focus", IconName::CircleDot, strings.start_focus, DetailSubmenu::StartFocus, cx)
                                                .height(px(34.))
                                                .items([(false, strings.start_pomo), (true, strings.start_stopwatch)].map(|(stopwatch, label)| {
                                                    let title = title.clone();
                                                    ListItem::new(SharedString::from(format!("habit-focus-{stopwatch}")))
                                                        .child(Label::new(label))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.detail_menu_open = false;
                                                            this.detail_submenu = None;
                                                            cx.emit(StartFocus { title: title.clone(), stopwatch });
                                                            cx.notify();
                                                        }))
                                                }))
                                        })
                                        .child({
                                            let current = habit.icon_checkins;
                                            self.side_menu("habit-checkin-style", IconName::SquareCheck, strings.checked_in_style, DetailSubmenu::CheckinStyle, cx)
                                                .height(px(34.))
                                                .items([(false, strings.checkin_style_checkmark), (true, strings.checkin_style_icon)].map(|(icon, label)| {
                                                    ListItem::new(SharedString::from(format!("habit-style-{icon}")))
                                                        .child(Label::new(label).color(if icon == current { Color::Accent } else { Color::Default }))
                                                        .when(icon == current, |item| {
                                                            item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent))
                                                        })
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            if let Err(err) = this.store.set_habit_icon_checkins(id, icon) {
                                                                eprintln!("failed to save the checked-in style: {err:#}");
                                                            }
                                                            this.detail_menu_open = false;
                                                            this.detail_submenu = None;
                                                            this.reload(cx);
                                                        }))
                                                }))
                                        })
                                        .child({
                                            let archived = habit.archived;
                                            self.menu_entry(
                                                "habit-archive-entry",
                                                ListItem::new("habit-archive")
                                                    .height(px(34.))
                                                    .start_slot(Icon::new(IconName::Archive).color(Color::Muted))
                                                    .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                                                    .on_click(cx.listener(move |this, _, _, cx| this.set_archived(id, !archived, cx))),
                                                cx,
                                            )
                                        })
                                        .child(self.menu_entry(
                                            "habit-delete-entry",
                                            ListItem::new("habit-delete")
                                                .height(px(34.))
                                                .start_slot(Icon::new(IconName::Trash).color(Color::Muted))
                                                .child(Label::new(strings.delete))
                                                .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
                                            cx,
                                        )),
                                ))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(IconName::CircleCheck, status.success, strings.habit_monthly_checkins, month.days.to_string(), day_unit(month.days as i64, strings).into()))
                    .child(card(IconName::Zap, theme.colors().accent, strings.habit_total_checkins, stats.total_days.to_string(), day_unit(stats.total_days as i64, strings).into())),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(card(IconName::Percent, status.warning, strings.habit_monthly_rate, month.rate.to_string(), "%".into()))
                    .child(card(IconName::Flame, status.error, strings.habit_current_streak, stats.streak.to_string(), day_unit(stats.streak as i64, strings).into())),
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
            // A goal habit's amount for today, typed rather than clicked one unit at a time.
            .when_some(habit.goal.clone(), |el, goal| {
                let done = self.amount(id, today);
                el.child(
                    h_flex()
                        .p_3()
                        .gap_2()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .child(div().flex_1().child(Label::new(strings.habit_today_amount).size(LabelSize::Small)))
                        .child(Label::new(strings.habit_done_so_far.replace("{n}", &done.to_string())).size(LabelSize::Small).color(Color::Muted))
                        .child(
                            h_flex()
                                .w(px(64.))
                                .h(px(26.))
                                .px_2()
                                .rounded(px(6.))
                                .bg(theme.colors().background)
                                .child(self.amount_input.clone()),
                        )
                        .child(Label::new(format!("/ {} {}", goal.amount, goal.unit)).size(LabelSize::Small).color(Color::Muted))
                        .child(
                            div()
                                .id("habit-record-amount")
                                .px_3()
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(theme.colors().accent)
                                .cursor_pointer()
                                .child(Label::new(strings.habit_record).size(LabelSize::Small).color(Color::Custom(gpui::white())))
                                .on_click(cx.listener(|this, _, _, cx| this.record_amount(cx))),
                        ),
                )
            })
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
                    .when(grouped, |el| el.child(self.render_section_header(section, title, in_section.len(), cx)))
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
                        .weight(FontWeight::SEMIBOLD)
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
        // As in the original, the habits take about 64% of the width and the details the rest,
        // whatever the window's size.
        let column = v_flex()
            .w(gpui::relative(0.64))
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
                                // The original's title is medium, not bold.
                                Label::new(if self.show_archived { strings.habit_archived } else { strings.section_habits })
                                    .size(LabelSize::Large)
                                    .weight(FontWeight::MEDIUM),
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
                                    // The original's measures: 162 px wide, a row every 34 px.
                                    popover(cx)
                                        .w(px(162.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.header_menu_open = false;
                                            cx.notify();
                                        }))
                                        .children([(false, strings.habit_active), (true, strings.habit_archived)].map(|(value, label)| {
                                            ListItem::new(if value { "habits-archived" } else { "habits-active" })
                                                .height(px(34.))
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

        // Edit Habit over a dimmed background, as the original shows it.
        let modal = self.editing.as_ref().map(|edit| self.render_edit(edit, cx)).map(|dialog| {
            div()
                .id("habit-modal")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .occlude()
                .child(dialog)
        });
        h_flex()
            .relative()
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
            .children(modal)
    }
}

/// "Day" for 0 or 1 days and "Days" for more, as the original's statistics cards say.
fn day_unit(days: i64, strings: &i18n::Strings) -> &'static str {
    if days <= 1 { strings.habit_unit_day } else { strings.habit_unit_days }
}
