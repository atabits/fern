use chrono::{Datelike, Local, NaiveDate, NaiveTime, Timelike};
use fern_core::HabitFrequency;
use fern_core::habit::{
    CheckinMode, GOAL_DAYS_PRESETS, Goal, Habit, HabitDraft, HabitSection, MAX_CUSTOM_SECTIONS, MAX_REMINDERS,
    format_amount, parse_amount,
};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Button, ButtonSize, ButtonVariant, Modal, Placement, PopoverAttachment, popover};

use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use desktop_common::{calendar, colors as lists_panel};
use fern_i18n as i18n;

use crate::{ConfirmAction, HABIT_EMOJI, HabitView, avatar};

/// Unified Create / Edit Habit modal state.
pub(crate) struct HabitEdit {
    /// `None` when creating a new habit; `Some(id)` when editing an existing one.
    pub id: Option<fern_core::Id>,
    pub name: Entity<TextInput>,
    pub amount_input: Entity<TextInput>,
    pub step_input: Entity<TextInput>,
    pub unit: Entity<TextInput>,
    pub has_goal: bool,
    pub mode: CheckinMode,
    pub reminders: Vec<NaiveTime>,
    /// Tracks whether the single reminder came from auto-filling Morning/Afternoon/Night so
    /// switching sections can update it cleanly without overwriting user edits.
    pub auto_reminder: bool,
    pub frequency: HabitFrequency,
    pub section: HabitSection,
    pub new_section: Option<Entity<TextInput>>,
    /// Explicitly chosen start date, or `None` ("from creation / unset").
    pub start_date: Option<NaiveDate>,
    pub goal_days: Option<u32>,
    /// Month currently shown in the Start Date calendar popover.
    pub start_calendar: Option<NaiveDate>,
    pub icon: Option<String>,
    pub color: Option<String>,
    /// Open icon picker tab: 0 = Icon (emoji+color), 1 = Emoji, 2 = Text.
    pub icon_picker: Option<usize>,
    pub icon_text: Entity<TextInput>,
    pub reminder_input: Entity<TextInput>,
    pub goal_days_input: Option<Entity<TextInput>>,
    pub open: Option<EditRow>,
    /// Active reminder time picker: `Some(None)` for adding via `+`, `Some(Some(idx))` for editing chip `idx`.
    pub reminder_picker: Option<Option<usize>>,
    pub constant_reminder: bool,
    pub record_enable: bool,
    /// Original span values when editing, used to detect cycle-reset confirmation.
    pub original_start_date: Option<NaiveDate>,
    pub original_goal_days: Option<u32>,
}

/// The rows inside the Create/Edit modal that open a dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditRow {
    Frequency,
    Goal,
    StartDate,
    GoalDays,
    Section,
}

pub(crate) fn frequency_label(frequency: HabitFrequency, strings: &i18n::Strings) -> String {
    match frequency {
        HabitFrequency::Daily { weekdays } if weekdays == HabitFrequency::EVERY_DAY => {
            strings.habit_frequency_kinds[0].to_owned()
        }
        HabitFrequency::Daily { weekdays } => (0..7)
            .filter(|i| weekdays & (1 << i) != 0)
            .map(|i| strings.weekdays_short[i])
            .collect::<Vec<_>>()
            .join(", "),
        HabitFrequency::Weekly { days } => strings.habit_days_per_week.replace("{n}", &days.to_string()),
        HabitFrequency::Interval { days } => strings.habit_every_n_days.replace("{n}", &days.to_string()),
    }
}

impl HabitView {
    /// Opens the full Create Habit modal directly (optionally pre-selecting a section).
    pub(crate) fn open_create_modal(&mut self, section: HabitSection, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let name = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_name, cx));
        let amount_input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "1", cx);
            input.set_text("1", cx);
            input
        });
        let step_input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "1", cx);
            input.set_text("1", cx);
            input
        });
        let unit = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_unit_placeholder, cx);
            input.set_text("", cx);
            input
        });
        let reminder_input = self.make_reminder_input(cx);
        let icon_text = self.make_icon_text_input(cx);
        self.wire_edit_inputs(&name, &amount_input, &step_input, &unit, cx);

        let mut reminders = Vec::new();
        let mut auto_reminder = false;
        if let Some(time) = section.default_reminder() {
            reminders.push(time);
            auto_reminder = true;
        }

        window.focus(&name.focus_handle(cx), cx);
        self.editing = Some(HabitEdit {
            id: None,
            name,
            amount_input,
            step_input,
            unit,
            has_goal: false,
            mode: CheckinMode::Auto { step: 1.0 },
            reminders,
            auto_reminder,
            frequency: HabitFrequency::default(),
            section,
            new_section: None,
            start_date: None,
            goal_days: None,
            start_calendar: None,
            icon: None,
            color: None,
            icon_picker: None,
            icon_text,
            reminder_input,
            goal_days_input: None,
            open: None,
            reminder_picker: None,
            constant_reminder: false,
            record_enable: false,
            original_start_date: None,
            original_goal_days: None,
        });
        self.detail_menu_open = false;
        self.card_menu = None;
        self.day_menu = None;
        cx.notify();
    }

    pub(crate) fn open_edit_modal(&mut self, id: fern_core::Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(habit) = self.habits.iter().find(|h| h.id == id).cloned() else {
            return;
        };
        let strings = i18n::t(cx);
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_name, cx);
            input.set_text(habit.name.clone(), cx);
            input
        });
        let amount_text = habit.goal.as_ref().map(|g| format_amount(g.amount)).unwrap_or_else(|| "1".into());
        let amount_input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "1", cx);
            input.set_text(amount_text, cx);
            input
        });
        let step_val = habit
            .goal
            .as_ref()
            .map(|g| match g.mode {
                CheckinMode::Auto { step } => format_amount(step),
                _ => "1".into(),
            })
            .unwrap_or_else(|| "1".into());
        let step_input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "1", cx);
            input.set_text(step_val, cx);
            input
        });
        let unit = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.habit_unit_placeholder, cx);
            input.set_text(habit.goal.as_ref().map(|g| g.unit.clone()).unwrap_or_default(), cx);
            input
        });
        let reminder_input = self.make_reminder_input(cx);
        let icon_text = self.make_icon_text_input(cx);
        self.wire_edit_inputs(&name, &amount_input, &step_input, &unit, cx);

        let auto_reminder = habit.reminders.len() == 1
            && habit.section.default_reminder() == Some(habit.reminders[0]);

        window.focus(&name.focus_handle(cx), cx);
        self.editing = Some(HabitEdit {
            id: Some(habit.id),
            name,
            amount_input,
            step_input,
            unit,
            has_goal: habit.goal.is_some(),
            mode: habit.goal.as_ref().map_or(CheckinMode::Auto { step: 1.0 }, |g| g.mode),
            reminders: habit.reminders.clone(),
            auto_reminder,
            frequency: habit.frequency,
            section: habit.section,
            new_section: None,
            start_date: habit.start_date,
            goal_days: habit.goal_days,
            start_calendar: None,
            icon: habit.icon.clone(),
            color: habit.color.clone(),
            icon_picker: None,
            icon_text,
            reminder_input,
            goal_days_input: None,
            open: None,
            reminder_picker: None,
            constant_reminder: habit.constant_reminder,
            record_enable: habit.record_enable,
            original_start_date: habit.start_date,
            original_goal_days: habit.goal_days,
        });
        self.detail_menu_open = false;
        self.card_menu = None;
        self.day_menu = None;
        cx.notify();
    }

    fn wire_edit_inputs(
        &self,
        name: &Entity<TextInput>,
        amount_input: &Entity<TextInput>,
        step_input: &Entity<TextInput>,
        unit: &Entity<TextInput>,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(name, |this, _, _: &Submitted, cx| {
            this.request_save_edit(cx);
        })
        .detach();
        for input in [name, amount_input, step_input, unit] {
            cx.subscribe(input, |this, _, _: &Cancelled, cx| {
                if let Some(edit) = this.editing.as_mut()
                    && edit.open.is_some()
                {
                    edit.open = None;
                    edit.start_calendar = None;
                } else {
                    this.editing = None;
                }
                cx.notify();
            })
            .detach();
        }
    }

    fn make_reminder_input(&self, cx: &mut Context<Self>) -> Entity<TextInput> {
        let reminder_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "09:00", cx));
        cx.subscribe(&reminder_input, |this, input, Submitted(text), cx| {
            this.commit_reminder_text(text, &input, cx);
        })
        .detach();
        cx.subscribe(&reminder_input, |this, input, _: &Cancelled, cx| {
            input.update(cx, |input, cx| input.set_text("", cx));
            this.update_edit(cx, |e| e.reminder_picker = None);
        })
        .detach();
        reminder_input
    }

    fn open_reminder_picker(&mut self, slot: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let default_time = match slot {
            Some(idx) => self
                .editing
                .as_ref()
                .and_then(|e| e.reminders.get(idx).copied())
                .unwrap_or_else(next_hour_time),
            None => next_hour_time(),
        };
        let formatted = default_time.format("%H:%M").to_string();
        let input = self.editing.as_ref().map(|e| e.reminder_input.clone());
        if let Some(input) = input {
            input.update(cx, |inp, cx| inp.set_text(formatted, cx));
            window.focus(&input.focus_handle(cx), cx);
        }
        self.update_edit(cx, |e| {
            e.open = None;
            e.reminder_picker = Some(slot);
        });
    }

    fn apply_reminder_time(&mut self, slot: Option<usize>, time: NaiveTime, cx: &mut Context<Self>) {
        let input = self.editing.as_ref().map(|e| e.reminder_input.clone());
        self.update_edit(cx, |e| {
            match slot {
                Some(idx) if idx < e.reminders.len() => {
                    if !e.reminders.contains(&time) || e.reminders[idx] == time {
                        e.reminders[idx] = time;
                        e.reminders.sort();
                        e.reminders.dedup();
                        e.auto_reminder = false;
                    }
                }
                _ => {
                    if e.reminders.len() < MAX_REMINDERS && !e.reminders.contains(&time) {
                        e.reminders.push(time);
                        e.reminders.sort();
                        e.auto_reminder = false;
                    }
                }
            }
            e.reminder_picker = None;
        });
        if let Some(input) = input {
            input.update(cx, |inp, cx| inp.set_text("", cx));
        }
    }

    fn commit_reminder_text(&mut self, text: &str, _input: &Entity<TextInput>, cx: &mut Context<Self>) {
        let Some(time) = calendar::parse_time(text) else { return };
        let slot = self.editing.as_ref().and_then(|e| e.reminder_picker).flatten();
        self.apply_reminder_time(slot, time, cx);
    }

    fn make_icon_text_input(&self, cx: &mut Context<Self>) -> Entity<TextInput> {
        let strings = i18n::t(cx);
        let icon_text = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_text_placeholder, cx));
        cx.subscribe(&icon_text, |this, _, Submitted(text), cx| {
            let text: String = text.trim().chars().take(2).collect();
            this.update_edit(cx, |e| {
                e.icon = (!text.is_empty()).then_some(text);
                e.icon_picker = None;
            });
        })
        .detach();
        cx.subscribe(&icon_text, |this, _, _: &Cancelled, cx| {
            this.update_edit(cx, |e| e.icon_picker = None);
        })
        .detach();
        icon_text
    }

    pub(crate) fn update_edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut HabitEdit)) {
        if let Some(edit) = self.editing.as_mut() {
            change(edit);
        }
        cx.notify();
    }

    fn build_draft(edit: &HabitEdit, cx: &App) -> Option<HabitDraft> {
        let name = edit.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            return None;
        }
        let goal = if edit.has_goal {
            let amount = parse_amount(edit.amount_input.read(cx).text()).unwrap_or(1.0);
            let mode = match edit.mode {
                CheckinMode::Auto { .. } => {
                    let step = parse_amount(edit.step_input.read(cx).text()).unwrap_or(1.0);
                    CheckinMode::Auto { step }
                }
                other => other,
            };
            Some(Goal { amount, unit: edit.unit.read(cx).text().trim().to_owned(), mode })
        } else {
            None
        };
        let mut reminders = edit.reminders.clone();
        reminders.sort();
        reminders.dedup();
        reminders.truncate(MAX_REMINDERS);
        Some(HabitDraft {
            name,
            goal,
            reminders: reminders.clone(),
            frequency: edit.frequency.normalize(),
            icon: edit.icon.clone(),
            color: edit.color.clone(),
            start_date: edit.start_date,
            goal_days: edit.goal_days.filter(|d| *d > 0),
            section: edit.section,
            constant_reminder: !reminders.is_empty() && edit.constant_reminder,
            record_enable: edit.record_enable,
        })
    }

    pub(crate) fn request_save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.as_ref() else { return };
        let Some(draft) = Self::build_draft(edit, cx) else { return };
        // Warn before committing if editing an existing finite-cycle habit with check-ins and
        // changing its start_date or goal_days.
        if let Some(id) = edit.id {
            let span_changed =
                draft.start_date != edit.original_start_date || draft.goal_days != edit.original_goal_days;
            let had_finite_cycle = edit.original_goal_days.is_some();
            let has_checkins = self.checkins.get(&id).is_some_and(|c| !c.is_empty());
            if span_changed && had_finite_cycle && has_checkins {
                self.confirm = Some(ConfirmAction::SaveHabitCycleChange { id, draft });
                cx.notify();
                return;
            }
        }
        self.commit_habit_draft(edit.id, draft, cx);
    }

    pub(crate) fn commit_habit_draft(&mut self, id: Option<fern_core::Id>, draft: HabitDraft, cx: &mut Context<Self>) {
        self.editing = None;
        match id {
            Some(existing_id) => {
                if let Err(err) = self.store.save_habit(existing_id, &draft) {
                    eprintln!("failed to save habit: {err:#}");
                }
            }
            None => match self.store.create_habit(&draft) {
                Ok(created) => {
                    self.show_archived = false;
                    self.selected = Some(created.id);
                }
                Err(err) => eprintln!("failed to create habit: {err:#}"),
            },
        }
        self.notify_data_changed(cx);
        self.reload(cx);
    }

    pub(crate) fn render_edit_modal(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let today = Local::now().date_naive();
        let preview = Habit {
            id: edit.id.unwrap_or_default(),
            name: edit.name.read(cx).text().to_owned(),
            goal: None,
            reminders: Vec::new(),
            frequency: edit.frequency,
            icon: edit.icon.clone(),
            color: edit.color.clone(),
            start_date: edit.start_date,
            goal_days: edit.goal_days,
            section: edit.section,
            archived: false,
            archived_date: None,
            icon_checkins: false,
            constant_reminder: edit.constant_reminder,
            record_enable: edit.record_enable,
            ex_dates: Vec::new(),
            sort_order: 0,
        };
        let title_text = if edit.id.is_some() { strings.edit_habit } else { strings.habit_add_title };

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
            h_flex()
                .flex_1()
                .min_w_0()
                .h(px(32.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().background)
                .child(child)
        };
        let row = |label: &'static str, right: AnyElement| {
            h_flex()
                .gap(px(12.))
                .items_center()
                .child(div().w(px(100.)).flex_none().child(Label::new(label).size(LabelSize::Small).color(Color::Muted)))
                .child(div().flex_1().min_w_0().child(right))
        };
        let menu_row = |which: EditRow, id: &'static str, text: String, open: bool, cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .h(px(32.))
                .px(px(10.))
                .justify_between()
                .rounded(px(6.))
                .border_1()
                .border_color(if open { theme.colors().accent } else { theme.colors().border })
                .bg(theme.colors().background)
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().element_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_edit(cx, |e| {
                        let opening = e.open != Some(which);
                        e.open = opening.then_some(which);
                        if opening && which == EditRow::StartDate {
                            e.start_calendar = Some(calendar::month_start(e.start_date.unwrap_or(today)));
                        } else if !opening && which == EditRow::StartDate {
                            e.start_calendar = None;
                        }
                    });
                }))
                .child(Label::new(text).size(LabelSize::Small).truncate())
                .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
        };

        let section_name = self.section_label(edit.section, cx);
        let goal_summary = if edit.has_goal {
            let amt = edit.amount_input.read(cx).text().trim().to_owned();
            let unit = edit.unit.read(cx).text().trim().to_owned();
            if unit.is_empty() {
                format!("{amt}{}", strings.habit_per_day)
            } else {
                format!("{amt} {unit}{}", strings.habit_per_day)
            }
        } else {
            strings.habit_goal_all.to_owned()
        };
        let start_label = edit
            .start_date
            .map(|d| desktop_common::calendar::format_date(d, strings))
            .unwrap_or_else(|| strings.habit_start_anytime.to_owned());
        let goal_days_label = edit
            .goal_days
            .map(|n| format!("{n} {}", day_unit_str(n as i64, strings)))
            .unwrap_or_else(|| strings.habit_forever.to_owned());

        let can_save = !edit.name.read(cx).text().trim().is_empty();

        Modal::new("habit-modal")
            .width(px(470.))
            .max_height(px(640.))
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new(title_text).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                    .child(
                        IconButton::new("habit-edit-close", IconName::X)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                    ),
            )
            // Name + Icon picker trigger
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .relative()
                            .child(
                                div()
                                    .id("habit-edit-icon")
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.update_edit(cx, |e| {
                                            e.icon_picker = if e.icon_picker.is_some() { None } else { Some(0) };
                                        });
                                    }))
                                    .child(avatar(&preview, px(36.), cx)),
                            )
                            .when_some(edit.icon_picker, |el, tab| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_icon_picker(edit, tab, cx),
                                ))
                            }),
                    )
                    .child(field(edit.name.clone().into_any_element())),
            )
            // Frequency
            .child(row(
                strings.habit_frequency,
                div()
                    .relative()
                    .child(menu_row(
                        EditRow::Frequency,
                        "habit-edit-freq",
                        frequency_label(edit.frequency, strings),
                        edit.open == Some(EditRow::Frequency),
                        cx,
                    ))
                    .when(edit.open == Some(EditRow::Frequency), |el| {
                        el.child(PopoverAttachment::new(
                            Placement::BelowLeft,
                            self.render_frequency_popover(edit, cx),
                        ))
                    })
                    .into_any_element(),
            ))
            // Goal
            .child(row(
                strings.habit_goal,
                div()
                    .relative()
                    .child(menu_row(
                        EditRow::Goal,
                        "habit-edit-goal",
                        goal_summary,
                        edit.open == Some(EditRow::Goal),
                        cx,
                    ))
                    .when(edit.open == Some(EditRow::Goal), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_goal_popover(edit, cx)))
                    })
                    .into_any_element(),
            ))
            // Start Date & Goal Days on one balanced row
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(Label::new(strings.habit_start_date).size(LabelSize::XSmall).color(Color::Muted))
                            .child(
                                div()
                                    .relative()
                                    .child(menu_row(
                                        EditRow::StartDate,
                                        "habit-edit-start",
                                        start_label,
                                        edit.open == Some(EditRow::StartDate),
                                        cx,
                                    ))
                                    .when(edit.open == Some(EditRow::StartDate), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::BelowLeft,
                                            self.render_start_date_popover(edit, today, cx),
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(Label::new(strings.habit_goal_days).size(LabelSize::XSmall).color(Color::Muted))
                            .child(
                                div()
                                    .relative()
                                    .child(menu_row(
                                        EditRow::GoalDays,
                                        "habit-edit-goal-days",
                                        goal_days_label,
                                        edit.open == Some(EditRow::GoalDays),
                                        cx,
                                    ))
                                    .when(edit.open == Some(EditRow::GoalDays), |el| {
                                        el.child(PopoverAttachment::new(
                                            Placement::BelowRight,
                                            self.render_goal_days_popover(edit, cx),
                                        ))
                                    }),
                            ),
                    ),
            )
            // Section
            .child(row(
                strings.habit_section,
                div()
                    .relative()
                    .child(menu_row(
                        EditRow::Section,
                        "habit-edit-section",
                        section_name,
                        edit.open == Some(EditRow::Section),
                        cx,
                    ))
                    .when(edit.open == Some(EditRow::Section), |el| {
                        el.child(PopoverAttachment::new(Placement::BelowLeft, self.render_section_popover(edit, cx)))
                    })
                    .into_any_element(),
            ))
            // Reminders (up to 10, with 15-min dropdown TimePicker + manual input)
            .child(row(
                strings.reminder,
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(edit.reminders.iter().enumerate().map(|(idx, &time)| {
                        let label = calendar::format_time(time, strings);
                        let picker_open = edit.reminder_picker == Some(Some(idx));
                        div()
                            .relative()
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("habit-rem-{idx}")))
                                    .h(px(28.))
                                    .px_2()
                                    .gap_1()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(theme.colors().surface_background)
                                    .border_1()
                                    .border_color(if picker_open { theme.colors().accent } else { theme.colors().border })
                                    .hover(|s| s.border_color(theme.colors().accent))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_reminder_picker(Some(idx), window, cx);
                                    }))
                                    .child(Label::new(label).size(LabelSize::XSmall))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("habit-rem-del-{idx}")))
                                            .cursor_pointer()
                                            .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Muted))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.update_edit(cx, |e| {
                                                    if idx < e.reminders.len() {
                                                        e.reminders.remove(idx);
                                                        e.auto_reminder = false;
                                                        e.reminder_picker = None;
                                                        if e.reminders.is_empty() {
                                                            e.constant_reminder = false;
                                                        }
                                                    }
                                                });
                                            })),
                                    ),
                            )
                            .when(picker_open, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowLeft,
                                    self.render_reminder_time_picker(edit, Some(idx), cx),
                                ))
                            })
                    }))
                    .when(edit.reminders.len() < MAX_REMINDERS, |el| {
                        let picker_open = edit.reminder_picker == Some(None);
                        el.child(
                            div()
                                .relative()
                                .child(
                                    IconButton::new("habit-rem-add", IconName::Plus)
                                        .size(px(28.))
                                        .icon_size(IconSize::XSmall)
                                        .toggle_state(picker_open)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_reminder_picker(None, window, cx);
                                        })),
                                )
                                .when(picker_open, |el| {
                                    el.child(PopoverAttachment::new(
                                        Placement::BelowLeft,
                                        self.render_reminder_time_picker(edit, None, cx),
                                    ))
                                }),
                        )
                    })
                    .into_any_element(),
            ))
            // Constant Reminder (only shown when at least one reminder exists)
            .when(!edit.reminders.is_empty(), |el| {
                el.child(
                    option("habit-edit-constant", strings.habit_constant_reminder, edit.constant_reminder)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_edit(cx, |e| e.constant_reminder = !e.constant_reminder);
                        })),
                )
            })
            // Auto-pop Habit Log
            .child(
                option("habit-edit-auto-log", strings.habit_auto_log, edit.record_enable).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.update_edit(cx, |e| e.record_enable = !e.record_enable);
                    },
                )),
            )
            // Footer buttons
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("habit-edit-cancel", strings.cancel)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("habit-edit-save", strings.save)
                            .variant(ButtonVariant::Primary)
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, _, cx| this.request_save_edit(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_reminder_time_picker(
        &self,
        edit: &HabitEdit,
        slot: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let current_parsed = calendar::parse_time(edit.reminder_input.read(cx).text());
        let rem_input = edit.reminder_input.clone();

        let slots: Vec<NaiveTime> = (0..24u32)
            .flat_map(|h| [0u32, 15, 30, 45].into_iter().filter_map(move |m| NaiveTime::from_hms_opt(h, m, 0)))
            .collect();

        popover(cx)
            .w(px(156.))
            .p_1p5()
            .gap_1p5()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| e.reminder_picker = None);
            }))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h(px(28.))
                            .px_2()
                            .text_size(px(13.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().accent)
                            .bg(theme.colors().background)
                            .child(edit.reminder_input.clone()),
                    )
                    .child(
                        IconButton::new("rem-picker-ok", IconName::Check)
                            .icon_size(IconSize::XSmall)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let text = rem_input.read(cx).text().to_owned();
                                this.commit_reminder_text(&text, &rem_input, cx);
                            })),
                    ),
            )
            .child(
                v_scroll(
                    "habit-rem-time-scroll",
                    v_flex().id("habit-rem-time-list").children(slots.into_iter().map(|time| {
                        let selected = current_parsed == Some(time);
                        let label = calendar::format_time(time, strings);
                        ListItem::new(SharedString::from(format!("rem-slot-{:02}{:02}", time.hour(), time.minute())))
                            .height(px(28.))
                            .toggle_state(selected)
                            .child(
                                Label::new(label)
                                    .size(LabelSize::Small)
                                    .color(if selected { Color::Accent } else { Color::Default }),
                            )
                            .when(selected, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.apply_reminder_time(slot, time, cx);
                            }))
                    })),
                )
                .max_h(px(210.))
                .compact(),
            )
    }

    fn render_frequency_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let active_kind = match edit.frequency {
            HabitFrequency::Daily { .. } => 0,
            HabitFrequency::Weekly { .. } => 1,
            HabitFrequency::Interval { .. } => 2,
        };
        popover(cx)
            .w(px(280.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| e.open = None);
            }))
            .child(
                h_flex()
                    .gap_1()
                    .children((0..3).map(|kind| {
                        let selected = kind == active_kind;
                        div()
                            .id(SharedString::from(format!("freq-tab-{kind}")))
                            .flex_1()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .bg(if selected { theme.colors().accent.opacity(0.18) } else { theme.colors().surface_background })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    e.frequency = match kind {
                                        1 => HabitFrequency::Weekly { days: 3 },
                                        2 => HabitFrequency::Interval { days: 2 },
                                        _ => HabitFrequency::default(),
                                    };
                                });
                            }))
                            .child(
                                Label::new(strings.habit_frequency_kinds[kind])
                                    .size(LabelSize::XSmall)
                                    .color(if selected { Color::Accent } else { Color::Default }),
                            )
                    })),
            )
            .child(match edit.frequency {
                HabitFrequency::Daily { weekdays } => h_flex()
                    .justify_between()
                    .children((0..7usize).map(|day| {
                        let on = weekdays & (1 << day) != 0;
                        div()
                            .id(SharedString::from(format!("freq-wd-{day}")))
                            .size(px(30.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(if on { theme.colors().accent } else { theme.colors().surface_background })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    if let HabitFrequency::Daily { weekdays } = &mut e.frequency {
                                        let next = *weekdays ^ (1 << day);
                                        if next != 0 {
                                            *weekdays = next;
                                        }
                                    }
                                });
                            }))
                            .child(
                                Label::new(strings.weekdays_short[day])
                                    .size(LabelSize::XSmall)
                                    .color(if on { Color::OnAccent } else { Color::Default }),
                            )
                    }))
                    .into_any_element(),
                HabitFrequency::Weekly { days } => h_flex()
                    .justify_between()
                    .children((1u8..=6).map(|n| {
                        let selected = days == n;
                        div()
                            .id(SharedString::from(format!("freq-wk-{n}")))
                            .size(px(32.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(if selected { theme.colors().accent } else { theme.colors().surface_background })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| e.frequency = HabitFrequency::Weekly { days: n });
                            }))
                            .child(
                                Label::new(n.to_string())
                                    .size(LabelSize::Small)
                                    .color(if selected { Color::OnAccent } else { Color::Default }),
                            )
                    }))
                    .into_any_element(),
                HabitFrequency::Interval { days } => h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new(strings.habit_every_n_days.replace("{n}", &days.to_string())).size(LabelSize::Small))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("freq-int-dec", "-")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.update_edit(cx, |e| {
                                            if let HabitFrequency::Interval { days } = &mut e.frequency {
                                                *days = (*days).saturating_sub(1).max(2);
                                            }
                                        });
                                    })),
                            )
                            .child(
                                Button::new("freq-int-inc", "+")
                                    .variant(ButtonVariant::Secondary)
                                    .size(ButtonSize::Sm)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.update_edit(cx, |e| {
                                            if let HabitFrequency::Interval { days } = &mut e.frequency {
                                                *days = (*days + 1).min(30);
                                            }
                                        });
                                    })),
                            ),
                    )
                    .into_any_element(),
            })
    }

    fn render_goal_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let mode_idx = match edit.mode {
            CheckinMode::Auto { .. } => 0,
            CheckinMode::Manual => 1,
            CheckinMode::All => 2,
        };
        let input_box = |input: Entity<TextInput>| {
            h_flex()
                .h(px(28.))
                .px_2()
                .text_size(px(13.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().background)
                .child(input)
        };
        popover(cx)
            .w(px(340.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| e.open = None);
            }))
            .child(
                ListItem::new("goal-opt-all")
                    .height(px(30.))
                    .child(Label::new(strings.habit_goal_all).size(LabelSize::Small))
                    .when(!edit.has_goal, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.update_edit(cx, |e| {
                            e.has_goal = false;
                            e.open = None;
                        });
                    })),
            )
            .child(
                ListItem::new("goal-opt-amount")
                    .height(px(30.))
                    .child(Label::new(strings.habit_goal_amount).size(LabelSize::Small))
                    .when(edit.has_goal, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.update_edit(cx, |e| e.has_goal = true);
                    })),
            )
            .when(edit.has_goal, |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(input_box(edit.amount_input.clone()).w(px(72.)))
                        .child(input_box(edit.unit.clone()).flex_1().min_w_0())
                        .child(Label::new(strings.habit_per_day).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .pt_1()
                        .child(
                            Label::new(strings.habit_checkin_mode_label)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(h_flex().gap_1().children(
                            [
                                (0, strings.habit_checkin_auto),
                                (1, strings.habit_checkin_manual),
                                (2, strings.habit_checkin_all),
                            ]
                            .map(|(idx, label)| {
                                let selected = idx == mode_idx;
                                div()
                                    .id(SharedString::from(format!("goal-mode-{idx}")))
                                    .flex_1()
                                    .h(px(26.))
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(if selected {
                                        theme.colors().accent.opacity(0.18)
                                    } else {
                                        theme.colors().surface_background
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.update_edit(cx, |e| {
                                            e.mode = match idx {
                                                1 => CheckinMode::Manual,
                                                2 => CheckinMode::All,
                                                _ => CheckinMode::Auto { step: 1.0 },
                                            };
                                        });
                                    }))
                                    .child(
                                        Label::new(label)
                                            .size(LabelSize::XSmall)
                                            .color(if selected { Color::Accent } else { Color::Default }),
                                    )
                            }),
                        ))
                        .when(matches!(edit.mode, CheckinMode::Auto { .. }), |el| {
                            el.child(
                                h_flex()
                                    .gap_2()
                                    .pt_1()
                                    .items_center()
                                    .child(Label::new(strings.habit_step).size(LabelSize::XSmall).color(Color::Muted))
                                    .child(input_box(edit.step_input.clone()).w(px(76.))),
                            )
                        }),
                )
            })
    }

    fn render_start_date_popover(&self, edit: &HabitEdit, today: NaiveDate, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let shown_month = edit.start_calendar.unwrap_or_else(|| calendar::month_start(edit.start_date.unwrap_or(today)));
        let month_label = format!("{} {}", strings.months[shown_month.month0() as usize], shown_month.year());
        let grid = calendar::month_grid(shown_month);

        popover(cx)
            .w(px(264.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| {
                    e.open = None;
                    e.start_calendar = None;
                });
            }))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(IconButton::new("start-cal-prev", IconName::ChevronLeft).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.update_edit(cx, |e| {
                                let base = e
                                    .start_calendar
                                    .unwrap_or_else(|| calendar::month_start(e.start_date.unwrap_or_else(|| Local::now().date_naive())));
                                e.start_calendar = Some(calendar::add_months(base, -1));
                            });
                        },
                    )))
                    .child(Label::new(month_label).size(LabelSize::Small).weight(FontWeight::MEDIUM))
                    .child(IconButton::new("start-cal-next", IconName::ChevronRight).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.update_edit(cx, |e| {
                                let base = e
                                    .start_calendar
                                    .unwrap_or_else(|| calendar::month_start(e.start_date.unwrap_or_else(|| Local::now().date_naive())));
                                e.start_calendar = Some(calendar::add_months(base, 1));
                            });
                        },
                    ))),
            )
            .child(h_flex().children(calendar::weekday_labels(&strings.weekdays_short).iter().map(|d| {
                div().flex_1().flex().justify_center().child(Label::new(*d).size(LabelSize::XSmall).color(Color::Muted))
            })))
            .children(grid.chunks(7).map(|week| {
                h_flex().children(week.iter().map(|&date| {
                    let selected = edit.start_date == Some(date);
                    let in_month = date.month() == shown_month.month();
                    div()
                        .id(SharedString::from(format!("start-day-{date}")))
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(!in_month, |el| el.opacity(0.4))
                        .when(selected, |el| el.bg(theme.colors().accent))
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_edit(cx, |e| {
                                e.start_date = Some(date);
                                e.open = None;
                                e.start_calendar = None;
                            });
                        }))
                        .child(
                            Label::new(date.day().to_string())
                                .size(LabelSize::XSmall)
                                .color(if selected {
                                    Color::OnAccent
                                } else if date == today {
                                    Color::Accent
                                } else {
                                    Color::Default
                                }),
                        )
                }))
            }))
            .child(
                h_flex()
                    .justify_between()
                    .pt_1()
                    .child(
                        Button::new("start-cal-today", strings.today)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    e.start_date = Some(today);
                                    e.open = None;
                                    e.start_calendar = None;
                                });
                            })),
                    )
                    .child(
                        Button::new("start-cal-clear", strings.habit_clear_date)
                            .variant(ButtonVariant::Secondary)
                            .size(ButtonSize::Sm)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    e.start_date = None;
                                    e.open = None;
                                    e.start_calendar = None;
                                });
                            })),
                    ),
            )
    }

    fn render_goal_days_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(210.))
            .p_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| {
                    e.open = None;
                    e.goal_days_input = None;
                });
            }))
            .child(
                ListItem::new("gd-forever")
                    .height(px(30.))
                    .child(Label::new(strings.habit_forever).size(LabelSize::Small))
                    .when(edit.goal_days.is_none(), |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.update_edit(cx, |e| {
                            e.goal_days = None;
                            e.goal_days_input = None;
                            e.open = None;
                        });
                    })),
            )
            .children(GOAL_DAYS_PRESETS.map(|days| {
                let selected = edit.goal_days == Some(days);
                ListItem::new(SharedString::from(format!("gd-{days}")))
                    .height(px(30.))
                    .child(Label::new(format!("{days} {}", day_unit_str(days as i64, strings))).size(LabelSize::Small))
                    .when(selected, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_edit(cx, |e| {
                            e.goal_days = Some(days);
                            e.goal_days_input = None;
                            e.open = None;
                        });
                    }))
            }))
            .child(match edit.goal_days_input.clone() {
                Some(input) => h_flex()
                    .px_2()
                    .py_1()
                    .child(
                        h_flex()
                            .flex_1()
                            .h(px(28.))
                            .px_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().accent)
                            .bg(theme.colors().background)
                            .child(input),
                    )
                    .into_any_element(),
                None => ListItem::new("gd-custom")
                    .height(px(30.))
                    .child(Label::new(strings.habit_custom).size(LabelSize::Small).color(Color::Accent))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, "1–999", cx));
                        cx.subscribe(&input, |this, _, Submitted(text), cx| {
                            if let Ok(days) = text.trim().parse::<u32>()
                                && (1..=999).contains(&days)
                            {
                                this.update_edit(cx, |e| {
                                    e.goal_days = Some(days);
                                    e.goal_days_input = None;
                                    e.open = None;
                                });
                            }
                        })
                        .detach();
                        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
                            this.update_edit(cx, |e| e.goal_days_input = None);
                        })
                        .detach();
                        window.focus(&input.focus_handle(cx), cx);
                        this.update_edit(cx, |e| e.goal_days_input = Some(input));
                    }))
                    .into_any_element(),
            })
    }

    fn render_section_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(220.))
            .p_1()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| {
                    e.open = None;
                    e.new_section = None;
                });
            }))
            .children(HabitSection::ordered(&self.sections).into_iter().map(|section| {
                let selected = edit.section == section;
                let label = self.section_label(section, cx);
                ListItem::new(SharedString::from(format!("sec-opt-{}", section.key())))
                    .height(px(30.))
                    .child(Label::new(label).size(LabelSize::Small))
                    .when(selected, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_edit(cx, |e| {
                            e.section = section;
                            // Auto-suggest section default reminder (09:00 / 13:00 / 20:00) if
                            // reminders are empty or still hold an untouched auto-suggested default.
                            let can_auto = e.reminders.is_empty()
                                || (e.auto_reminder && e.reminders.len() == 1 && HabitSection::is_default_section_reminder(e.reminders[0]));
                            if can_auto {
                                if let Some(time) = section.default_reminder() {
                                    e.reminders = vec![time];
                                    e.auto_reminder = true;
                                } else if e.auto_reminder {
                                    e.reminders.clear();
                                    e.auto_reminder = false;
                                }
                            }
                            e.open = None;
                        });
                    }))
            }))
            .when(self.sections.len() < MAX_CUSTOM_SECTIONS, |el| {
                el.child(match edit.new_section.clone() {
                    Some(input) => h_flex()
                        .px_2()
                        .py_1()
                        .child(
                            h_flex()
                                .flex_1()
                                .h(px(28.))
                                .px_2()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.colors().accent)
                                .bg(theme.colors().background)
                                .child(input),
                        )
                        .into_any_element(),
                    None => ListItem::new("sec-add-new")
                        .height(px(30.))
                        .start_slot(Icon::new(IconName::Plus).size(IconSize::XSmall).color(Color::Accent))
                        .child(Label::new(strings.habit_add_section).size(LabelSize::Small).color(Color::Accent))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let strings = i18n::t(cx);
                            let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_section_name, cx));
                            cx.subscribe(&input, |this, _, Submitted(text), cx| {
                                if let Ok(section) = this.store.add_habit_section(text) {
                                    this.sections = this.store.habit_sections().unwrap_or_default();
                                    this.notify_data_changed(cx);
                                    this.update_edit(cx, |e| {
                                        e.section = HabitSection::Custom(section.id);
                                        e.new_section = None;
                                        e.open = None;
                                    });
                                }
                            })
                            .detach();
                            cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
                                this.update_edit(cx, |e| e.new_section = None);
                            })
                            .detach();
                            window.focus(&input.focus_handle(cx), cx);
                            this.update_edit(cx, |e| e.new_section = Some(input));
                        }))
                        .into_any_element(),
                })
            })
    }

    fn render_icon_picker(&self, edit: &HabitEdit, tab: usize, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(276.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.update_edit(cx, |e| e.icon_picker = None);
            }))
            .child(
                h_flex().gap_1().children((0..3).map(|idx| {
                    let selected = idx == tab;
                    div()
                        .id(SharedString::from(format!("icon-tab-{idx}")))
                        .flex_1()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .bg(if selected { theme.colors().accent.opacity(0.18) } else { theme.colors().surface_background })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let focus = (idx == 2).then(|| this.editing.as_ref().map(|e| e.icon_text.focus_handle(cx))).flatten();
                            this.update_edit(cx, |e| e.icon_picker = Some(idx));
                            if let Some(focus) = focus {
                                window.focus(&focus, cx);
                            }
                        }))
                        .child(
                            Label::new(strings.habit_icon_tabs[idx])
                                .size(LabelSize::XSmall)
                                .color(if selected { Color::Accent } else { Color::Default }),
                        )
                })),
            )
            // Color palette strip
            .child(
                h_flex().flex_wrap().gap_1p5().children(lists_panel::LIST_COLORS.iter().enumerate().map(|(i, hex)| {
                    let selected = edit.color.as_deref() == Some(*hex);
                    let color = lists_panel::parse_color(Some(hex)).unwrap_or(theme.colors().accent);
                    div()
                        .id(SharedString::from(format!("icon-color-{i}")))
                        .size(px(20.))
                        .rounded_full()
                        .bg(color)
                        .border_2()
                        .border_color(if selected { theme.colors().text } else { gpui::transparent_black() })
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_edit(cx, |e| e.color = Some((*hex).to_owned()));
                        }))
                })),
            )
            .child(match tab {
                2 => h_flex()
                    .h(px(30.))
                    .px_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme.colors().border)
                    .bg(theme.colors().background)
                    .child(edit.icon_text.clone())
                    .into_any_element(),
                _ => div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(HABIT_EMOJI.iter().enumerate().map(|(i, emoji)| {
                        div()
                            .id(SharedString::from(format!("icon-emoji-{i}")))
                            .size(px(30.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.colors().element_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_edit(cx, |e| {
                                    e.icon = Some((*emoji).to_owned());
                                    e.icon_picker = None;
                                });
                            }))
                            .child(*emoji)
                    }))
                    .into_any_element(),
            })
    }
}

fn next_hour_time() -> NaiveTime {
    let next_hour = (Local::now().hour() + 1) % 24;
    NaiveTime::from_hms_opt(next_hour, 0, 0).unwrap_or_default()
}

pub(crate) fn day_unit_str(days: i64, strings: &i18n::Strings) -> &'static str {
    if days <= 1 { strings.habit_unit_day } else { strings.habit_unit_days }
}
