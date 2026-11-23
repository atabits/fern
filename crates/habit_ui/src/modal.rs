use chrono::{Local, NaiveDate, NaiveTime, Timelike};
use fern_core::HabitFrequency;
use fern_core::habit::{
    CheckinMode, Goal, Habit, HabitDraft, HabitSection, MAX_REMINDERS, format_amount, parse_amount,
};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Button, ButtonVariant, Modal, Placement, PopoverAttachment};

use desktop_common::calendar;
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use fern_i18n as i18n;

use crate::{ConfirmAction, HabitView, PendingFocus, avatar};

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

impl HabitEdit {
    /// Closes the active nested popover or inline input inside the modal, returning `true` if one was open.
    pub(crate) fn dismiss_popover(&mut self) -> bool {
        if self.icon_picker.take().is_some() || self.reminder_picker.take().is_some() {
            return true;
        }
        if self.new_section.take().is_some() || self.goal_days_input.take().is_some() {
            return true;
        }
        if self.open.take().is_some() {
            self.start_calendar = None;
            return true;
        }
        self.start_calendar.take().is_some()
    }

    pub(crate) fn any_goal_input_focused(&self, window: &Window, cx: &App) -> bool {
        [&self.amount_input, &self.step_input, &self.unit]
            .into_iter()
            .any(|input| input.focus_handle(cx).is_focused(window))
    }
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
        let reminder_input = self.make_reminder_input(window, cx);
        let icon_text = self.make_icon_text_input(window, cx);
        self.wire_edit_inputs(&name, &amount_input, &step_input, &unit, window, cx);

        let mut reminders = Vec::new();
        let mut auto_reminder = false;
        if let Some(time) = section.default_reminder() {
            reminders.push(time);
            auto_reminder = true;
        }

        self.pending_focus = None;
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
        let reminder_input = self.make_reminder_input(window, cx);
        let icon_text = self.make_icon_text_input(window, cx);
        self.wire_edit_inputs(&name, &amount_input, &step_input, &unit, window, cx);

        let auto_reminder = habit.reminders.len() == 1
            && habit.section.default_reminder() == Some(habit.reminders[0]);

        self.pending_focus = None;
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

    pub(crate) fn close_edit_modal(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        self.editing = None;
        self.focus_or_defer(PendingFocus::View, window, cx);
        cx.notify();
    }

    fn wire_edit_inputs(
        &self,
        name: &Entity<TextInput>,
        amount_input: &Entity<TextInput>,
        step_input: &Entity<TextInput>,
        unit: &Entity<TextInput>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(name, window, |this, _, _: &Submitted, window, cx| {
            this.request_save_edit(cx);
            this.flush_pending_focus(window, cx);
        })
        .detach();
        for input in [name, amount_input, step_input, unit] {
            cx.subscribe_in(input, window, |this, _, _: &Cancelled, window, cx| {
                let closed_popover = this.editing.as_mut().is_some_and(HabitEdit::dismiss_popover);
                if closed_popover {
                    this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
                    cx.notify();
                } else {
                    this.close_edit_modal(Some(window), cx);
                }
            })
            .detach();
        }
    }

    fn make_reminder_input(&self, window: &Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        let reminder_input = cx.new(|cx| TextInput::new(Mode::SingleLine, "09:00", cx));
        cx.subscribe_in(&reminder_input, window, |this, input, Submitted(text), window, cx| {
            this.commit_reminder_text(text, input, Some(window), cx);
        })
        .detach();
        cx.subscribe_in(&reminder_input, window, |this, input, _: &Cancelled, window, cx| {
            input.update(cx, |input, cx| input.set_text("", cx));
            this.update_edit(cx, |e| e.reminder_picker = None);
            this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
        })
        .detach();
        reminder_input
    }

    pub(crate) fn open_reminder_picker(&mut self, slot: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
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
            self.pending_focus = None;
            window.focus(&input.focus_handle(cx), cx);
        }
        self.update_edit(cx, |e| {
            e.open = None;
            e.reminder_picker = Some(slot);
        });
    }

    pub(crate) fn apply_reminder_time(
        &mut self,
        slot: Option<usize>,
        time: NaiveTime,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
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
        self.focus_or_defer(PendingFocus::EditName, window, cx);
    }

    pub(crate) fn commit_reminder_text(
        &mut self,
        text: &str,
        _input: &Entity<TextInput>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(time) = calendar::parse_time(text) else { return };
        let slot = self.editing.as_ref().and_then(|e| e.reminder_picker).flatten();
        self.apply_reminder_time(slot, time, window, cx);
    }

    fn make_icon_text_input(&self, window: &Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        let strings = i18n::t(cx);
        let icon_text = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_text_placeholder, cx));
        cx.subscribe_in(&icon_text, window, |this, _, Submitted(text), window, cx| {
            let text: String = text.trim().chars().take(2).collect();
            this.update_edit(cx, |e| {
                e.icon = (!text.is_empty()).then_some(text);
                e.icon_picker = None;
            });
            this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
        })
        .detach();
        cx.subscribe_in(&icon_text, window, |this, _, _: &Cancelled, window, cx| {
            this.update_edit(cx, |e| e.icon_picker = None);
            this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
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
        self.pending_focus = Some(PendingFocus::View);
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
                .on_click(cx.listener(move |this, _, window, cx| {
                    let restore = which == EditRow::Goal
                        && this.edit_input_focused(|e| {
                            e.open == Some(EditRow::Goal) && e.any_goal_input_focused(window, cx)
                        });
                    this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_edit_modal(Some(window), cx);
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
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let restore = this.edit_input_focused(|e| {
                                            e.icon_picker.is_some() && e.icon_text.focus_handle(cx).is_focused(window)
                                        });
                                        this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                let restore = this.edit_input_focused(|e| {
                                                    e.reminder_picker == Some(Some(idx))
                                                        && e.reminder_input.focus_handle(cx).is_focused(window)
                                                });
                                                this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_edit_modal(Some(window), cx);
                            })),
                    )
                    .child(
                        Button::new("habit-edit-save", strings.save)
                            .variant(ButtonVariant::Primary)
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.request_save_edit(cx);
                                this.flush_pending_focus(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn next_hour_time() -> NaiveTime {
    let next_hour = (Local::now().hour() + 1) % 24;
    NaiveTime::from_hms_opt(next_hour, 0, 0).unwrap_or_default()
}

pub(crate) fn day_unit_str(days: i64, strings: &i18n::Strings) -> &'static str {
    if days <= 1 { strings.habit_unit_day } else { strings.habit_unit_days }
}
