use chrono::NaiveDate;
use fern_core::habit::{
    CheckinStatus, HabitCycle, HabitDraft, MAX_LOG_NOTE_CHARS, MOODS, format_amount, parse_amount,
    parse_amount_non_negative,
};
use fern_core::Id;
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Button, ButtonVariant, ConfirmDialog, Modal};

use desktop_common::habit_helpers::mood_emoji;
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use fern_i18n as i18n;

use crate::{HabitView, PendingFocus};

/// Modal for creating or editing a day's Habit Log (mood + note).
pub(crate) struct HabitLogModal {
    pub habit_id: Id,
    pub date: NaiveDate,
    pub mood: u8,
    pub note_input: Entity<TextInput>,
    pub auto_popped: bool,
    pub disable_auto_pop: bool,
}

/// Modal for entering a numeric check-in amount ("Increase" vs "Total").
pub(crate) struct AmountModal {
    pub habit_id: Id,
    pub date: NaiveDate,
    /// `true` adds to existing day value (manual card check-in); `false` sets the day's total.
    pub increase: bool,
    pub input: Entity<TextInput>,
}

/// Confirmation modal actions.
pub(crate) enum ConfirmAction {
    ResetCheckin { habit_id: Id, date: NaiveDate },
    DeleteHabit { habit_id: Id },
    DeleteLog { habit_id: Id, date: NaiveDate },
    DeleteSection { section_id: Id },
    SaveHabitCycleChange { id: Id, draft: HabitDraft },
}

/// Congratulation modal when finishing a Goal Days cycle.
pub(crate) struct CycleCompletedModal {
    pub habit_id: Id,
    pub target_days: u32,
}

/// Modal listing all completed Goal Days cycles for a habit.
pub(crate) struct CompletedCyclesModal {
    pub habit_name: String,
    pub cycles: Vec<HabitCycle>,
}

impl HabitView {
    pub(crate) fn open_log_modal(
        &mut self,
        habit_id: Id,
        date: NaiveDate,
        auto_popped: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let strings = i18n::t(cx);
        let existing = self
            .records
            .get(&habit_id)
            .and_then(|recs| recs.iter().find(|r| r.date == date))
            .cloned();
        let default_mood = match existing.as_ref() {
            Some(r) if r.mood != 0 => r.mood,
            Some(r) if r.status == CheckinStatus::Done => 50,
            _ => 10,
        };
        let initial_note = existing.and_then(|r| r.note).unwrap_or_default();
        let note_input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::MultiLine, strings.habit_note_placeholder, cx);
            input.set_text(initial_note, cx);
            input
        });
        cx.subscribe_in(&note_input, window, |this, _, _: &Cancelled, window, cx| {
            this.log_modal = None;
            this.focus_or_defer(PendingFocus::View, Some(window), cx);
            cx.notify();
        })
        .detach();
        self.pending_focus = None;
        window.focus(&note_input.focus_handle(cx), cx);
        self.log_modal = Some(HabitLogModal {
            habit_id,
            date,
            mood: default_mood,
            note_input,
            auto_popped,
            disable_auto_pop: false,
        });
        self.day_menu = None;
        self.card_menu = None;
        cx.notify();
    }

    pub(crate) fn save_log_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.log_modal.take() else { return };
        self.pending_focus = Some(PendingFocus::View);
        let note = modal.note_input.read(cx).text().to_owned();
        if let Err(err) = self.store.set_checkin_log(modal.habit_id, modal.date, Some(&note), modal.mood) {
            eprintln!("failed to save habit log: {err:#}");
        }
        if modal.disable_auto_pop
            && let Err(err) = self.store.set_habit_record_enable(modal.habit_id, false)
        {
            eprintln!("failed to disable auto-pop habit log: {err:#}");
        }
        self.notify_data_changed(cx);
        self.reload(cx);
    }

    pub(crate) fn open_amount_modal(
        &mut self,
        habit_id: Id,
        date: NaiveDate,
        increase: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.habits.iter().find(|h| h.id == habit_id).is_none_or(|h| h.archived) {
            return;
        }
        let existing_val = self
            .records
            .get(&habit_id)
            .and_then(|recs| recs.iter().find(|r| r.date == date))
            .map_or(0.0, |r| r.value);
        let initial = if increase {
            String::new()
        } else if existing_val > 0.0 {
            format_amount(existing_val)
        } else {
            String::new()
        };
        let input = cx.new(|cx| {
            let mut inp = TextInput::new(Mode::SingleLine, "0", cx);
            inp.set_text(initial, cx);
            inp
        });
        cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| {
            this.submit_amount_modal(Some(window), cx);
        })
        .detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.amount_modal = None;
            this.focus_or_defer(PendingFocus::View, Some(window), cx);
            cx.notify();
        })
        .detach();
        self.pending_focus = None;
        window.focus(&input.focus_handle(cx), cx);
        self.amount_modal = Some(AmountModal { habit_id, date, increase, input });
        self.day_menu = None;
        self.card_menu = None;
        cx.notify();
    }

    pub(crate) fn submit_amount_modal(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(modal) = self.amount_modal.as_ref() else { return };
        let habit_id = modal.habit_id;
        let date = modal.date;
        let increase = modal.increase;
        let raw = modal.input.read(cx).text().to_owned();
        let parsed = if increase {
            parse_amount(&raw)
        } else {
            parse_amount_non_negative(&raw)
        };
        let Some(amount) = parsed else {
            // Preserve the open modal and typed input so the user can fix a typo.
            cx.notify();
            return;
        };
        self.amount_modal = None;
        self.pending_focus = Some(PendingFocus::View);
        let was_done = self.checkins.get(&habit_id).is_some_and(|d| d.contains(&date));
        let res = if increase {
            self.store.add_checkin_amount(habit_id, date, amount).map(|_| ())
        } else {
            self.store.set_checkin_amount(habit_id, date, amount)
        };
        if let Err(err) = res {
            eprintln!("failed to save check-in amount: {err:#}");
            cx.notify();
            return;
        }
        self.notify_data_changed(cx);
        self.reload(cx);
        let now_done = self.checkins.get(&habit_id).is_some_and(|d| d.contains(&date));
        if !was_done && now_done {
            self.after_checkin_completed(habit_id, date, window.as_deref_mut(), cx);
        }
        if let Some(window) = window {
            self.flush_pending_focus(window, cx);
        }
    }

    pub(crate) fn render_log_modal(&self, modal: &HabitLogModal, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let habit_name = self
            .habits
            .iter()
            .find(|h| h.id == modal.habit_id)
            .map(|h| h.name.clone())
            .unwrap_or_default();
        let char_count = modal.note_input.read(cx).text().chars().count();

        Modal::new("habit-log-modal")
            .width(px(420.))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Label::new(format!(
                                    "{}{}",
                                    strings.habit_log_on,
                                    desktop_common::calendar::format_date(modal.date, strings)
                                ))
                                .size(LabelSize::Large)
                                .weight(FontWeight::SEMIBOLD),
                            )
                            .child(Label::new(habit_name).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .child(
                        IconButton::new("log-modal-close", IconName::X)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.log_modal = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                cx.notify();
                            })),
                    ),
            )
            // Mood picker (5 emojis)
            .child(
                v_flex()
                    .gap_1p5()
                    .child(Label::new(strings.habit_mood_prompt).size(LabelSize::XSmall).color(Color::Muted))
                    .child(h_flex().gap_2().children(MOODS.map(|m| {
                        let selected = modal.mood == m;
                        let emoji = mood_emoji(m).unwrap_or("");
                        div()
                            .id(SharedString::from(format!("log-mood-{m}")))
                            .size(px(36.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(20.))
                            .cursor_pointer()
                            .bg(if selected {
                                theme.colors().accent.opacity(0.22)
                            } else {
                                theme.colors().surface_background
                            })
                            .border_1()
                            .border_color(if selected {
                                theme.colors().accent
                            } else {
                                gpui::transparent_black()
                            })
                            .hover(|s| s.bg(theme.colors().accent.opacity(if selected { 0.28 } else { 0.12 })))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(lm) = this.log_modal.as_mut() {
                                    lm.mood = if lm.mood == m { 0 } else { m };
                                    cx.notify();
                                }
                            }))
                            .child(emoji)
                    }))),
            )
            // Note text area
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .h(px(110.))
                            .p_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .bg(theme.colors().background)
                            .child(modal.note_input.clone()),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Label::new(format!("{char_count}/{MAX_LOG_NOTE_CHARS}"))
                                .size(LabelSize::XSmall)
                                .color(if char_count > MAX_LOG_NOTE_CHARS {
                                    Color::Error
                                } else {
                                    Color::Muted
                                }),
                        ),
                    ),
            )
            // Auto-pop opt-out
            .when(modal.auto_popped, |el| {
                el.child(
                    h_flex()
                        .id("log-no-auto-pop")
                        .gap_2()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(lm) = this.log_modal.as_mut() {
                                lm.disable_auto_pop = !lm.disable_auto_pop;
                                cx.notify();
                            }
                        }))
                        .child(Checkbox::new("log-no-auto-pop-box").toggle_state(modal.disable_auto_pop))
                        .child(Label::new(strings.habit_do_not_show_again).size(LabelSize::XSmall).color(Color::Muted)),
                )
            })
            // Footer
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("log-modal-cancel", strings.cancel)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.log_modal = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("log-modal-save", strings.save)
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_log_modal(cx);
                                this.flush_pending_focus(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_amount_modal(&self, modal: &AmountModal, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let habit = self.habits.iter().find(|h| h.id == modal.habit_id);
        let unit = habit
            .and_then(|h| h.goal.as_ref())
            .map(|g| g.unit.clone())
            .unwrap_or_default();
        let goal_amt = habit.map(|h| h.goal_amount()).unwrap_or(1.0);
        let current_val = self
            .records
            .get(&modal.habit_id)
            .and_then(|recs| recs.iter().find(|r| r.date == modal.date))
            .map_or(0.0, |r| r.value);
        let mode_title = if modal.increase { strings.habit_increase } else { strings.habit_total };
        let raw = modal.input.read(cx).text();
        let invalid = !raw.trim().is_empty()
            && if modal.increase {
                parse_amount(raw).is_none()
            } else {
                parse_amount_non_negative(raw).is_none()
            };

        Modal::new("habit-amount-modal")
            .width(px(340.))
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new(strings.habit_how_much).size(LabelSize::Default).weight(FontWeight::SEMIBOLD))
                    .child(
                        IconButton::new("amt-close", IconName::X)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.amount_modal = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Label::new(format!(
                    "{mode_title} · {} / {} {unit}",
                    format_amount(current_val),
                    format_amount(goal_amt)
                ))
                .size(LabelSize::XSmall)
                .color(Color::Muted),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .flex_1()
                            .h(px(34.))
                            .px_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(if invalid { theme.status().error } else { theme.colors().accent })
                            .bg(theme.colors().background)
                            .child(modal.input.clone()),
                    )
                    .when(!unit.is_empty(), |el| el.child(Label::new(unit).size(LabelSize::Small).color(Color::Muted))),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("amt-cancel", strings.cancel)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.amount_modal = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("amt-save", strings.save)
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(|this, _, window, cx| this.submit_amount_modal(Some(window), cx))),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_confirm_modal(&self, action: &ConfirmAction, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let (title, message, confirm_label, destructive) = match action {
            ConfirmAction::ResetCheckin { .. } => (strings.habit_reset, strings.habit_reset_confirm, strings.habit_reset, false),
            ConfirmAction::DeleteHabit { .. } => (strings.delete, strings.habit_delete_confirm, strings.delete, true),
            ConfirmAction::DeleteLog { .. } => (strings.delete, strings.habit_delete_log_confirm, strings.delete, true),
            ConfirmAction::DeleteSection { .. } => (strings.delete, strings.habit_delete_section_confirm, strings.delete, true),
            ConfirmAction::SaveHabitCycleChange { .. } => (strings.edit_habit, strings.habit_cycle_reset_confirm, strings.save, false),
        };

        ConfirmDialog::new("habit-confirm-modal", title)
            .width(px(360.))
            .description(message)
            .cancel_label(strings.cancel)
            .confirm_label(confirm_label)
            .destructive(destructive)
            .on_cancel(cx.listener(|this, _, window, cx| {
                this.confirm = None;
                let target = if this.editing.is_some() { PendingFocus::EditName } else { PendingFocus::View };
                this.focus_or_defer(target, Some(window), cx);
                cx.notify();
            }))
            .on_confirm(cx.listener(|this, _, window, cx| {
                this.execute_confirm(cx);
                this.flush_pending_focus(window, cx);
            }))
            .into_any_element()
    }

    pub(crate) fn execute_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self.confirm.take() else { return };
        self.pending_focus = Some(PendingFocus::View);
        match action {
            ConfirmAction::ResetCheckin { habit_id, date } => {
                if let Err(err) = self.store.reset_checkin(habit_id, date, true) {
                    eprintln!("failed to reset check-in: {err:#}");
                }
                self.notify_data_changed(cx);
                self.reload(cx);
            }
            ConfirmAction::DeleteHabit { habit_id } => {
                if let Err(err) = self.store.delete_habit(habit_id) {
                    eprintln!("failed to delete habit: {err:#}");
                }
                if self.selected == Some(habit_id) {
                    self.selected = None;
                }
                self.detail_menu_open = false;
                self.notify_data_changed(cx);
                self.reload(cx);
            }
            ConfirmAction::DeleteLog { habit_id, date } => {
                if let Err(err) = self.store.delete_checkin_log(habit_id, date) {
                    eprintln!("failed to delete habit log: {err:#}");
                }
                self.notify_data_changed(cx);
                self.reload(cx);
            }
            ConfirmAction::DeleteSection { section_id } => {
                if let Err(err) = self.store.delete_habit_section(section_id) {
                    eprintln!("failed to delete section: {err:#}");
                }
                self.section_menu = None;
                self.notify_data_changed(cx);
                self.reload(cx);
            }
            ConfirmAction::SaveHabitCycleChange { id, draft } => {
                self.commit_habit_draft(Some(id), draft, cx);
            }
        }
    }

    pub(crate) fn render_cycle_completed_modal(
        &self,
        modal: &CycleCompletedModal,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let habit_id = modal.habit_id;
        let body = strings.habit_cycle_done_body.replace("{n}", &modal.target_days.to_string());

        Modal::new("habit-cycle-done-modal")
            .width(px(380.))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_size(px(22.)).child("🏆"))
                    .child(Label::new(strings.habit_cycle_done_title).size(LabelSize::Large).weight(FontWeight::SEMIBOLD)),
            )
            .child(Label::new(body).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("cycle-archive", strings.habit_end_and_archive)
                            .variant(ButtonVariant::Secondary)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.cycle_completed = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                this.set_archived(habit_id, true, cx);
                            })),
                    )
                    .child(
                        Button::new("cycle-continue", strings.habit_continue_cycle)
                            .variant(ButtonVariant::Primary)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cycle_completed = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_completed_cycles_modal(
        &self,
        modal: &CompletedCyclesModal,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();

        Modal::new("habit-cycles-list-modal")
            .width(px(380.))
            .max_height(px(460.))
            .on_dismiss(cx.listener(|this, _, window, cx| {
                this.cycles_modal = None;
                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                cx.notify();
            }))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(Label::new(strings.habit_cycles).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                            .child(Label::new(modal.habit_name.clone()).size(LabelSize::XSmall).color(Color::Muted)),
                    )
                    .child(
                        IconButton::new("cycles-list-close", IconName::X)
                            .tooltip(Tooltip::text(strings.close))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cycles_modal = None;
                                this.focus_or_defer(PendingFocus::View, Some(window), cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_scroll(
                    "cycles-list-scroll",
                    v_flex().id("cycles-list-body").gap_1p5().children(modal.cycles.iter().rev().enumerate().map(|(i, c)| {
                        h_flex()
                            .px_3()
                            .py_2()
                            .rounded(px(6.))
                            .bg(theme.colors().surface_background)
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(Label::new(format!("#{}", modal.cycles.len() - i)).size(LabelSize::Small).color(Color::Accent))
                                    .child(
                                        Label::new(format!(
                                            "{} — {}",
                                            desktop_common::calendar::format_date(c.start, strings),
                                            desktop_common::calendar::format_date(c.end, strings),
                                        ))
                                        .size(LabelSize::Small),
                                    ),
                            )
                            .child(
                                Label::new(format!("{} {}", c.count, crate::modal::day_unit_str(c.count as i64, strings)))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                    })),
                )
                .max_h(px(300.)),
            )
            .into_any_element()
    }
}
