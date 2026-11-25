use std::rc::Rc;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::{Id, Task};
use gpui::FontWeight;
use ui::prelude::*;
use ui::popover;

use crate::calendar;
use crate::calendar_options_view::{CalendarOptionsDialog, OptionsEvent, Page};
use crate::i18n;
use crate::task_rows;

use super::{CARD_HEIGHT, CARD_WIDTH, CalendarMode, CalendarView, EventBar, OpenTaskPicker};

const MULTI_DAYS: std::ops::RangeInclusive<u8> = 1..=14;
const MULTI_WEEKS: std::ops::RangeInclusive<u8> = 2..=6;

pub(super) struct ContextMenuState {
    pub(super) id: Id,
    pub(super) position: gpui::Point<Pixels>,
    pub(super) task: Task,
    pub(super) nesting: crate::task_menu::TaskNesting,
}

pub(super) struct PendingDeleteEvent {
    pub(super) subscription_id: Id,
    pub(super) uid: String,
    pub(super) title: String,
    pub(super) calendar: String,
}

impl CalendarView {
    /// An event's card: its title, when it is, its calendar, place and description.
    pub(super) fn render_event_card(&self, event: &EventBar, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let day = |date: NaiveDate| task_rows::format_day(strings.closed_date[0], date, strings);
        let when = match (event.start_time, event.end_time) {
            (Some(from), Some(to)) if event.start == event.end => format!(
                "{}, {} – {}",
                day(event.start),
                calendar::format_time(from, strings, cx),
                calendar::format_time(to, strings, cx)
            ),
            (Some(from), Some(to)) => format!(
                "{} {} – {} {}",
                day(event.start),
                calendar::format_time(from, strings, cx),
                day(event.end),
                calendar::format_time(to, strings, cx)
            ),
            (Some(from), None) => format!("{}, {}", day(event.start), calendar::format_time(from, strings, cx)),
            (None, _) if event.start == event.end => format!("{}, {}", day(event.start), strings.range_all_day),
            (None, _) => format!("{} – {}", day(event.start), day(event.end)),
        };
        let sub_id = event.subscription_id;
        let uid = event.key.clone();
        let delete_title = event.title.clone();
        let delete_calendar = event.calendar.clone();
        let edit_snapshot = event.clone();
        popover(cx)
            .w(px(300.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_event = None;
                cx.notify();
            }))
            .child(Label::new(event.title.clone()).weight(FontWeight::SEMIBOLD))
            .child(Label::new(when).size(LabelSize::Small).color(Color::Muted))
            .child(
                h_flex()
                    .gap_1p5()
                    .child(div().size(px(8.)).rounded_full().bg(event.color))
                    .child(Label::new(event.calendar.clone()).size(LabelSize::Small).color(Color::Muted)),
            )
            .when(!event.location.trim().is_empty(), |el| {
                el.child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(event.location.clone()).size(LabelSize::Small)),
                )
            })
            .when(!event.notes.trim().is_empty(), |el| {
                el.child(
                    v_scroll(
                        "calendar-event-notes-scroll",
                        div()
                            .id("calendar-event-notes")
                            .text_size(px(12.))
                            .child(event.notes.trim().to_owned()),
                    )
                    .max_h(px(160.))
                    .compact(),
                )
            })
            .when(event.recurring, |el| {
                el.child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted))
                        .child(Label::new(strings.filter_date_recurring).size(LabelSize::Small).color(Color::Muted)),
                )
            })
            .when(event.writable, |el| {
                el.child(div().h(px(1.)).bg(theme.colors().border_variant)).child(
                    h_flex()
                        .justify_end()
                        .gap_1p5()
                        .when(!event.recurring, |el| {
                            el.child(
                                IconButton::new("calendar-event-edit", IconName::Pencil)
                                    .tip(strings.cal_edit_event, ui::TipSide::Above)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_edit_event_editor(&edit_snapshot, window, cx);
                                    })),
                            )
                        })
                        .child(
                            div()
                                .debug_selector(|| "calendar-event-delete".to_owned())
                                .child(
                                    IconButton::new("calendar-event-delete", IconName::Trash)
                                        .icon_color(Color::Error)
                                        .tip(strings.delete, ui::TipSide::Above)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.request_delete_caldav_event(
                                                sub_id,
                                                uid.clone(),
                                                delete_title.clone(),
                                                delete_calendar.clone(),
                                                cx,
                                            );
                                        })),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_habit_popover(
        &self,
        habit: &fern_core::Habit,
        date: NaiveDate,
        cx: &mut Context<Self>,
    ) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let rec = self
            .habit_records
            .get(&id)
            .and_then(|recs| recs.iter().find(|r| r.date == date));
        let done = rec.is_some_and(|r| r.is_done());
        let value = rec.map_or(0.0, |r| r.value);
        let goal_amt = habit.goal_amount();
        let unit = habit.goal.as_ref().map(|g| g.unit.clone()).unwrap_or_default();
        let pct = (value / goal_amt).clamp(0.0, 1.0) as f32;
        let has_goal = habit.goal.is_some();
        let step = habit.goal.as_ref().map_or(1.0, |g| g.step_amount());

        popover(cx)
            .w(px(236.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_habit = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .id(SharedString::from(format!("cal-habit-title-{id}")))
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_habit = None;
                        cx.emit(super::OpenHabit(id));
                        cx.notify();
                    }))
                    .child(desktop_common::habit_helpers::avatar(habit, px(24.), cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Label::new(habit.name.clone()).weight(FontWeight::SEMIBOLD).truncate()),
                    )
                    .child(Icon::new(IconName::ChevronRight).size(IconSize::XSmall).color(Color::Muted)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(
                            format!(
                                "{} / {} {}",
                                fern_core::habit::format_amount(value),
                                fern_core::habit::format_amount(goal_amt),
                                unit
                            )
                            .trim()
                            .to_owned(),
                        )
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    )
                    .child(
                        div()
                            .h(px(5.))
                            .w_full()
                            .rounded_full()
                            .bg(theme.colors().border.opacity(0.5))
                            .child(div().h_full().w(gpui::relative(pct)).rounded_full().bg(theme.colors().accent)),
                    ),
            )
            .when(!habit.archived, |el| {
                let btn_label = if done {
                    strings.habit_reset.to_owned()
                } else if has_goal {
                    format!("+{}", fern_core::habit::format_amount(step))
                } else {
                    strings.habit_i_did_it.to_owned()
                };
                el.child(
                    h_flex()
                        .gap_2()
                        .pt_1()
                        .child(
                            Button::new(SharedString::from(format!("cal-habit-act-{id}-{date}")), btn_label)
                                .variant(if done { ButtonVariant::Secondary } else { ButtonVariant::Primary })
                                .size(ButtonSize::Sm)
                                .full_width()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let res = if done {
                                        this.store.reset_checkin(id, date, true)
                                    } else if has_goal {
                                        this.store.step_checkin(id, date).map(|_| ())
                                    } else {
                                        this.store.toggle_checkin(id, date).map(|_| ())
                                    };
                                    if let Err(err) = res {
                                        desktop_common::logging::log_error(
                                            "calendar_ui",
                                            format!("failed to update habit from calendar: {err:#}"),
                                        );
                                    }
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                    this.reload(cx);
                                })),
                        ),
                )
            })
    }

    pub(super) fn open_options(&mut self, page: Page, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let dialog = cx.new(|cx| CalendarOptionsDialog::new(store, page, cx));
        cx.subscribe(&dialog, |this, _, event: &OptionsEvent, cx| {
            match event {
                OptionsEvent::Changed(options) => this.options = options.clone(),
                OptionsEvent::Closed => this.options_dialog = None,
            }
            this.reload(cx);
        })
        .detach();
        self.options_dialog = Some(dialog);
        self.more_menu = false;
        cx.notify();
    }

    /// The header's "…" menu, as the original's: View Options and Arrange Tasks.
    pub(super) fn render_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let row = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id)
                .height(px(34.))
                .start_slot(Icon::new(icon).color(Color::Muted))
                .child(Label::new(label))
        };
        popover(cx)
            .min_w(px(184.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.more_menu = false;
                cx.notify();
            }))
            .child(
                row("calendar-view-options", IconName::Settings2, strings.view_options)
                    .on_click(cx.listener(|this, _, _, cx| this.open_options(Page::Main, cx))),
            )
            .child(row("calendar-arrange", IconName::ListChecks, strings.arrange_tasks).on_click(cx.listener(|this, _, _, cx| {
                this.more_menu = false;
                this.arrange = Some(this.arrange.unwrap_or(0));
                this.reload(cx);
            })))
            .when(self.mode == CalendarMode::Year, |el| {
                el.child(
                    row("calendar-go-to-date", IconName::CalendarDays, strings.calendar_go_to_date)
                        .on_click(cx.listener(|this, _, window, cx| this.open_year_jump(window, cx))),
                )
                .child(
                    row("calendar-print-year", IconName::Printer, strings.print)
                        .on_click(cx.listener(|this, _, _, cx| this.print_year(cx))),
                )
            })
            .into_any_element()
    }

    /// Opens a task's card beside it, the task as clicked on `day`.
    pub(super) fn open_card(&mut self, id: Id, day: NaiveDate, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) {
            return;
        }
        let Ok(Some(task)) = self.store.task(id) else { return };
        let lists = self.lists.clone();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(Some(task), cx);
        });
        self.card = Some((id, day));
        cx.notify();
    }

    pub(super) fn close_card(&mut self, cx: &mut Context<Self>) {
        if self.card.take().is_some() {
            self.detail.update(cx, |detail, cx| detail.show(None, cx));
            cx.notify();
        }
    }

    /// The card a clicked task opens: its details, framed, closed by a click elsewhere or Escape.
    pub(super) fn render_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        div()
            .id("calendar-task-card")
            .occlude()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.colors().border)
            .bg(theme.colors().background)
            .shadow_lg()
            // The details' own popovers lie outside the card; a click in them keeps it open.
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if !this.detail.read(cx).has_popup() {
                    this.close_card(cx);
                }
            }))
            .child(self.detail.clone())
            .into_any_element()
    }

    /// Opens a task's right-click menu, snapshotting task & nesting state outside `render()`.
    pub(super) fn open_context(&mut self, id: Id, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        if !self.finish_task_before_navigation(cx) || self.is_task_read_only(id) {
            return;
        }
        let Some(task) = self.store.task(id).ok().flatten() else {
            return;
        };
        let nesting = crate::task_menu::TaskNesting::of(&self.store, task.id);
        self.context_menu = Some(ContextMenuState {
            id,
            position,
            task,
            nesting,
        });
        self.context_side = None;
        cx.notify();
    }

    /// A choice in a task's right-click menu (see `task_menu`): edits apply here; the pickers
    /// open the task in the list.
    fn context_action(&mut self, action: crate::task_menu::MenuAction, cx: &mut Context<Self>) {
        use crate::task_menu::MenuAction;
        let Some(id) = self.context_menu.as_ref().map(|menu| menu.id) else { return };
        if let MenuAction::Side(side) = action {
            self.context_side = side;
            cx.notify();
            return;
        }
        self.context_menu = None;
        self.context_side = None;
        if self.is_task_read_only(id) {
            cx.notify();
            return;
        }
        let Some(task) = self.store.task(id).ok().flatten() else {
            cx.notify();
            return;
        };
        if let Some(result) = crate::task_menu::apply(&self.store, &task, action, Local::now().date_naive()) {
            if let Err(err) = result {
                desktop_common::logging::log_error(
                    "calendar_ui",
                    format!("failed to change the task: {err:#}"),
                );
            }
            crate::data_version::data_changed(cx.entity_id(), cx);
            self.card_updated(cx);
            return;
        }
        match action {
            MenuAction::Due(None) => cx.emit(OpenTaskPicker(id, "due")),
            MenuAction::LinkParent => cx.emit(OpenTaskPicker(id, "link-parent")),
            MenuAction::Tags => cx.emit(OpenTaskPicker(id, "tags")),
            MenuAction::AddSubtask => cx.emit(OpenTaskPicker(id, "subtask")),
            MenuAction::Focus(stopwatch) => cx.emit(crate::task_detail::StartFocus(id, task.title, stopwatch)),
            MenuAction::CopyLink => cx.write_to_clipboard(gpui::ClipboardItem::new_string(fern_core::link::task_link(id))),
            MenuAction::StickyNote => crate::sticky_opener::open_sticky_note(self.store.clone(), id, cx),
            MenuAction::Delete => {
                if let Err(err) = self.store.delete_task(id) {
                    desktop_common::logging::log_error(
                        "calendar_ui",
                        format!("failed to delete the task: {err:#}"),
                    );
                }
                crate::data_version::data_changed(cx.entity_id(), cx);
                self.card_updated(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The task's right-click menu, as the original's calendar has it (no Add Subtask or Pin).
    pub(super) fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.context_menu.as_ref()?;
        let menu = crate::task_menu::render(
            &state.task,
            &self.lists,
            crate::task_menu::MenuKind::Calendar,
            state.nesting,
            self.context_side,
            i18n::t(cx),
            cx,
            Rc::new(|this: &mut Self, action, _, cx| this.context_action(action, cx)),
        );
        Some(crate::task_menu::place(state.position, menu))
    }

    pub(super) fn request_delete_caldav_event(
        &mut self,
        subscription_id: Id,
        uid: String,
        title: String,
        calendar: String,
        cx: &mut Context<Self>,
    ) {
        self.open_event = None;
        self.pending_delete_event = Some(PendingDeleteEvent {
            subscription_id,
            uid,
            title,
            calendar,
        });
        cx.notify();
    }

    fn delete_caldav_event(&mut self, subscription_id: Id, uid: &str, cx: &mut Context<Self>) {
        self.open_event = None;
        self.pending_delete_event = None;
        if let Some(feeds) = crate::subscriptions::feeds(cx) {
            feeds.update(cx, |feeds, cx| {
                if let Err(err) = feeds.delete_event(subscription_id, uid, cx) {
                    desktop_common::logging::log_error(
                        "calendar_ui",
                        format!("failed to delete CalDAV event: {err:#}"),
                    );
                }
            });
        }
        cx.notify();
    }

    pub(super) fn render_delete_event_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.pending_delete_event.as_ref()?;
        let strings = i18n::t(cx);
        let title = if pending.title.trim().is_empty() {
            strings.untitled
        } else {
            pending.title.as_str()
        };
        let sub_id = pending.subscription_id;
        let uid = pending.uid.clone();
        let mut dialog = ConfirmDialog::new(
            "calendar-delete-event-modal",
            format!("{} \"{}\"?", strings.delete, title),
            strings.cancel,
            strings.delete,
        )
        .width(px(360.))
        .destructive(true)
        .on_cancel(cx.listener(|this, _, window, cx| {
            this.pending_delete_event = None;
            window.focus(&this.focus_handle, cx);
            cx.notify();
        }))
        .on_confirm(cx.listener(move |this, _, window, cx| {
            this.pending_delete_event = None;
            this.delete_caldav_event(sub_id, &uid, cx);
            window.focus(&this.focus_handle, cx);
        }));
        if !pending.calendar.trim().is_empty() {
            dialog = dialog.description(pending.calendar.clone());
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .when(cfg!(test), |el| {
                    el.debug_selector(|| "calendar-delete-event-modal".to_owned())
                })
                .child(dialog)
                .into_any_element(),
        )
    }

    pub(super) fn render_month_picker_popover(
        &self,
        picker_year: i32,
        shown_month: NaiveDate,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors();
        popover(cx)
            .debug_selector(|| "calendar-month-picker".to_owned())
            .w(px(236.))
            .p_3()
            .gap_3()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.month_picker_open = None;
                cx.notify();
            }))
            .child(
                h_flex()
                    .px_1()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .child(Label::new(picker_year.to_string()).weight(FontWeight::SEMIBOLD)),
                    )
                    .child(
                        IconButton::new("month-picker-prev-year", IconName::ChevronLeft)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text((picker_year - 1).max(1910).to_string()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(year) = this.month_picker_open.as_mut() {
                                    *year = (*year - 1).max(1910);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        IconButton::new("month-picker-this-year", IconName::Circle)
                            .size(px(24.))
                            .icon_size(IconSize::Custom(px(8.)))
                            .tooltip(ui::Tooltip::text(today.year().to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.month_picker_open = Some(today.year());
                                cx.notify();
                            })),
                    )
                    .child(
                        IconButton::new("month-picker-next-year", IconName::ChevronRight)
                            .size(px(24.))
                            .icon_size(IconSize::Small)
                            .tooltip(ui::Tooltip::text((picker_year + 1).min(2099).to_string()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(year) = this.month_picker_open.as_mut() {
                                    *year = (*year + 1).min(2099);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .children((0..3).map(|row| {
                h_flex().gap_1().children((0..4).map(|col| {
                    let month_index = row * 4 + col;
                    let month_num = (month_index + 1) as u32;
                    let active = shown_month.year() == picker_year && shown_month.month() == month_num;
                    let is_current = today.year() == picker_year && today.month() == month_num;
                    div()
                        .flex_1()
                        .h(px(40.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .id(SharedString::from(format!("calendar-month-pick-{picker_year}-{month_num}")))
                                .debug_selector(move || format!("calendar-month-pick-{picker_year}-{month_num}"))
                                .size(px(38.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .when(active, |el| el.bg(colors.accent))
                                .when(!active, |el| el.hover(|s| s.bg(colors.element_hover)))
                                .child(
                                    Label::new(strings.months_short[month_index])
                                        .centered()
                                        .when(active || is_current, |l| l.weight(FontWeight::SEMIBOLD))
                                        .color(if active {
                                            Color::OnAccent
                                        } else if is_current {
                                            Color::Accent
                                        } else {
                                            Color::Default
                                        }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_month(picker_year, month_num, cx);
                                })),
                        )
                }))
            }))
            .into_any_element()
    }

    pub(super) fn render_mode_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let items = [
            (CalendarMode::Day, strings.day, "D/1"),
            (CalendarMode::Week, strings.week, "W/2"),
            (CalendarMode::Month, strings.month, "M/3"),
            (CalendarMode::Year, strings.year, "Y/4"),
            (CalendarMode::Agenda, strings.agenda, "A/5"),
        ];

        popover(cx)
            .debug_selector(|| "calendar-mode-menu".to_owned())
            .min_w(px(192.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.mode_menu_open = false;
                cx.notify();
            }))
            .children(items.into_iter().map(|(mode, label, hint)| {
                self.mode_item(mode, label, cx)
                    .end_slot(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted))
            }))
            .child(div().py_1().child(Divider::horizontal()))
            .child(self.render_multi_mode_item(CalendarMode::MultiDay(self.multi_days), cx))
            .child(self.render_multi_mode_item(CalendarMode::MultiWeek(self.multi_weeks), cx))
            .into_any_element()
    }

    fn mode_item(&self, mode: CalendarMode, label: &'static str, cx: &Context<Self>) -> ListItem {
        let selected = self.mode == mode;
        ListItem::new(label)
            .start_slot(div().w(px(14.)).when(selected, |el| {
                el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::Accent))
            }))
            .child(Label::new(label).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
            .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
    }

    fn render_multi_mode_item(&self, mode: CalendarMode, cx: &Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let (group, label, count, hint, limits, mode_with_count) = match mode {
            CalendarMode::MultiDay(days) => (
                "calendar-multi-day",
                strings.multi_day,
                days,
                if days == 1 {
                    strings.one_day.to_owned()
                } else {
                    strings.n_days.replace("{n}", &days.to_string())
                },
                MULTI_DAYS,
                CalendarMode::MultiDay as fn(u8) -> CalendarMode,
            ),
            CalendarMode::MultiWeek(weeks) => (
                "calendar-multi-week",
                strings.multi_week,
                weeks,
                strings.n_weeks.replace("{n}", &weeks.to_string()),
                MULTI_WEEKS,
                CalendarMode::MultiWeek as fn(u8) -> CalendarMode,
            ),
            _ => unreachable!("only multi-day and multi-week have count controls"),
        };
        let step = |id: &'static str, icon: IconName, delta: i8| {
            let next = count.checked_add_signed(delta).filter(|next| limits.contains(next));
            div().debug_selector(|| format!("{group}-{id}")).child(
                IconButton::new(id, icon)
                    .size(px(20.))
                    .icon_size(IconSize::XSmall)
                    .when(next.is_none(), |button| button.icon_color(Color::Placeholder))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(next) = next {
                            this.apply_mode(mode_with_count(next), cx);
                        }
                    })),
            )
        };
        let touch = crate::touch::active();
        let count_controls = div()
            .relative()
            .flex_none()
            .child(
                // Keep the controls in the layout so hover only changes visibility.
                h_flex()
                    .when(!touch, |el| el.invisible().group_hover(group, |style| style.visible()))
                    .child(step("fewer", IconName::Minus, -1))
                    .child(
                        div()
                            .w(px(22.))
                            .flex()
                            .justify_center()
                            .child(Label::new(count.to_string()).size(LabelSize::Small)),
                    )
                    .child(step("more", IconName::Plus, 1)),
            )
            .when(!touch, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .group_hover(group, |style| style.invisible())
                        .child(Label::new(hint).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                )
            });
        div()
            .id(group)
            .group(group)
            .debug_selector(|| group.to_owned())
            .child(self.mode_item(mode, label, cx).end_slot(count_controls))
    }
}
