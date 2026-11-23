use std::collections::HashSet;

use chrono::{Datelike, Local, NaiveDate};
use fern_core::habit::{
    self, CheckinStatus, Habit, MAX_EDITABLE_PAST_DAYS, format_amount, goal_cycles, month_stats_with, stats_with,
};
use gpui::FontWeight;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use desktop_common::habit_helpers::mood_emoji;
use desktop_common::{calendar, layout};
use fern_i18n as i18n;

use crate::dialogs::CompletedCyclesModal;
use crate::modal::day_unit_str;
use crate::{ConfirmAction, DetailSubmenu, HabitView, STREAK_BEST_SETTING, StartFocus, avatar};

impl HabitView {
    pub(crate) fn render_detail(&self, habit: &Habit, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let status_colors = theme.status().clone();
        let id = habit.id;
        let week_start = calendar::week_start();
        let empty = HashSet::new();
        let checked = self.checkins.get(&id).unwrap_or(&empty);
        let stats = stats_with(&habit.frequency, checked, today, week_start);
        let month_stats = month_stats_with(
            &habit.frequency,
            checked,
            self.month,
            today,
            habit.start_date,
            habit.archived_date,
            week_start,
        );
        let no_records = Vec::new();
        let records = self.records.get(&id).unwrap_or(&no_records);
        let (monthly_done, total_done) = habit::completion(records, self.month);
        let cycles = goal_cycles(habit, checked);
        let in_month = |date: NaiveDate| date.year() == self.month.year() && date.month() == self.month.month();

        let logs: Vec<_> = records
            .iter()
            .rev()
            .filter(|c| in_month(c.date) && c.has_log())
            .cloned()
            .collect();
        let goal_unit: SharedString = habit.goal.as_ref().map_or(SharedString::default(), |g| g.unit.clone().into());
        let show_best_streak = self.flag(STREAK_BEST_SETTING, false);

        let stat_card = |icon: IconName, color: Hsla, label: &'static str, value: String, unit: SharedString| {
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
                        .child(Label::new(label).size(LabelSize::Small).truncate()),
                )
                .child(
                    h_flex()
                        .items_end()
                        .gap_1()
                        .child(div().text_size(px(20.)).line_height(px(24.)).font_weight(FontWeight::MEDIUM).child(value))
                        .child(div().pb(px(3.)).child(Label::new(unit).size(LabelSize::Small).color(Color::Muted))),
                )
        };

        let streak_val = if show_best_streak { stats.best_streak } else { stats.streak };
        let streak_label = if show_best_streak { strings.habit_best_streak } else { strings.habit_current_streak };
        let alt_streak_tip = if show_best_streak {
            format!("{}: {}", strings.habit_current_streak, stats.streak)
        } else {
            format!("{}: {}", strings.habit_best_streak, stats.best_streak)
        };
        let streak_card = v_flex()
            .id("habit-streak-toggle")
            .flex_1()
            .h(px(68.))
            .px_3()
            .justify_center()
            .gap_0p5()
            .rounded(px(8.))
            .bg(theme.colors().surface_background)
            .cursor_pointer()
            .hover(|s| s.bg(theme.colors().element_hover))
            .tooltip(Tooltip::text(alt_streak_tip))
            .on_click(cx.listener(|this, _, _, cx| {
                let next = !this.flag(STREAK_BEST_SETTING, false);
                this.set_flag(STREAK_BEST_SETTING, next, cx);
            }))
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::Flame).size(IconSize::Small).color(Color::Custom(status_colors.warning)))
                    .child(Label::new(streak_label).size(LabelSize::Small).truncate())
                    .child(Icon::new(IconName::ArrowDownUp).size(IconSize::XSmall).color(Color::Muted)),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap_1()
                    .child(div().text_size(px(20.)).line_height(px(24.)).font_weight(FontWeight::MEDIUM).child(streak_val.to_string()))
                    .child(div().pb(px(3.)).child(Label::new(day_unit_str(streak_val as i64, strings)).size(LabelSize::Small).color(Color::Muted))),
            );

        let weekdays = h_flex().children(calendar::weekday_labels(&strings.weekdays_short).iter().map(|day| {
            div().flex_1().flex().justify_center().child(Label::new(*day).size(LabelSize::Small).color(Color::Muted))
        }));

        let is_archived = habit.archived;
        let has_goal = habit.goal.is_some();
        let grid: Vec<AnyElement> = calendar::month_grid(self.month)
            .chunks(7)
            .map(|week| {
                h_flex()
                    .children(week.iter().map(|&date| {
                        let in_this_month = date.month() == self.month.month();
                        let future = date > today;
                        let before_start = habit.start_date.is_some_and(|s| date < s);
                        let after_archive = habit.archived_date.is_some_and(|a| date > a);
                        let blocked = is_archived || future;
                        let dimmed = !in_this_month || future || before_start || after_archive;
                        let date_label = calendar::format_date(date, strings);
                        let record = records.iter().find(|r| r.date == date);
                        let day_tip: SharedString = if let Some(goal) = habit.goal.as_ref() {
                            let amt = record.map(|r| r.value).unwrap_or(0.0);
                            if amt > 0.0 {
                                format!("{date_label} · {}/{} {}", format_amount(amt), format_amount(goal.amount), goal.unit).into()
                            } else if checked.contains(&date) {
                                format!("{date_label} · {}", strings.habit_checked_in).into()
                            } else {
                                date_label.into()
                            }
                        } else if checked.contains(&date) {
                            format!("{date_label} · {}", strings.habit_checked_in).into()
                        } else if record.is_some_and(|r| r.is_undone()) {
                            format!("{date_label} · {}", strings.habit_unachieved).into()
                        } else {
                            date_label.into()
                        };
                        v_flex()
                            .flex_1()
                            .items_center()
                            .gap_1()
                            .py_1()
                            .when(dimmed, |el| el.opacity(0.45))
                            .child(
                                Label::new(date.day().to_string())
                                    .size(LabelSize::Small)
                                    .color(if date == today { Color::Accent } else { Color::Default }),
                            )
                            .child(
                                self.checkin_mark(habit, date, px(26.), cx)
                                    .id(SharedString::from(format!("detail-checkin-{date}")))
                                    .tooltip(ui::Tooltip::text(day_tip))
                                    .when(!blocked, |el| {
                                        el.cursor_pointer()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.handle_detail_calendar_click(id, date, has_goal, window, cx);
                                            }))
                                            .on_mouse_down(
                                                gpui::MouseButton::Right,
                                                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    this.open_day_menu(id, date, ev.position, cx);
                                                }),
                                            )
                                    }),
                            )
                    }))
                    .into_any_element()
            })
            .collect();

        let month_label = format!("{} {}", strings.months[self.month.month0() as usize], self.month.year());
        let chart_el = habit.goal.as_ref().map(|goal| self.render_daily_goals_chart(habit, goal.amount, records, cx));

        v_scroll(
            "habit-detail-scroll",
            v_flex()
                .id("habit-detail")
                .px_4()
                .pb_6()
                .gap_3()
                // Header
                .child(
                    h_flex()
                        .mt(layout::header_inset())
                        .h(px(52.))
                        .flex_none()
                        .gap_2()
                        .child(avatar(habit, px(30.), cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    Label::new(habit.name.clone())
                                        .size(LabelSize::Large)
                                        .weight(FontWeight::SEMIBOLD)
                                        .truncate(),
                                ),
                        )
                        .when(!is_archived, |el| {
                            let done_today = checked.contains(&today);
                            el.child(
                                h_flex()
                                    .id("habit-detail-checkin-today")
                                    .h(px(28.))
                                    .px_2p5()
                                    .gap_1()
                                    .rounded(px(6.))
                                    .cursor_pointer()
                                    .bg(if done_today {
                                        status_colors.success.opacity(0.16)
                                    } else {
                                        theme.colors().accent.opacity(0.14)
                                    })
                                    .hover(|s| {
                                        s.bg(if done_today {
                                            status_colors.success.opacity(0.24)
                                        } else {
                                            theme.colors().accent.opacity(0.22)
                                        })
                                    })
                                    .child(
                                        Icon::new(if done_today { IconName::CircleCheck } else { IconName::Check })
                                            .size(IconSize::XSmall)
                                            .color(if done_today {
                                                Color::Custom(status_colors.success)
                                            } else {
                                                Color::Accent
                                            }),
                                    )
                                    .child(
                                        Label::new(if done_today { strings.habit_checked_in } else { strings.check_in })
                                            .size(LabelSize::XSmall)
                                            .weight(FontWeight::MEDIUM)
                                            .color(if done_today {
                                                Color::Custom(status_colors.success)
                                            } else {
                                                Color::Accent
                                            }),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.handle_detail_calendar_click(id, today, has_goal, window, cx);
                                    })),
                            )
                        })
                        .child(
                            div()
                                .relative()
                                .child(
                                    IconButton::new("habit-detail-more", IconName::Ellipsis)
                                        .tooltip(ui::Tooltip::text(strings.more))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.detail_menu_open = true;
                                            cx.notify();
                                        })),
                                )
                                .when(self.detail_menu_open, |el| {
                                    el.child(self.render_detail_menu(habit, cx))
                                }),
                        ),
                )
                // Optional notice (e.g. >90 days limit)
                .when_some(self.detail_notice.clone(), |el, notice| {
                    el.child(
                        h_flex()
                            .px_3()
                            .py_1p5()
                            .rounded(px(6.))
                            .bg(theme.colors().surface_background)
                            .justify_between()
                            .child(Label::new(notice).size(LabelSize::XSmall).color(Color::Muted))
                            .child(
                                IconButton::new("notice-dismiss", IconName::X)
                                    .icon_size(IconSize::XSmall)
                                    .tooltip(ui::Tooltip::text(strings.dismiss))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.detail_notice = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                })
                // Stats grid (2 columns x 2 or 3 rows)
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(stat_card(
                                    IconName::CalendarCheck,
                                    theme.colors().accent,
                                    strings.habit_monthly_checkins,
                                    month_stats.days.to_string(),
                                    day_unit_str(month_stats.days as i64, strings).into(),
                                ))
                                .child(stat_card(
                                    IconName::CircleCheck,
                                    status_colors.success,
                                    strings.habit_total_checkins,
                                    stats.total_days.to_string(),
                                    day_unit_str(stats.total_days as i64, strings).into(),
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(stat_card(
                                    IconName::Percent,
                                    status_colors.info,
                                    strings.habit_monthly_rate,
                                    month_stats.rate.to_string(),
                                    "%".into(),
                                ))
                                .child(streak_card),
                        )
                        .when(habit.goal.is_some(), |el| {
                            el.child(
                                h_flex()
                                    .gap_2()
                                    .child(stat_card(
                                        IconName::ChartNoAxesColumn,
                                        theme.colors().accent,
                                        strings.habit_monthly_completion,
                                        format_amount(monthly_done),
                                        goal_unit.clone(),
                                    ))
                                    .child(stat_card(
                                        IconName::Zap,
                                        status_colors.success,
                                        strings.habit_total_completion,
                                        format_amount(total_done),
                                        goal_unit.clone(),
                                    )),
                            )
                        }),
                )
                // Habit Cycles card (only for finite goal_days)
                .when_some(cycles, |el, summary| {
                    let pct = (summary.current_count as f32 / summary.target_days.max(1) as f32).clamp(0.0, 1.0);
                    let progress_text = strings
                        .habit_cycle_progress
                        .replace("{done}", &summary.current_count.to_string())
                        .replace("{target}", &summary.target_days.to_string());
                    let completed_count = summary.completed.len();
                    let completed_label = strings
                        .habit_cycle_completed_count
                        .replace("{n}", &completed_count.to_string());
                    let habit_name = habit.name.clone();
                    let completed_cycles = summary.completed;
                    el.child(
                        v_flex()
                            .id("habit-cycles-card")
                            .p_3()
                            .gap_2()
                            .rounded(px(8.))
                            .bg(theme.colors().surface_background)
                            .when(completed_count > 0, |card| {
                                card.cursor_pointer()
                                    .hover(|s| s.bg(theme.colors().element_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.cycles_modal = Some(CompletedCyclesModal {
                                            habit_name: habit_name.clone(),
                                            cycles: completed_cycles.clone(),
                                        });
                                        cx.notify();
                                    }))
                            })
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(Label::new(strings.habit_cycles).size(LabelSize::Small).weight(FontWeight::MEDIUM))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .when(completed_count > 0, |row| {
                                                row.child(Label::new(format!("🏆 {completed_label}")).size(LabelSize::XSmall).color(Color::Accent))
                                            })
                                            .child(Label::new(progress_text).size(LabelSize::XSmall).color(Color::Muted)),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(6.))
                                    .w_full()
                                    .rounded_full()
                                    .bg(theme.colors().border.opacity(0.5))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(gpui::relative(pct))
                                            .rounded_full()
                                            .bg(theme.colors().accent),
                                    ),
                            ),
                    )
                })
                // Monthly calendar card
                .child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .id("habit-month-jump")
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.month = calendar::month_start(Local::now().date_naive());
                                            cx.notify();
                                        }))
                                        .child(Label::new(month_label).weight(FontWeight::MEDIUM)),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .when(self.month != calendar::month_start(today), |el| {
                                            el.child(
                                                div()
                                                    .id("habit-month-today")
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded(px(4.))
                                                    .bg(theme.colors().accent.opacity(0.12))
                                                    .hover(|s| s.bg(theme.colors().accent.opacity(0.2)))
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.month = calendar::month_start(Local::now().date_naive());
                                                        cx.notify();
                                                    }))
                                                    .child(Label::new(strings.today).size(LabelSize::XSmall).color(Color::Accent)),
                                            )
                                        })
                                        .child(
                                            IconButton::new("habit-month-prev", IconName::ChevronLeft)
                                                .tooltip(ui::Tooltip::text("←"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.month = calendar::add_months(this.month, -1);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            IconButton::new("habit-month-next", IconName::ChevronRight)
                                                .tooltip(ui::Tooltip::text("→"))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.month = calendar::add_months(this.month, 1);
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                        .child(weekdays)
                        .children(grid),
                )
                // Daily Goals bar chart (quantifiable habits)
                .children(chart_el)
                // Habit Log timeline
                .child(
                    v_flex()
                        .p_3()
                        .gap_2()
                        .rounded(px(8.))
                        .bg(theme.colors().surface_background)
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    Label::new(format!(
                                        "{}{}",
                                        strings.habit_log_on,
                                        strings.months[self.month.month0() as usize]
                                    ))
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::MEDIUM),
                                )
                                .when(!is_archived, |el| {
                                    el.child(
                                        IconButton::new("habit-log-add", IconName::Plus)
                                            .icon_size(IconSize::XSmall)
                                            .tooltip(ui::Tooltip::text(strings.habit_record))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                let target_date = this.selected_date.unwrap_or(today);
                                                this.open_log_modal(id, target_date, false, window, cx);
                                            })),
                                    )
                                }),
                        )
                        .when(logs.is_empty(), |el| {
                            el.child(Label::new(strings.habit_log_empty).size(LabelSize::XSmall).color(Color::Muted))
                        })
                        .children(logs.into_iter().map(|entry| {
                            let entry_date = entry.date;
                            let is_expanded = self.expanded_logs.contains(&(id, entry_date));
                            let note_text = entry.note.clone().unwrap_or_default();
                            let is_long = note_text.chars().count() > 140 || note_text.lines().count() > 3;
                            let shown_note = if is_long && !is_expanded {
                                let truncated: String = note_text.chars().take(140).collect();
                                format!("{truncated}…")
                            } else {
                                note_text
                            };
                            let dot_color = match entry.status {
                                CheckinStatus::Done => theme.colors().accent,
                                CheckinStatus::Undone => status_colors.error,
                                CheckinStatus::Unlabeled => theme.colors().text_muted,
                            };
                            v_flex()
                                .gap_1()
                                .p_2()
                                .rounded(px(6.))
                                .bg(theme.colors().background)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .items_center()
                                                .child(div().size(px(8.)).rounded_full().bg(dot_color))
                                                .child(
                                                    Label::new(calendar::short_date(entry_date, strings))
                                                        .size(LabelSize::XSmall)
                                                        .color(Color::Muted),
                                                )
                                                .when_some(mood_emoji(entry.mood), |row, emoji| {
                                                    row.child(div().text_size(px(14.)).child(emoji))
                                                }),
                                        )
                                        .when(!is_archived, |row| {
                                            row.child(
                                                h_flex()
                                                    .gap_0p5()
                                                    .child(
                                                        IconButton::new(
                                                            SharedString::from(format!("log-edit-{entry_date}")),
                                                            IconName::Pencil,
                                                        )
                                                        .icon_size(IconSize::XSmall)
                                                        .tooltip(ui::Tooltip::text(strings.edit))
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            this.open_log_modal(id, entry_date, false, window, cx);
                                                        })),
                                                    )
                                                    .child(
                                                        IconButton::new(
                                                            SharedString::from(format!("log-del-{entry_date}")),
                                                            IconName::Trash,
                                                        )
                                                        .icon_size(IconSize::XSmall)
                                                        .hover_icon_color(theme.status().error)
                                                        .tooltip(ui::Tooltip::text(strings.delete))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.confirm = Some(ConfirmAction::DeleteLog {
                                                                habit_id: id,
                                                                date: entry_date,
                                                            });
                                                            cx.notify();
                                                        })),
                                                    ),
                                            )
                                        }),
                                )
                                .when(!shown_note.is_empty(), |el| {
                                    el.child(Label::new(shown_note).size(LabelSize::Small))
                                })
                                .when(is_long, |el| {
                                    el.child(
                                        div()
                                            .id(SharedString::from(format!("log-toggle-{entry_date}")))
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let key = (id, entry_date);
                                                if !this.expanded_logs.remove(&key) {
                                                    this.expanded_logs.insert(key);
                                                }
                                                cx.notify();
                                            }))
                                            .child(
                                                Label::new(if is_expanded {
                                                    strings.habit_collapse
                                                } else {
                                                    strings.habit_expand
                                                })
                                                .size(LabelSize::XSmall)
                                                .color(Color::Accent),
                                            ),
                                    )
                                })
                        })),
                ),
        )
        .size_full()
        .into_any_element()
    }

    fn render_detail_menu(&self, habit: &Habit, cx: &mut Context<Self>) -> PopoverAttachment {
        let strings = i18n::t(cx);
        let id = habit.id;
        let name = habit.name.clone();
        let archived = habit.archived;
        let icon_checkins = habit.icon_checkins;
        let focus_bounds = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
        let style_bounds = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
        let focus_bounds_out = focus_bounds.clone();
        let style_bounds_out = style_bounds.clone();

        PopoverAttachment::new(
            Placement::BelowRight,
            popover(cx)
                .min_w(px(184.))
                .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if focus_bounds_out.get().is_some_and(|b| b.contains(&event.position))
                        || style_bounds_out.get().is_some_and(|b| b.contains(&event.position))
                    {
                        return;
                    }
                    this.detail_submenu = None;
                    this.detail_menu_open = false;
                    cx.notify();
                }))
                .child(self.menu_entry(
                    "habit-edit-entry",
                    ListItem::new("habit-edit")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                        .child(Label::new(strings.edit_habit))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_edit_modal(id, window, cx))),
                    cx,
                ))
                .child({
                    let name_pomo = name.clone();
                    let name_sw = name;
                    SubmenuItem::new("habit-start-focus", strings.start_focus)
                        .icon(IconName::FernFocus)
                        .height(px(34.))
                        .open(self.detail_submenu == Some(DetailSubmenu::StartFocus))
                        .track_bounds(focus_bounds)
                        .on_hover(cx.listener(|this, hovered, _, cx| {
                            if *hovered && this.detail_submenu != Some(DetailSubmenu::StartFocus) {
                                this.detail_submenu = Some(DetailSubmenu::StartFocus);
                                cx.notify();
                            }
                        }))
                        .items([
                            ListItem::new("habit-start-pomo")
                                .height(px(34.))
                                .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                                .child(Label::new(strings.habit_start_pomo))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.detail_menu_open = false;
                                    this.detail_submenu = None;
                                    cx.emit(StartFocus {
                                        habit_id: Some(id),
                                        title: name_pomo.clone(),
                                        stopwatch: false,
                                    });
                                    cx.notify();
                                })),
                            ListItem::new("habit-start-sw")
                                .height(px(34.))
                                .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                                .child(Label::new(strings.habit_start_stopwatch))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.detail_menu_open = false;
                                    this.detail_submenu = None;
                                    cx.emit(StartFocus {
                                        habit_id: Some(id),
                                        title: name_sw.clone(),
                                        stopwatch: true,
                                    });
                                    cx.notify();
                                })),
                        ])
                })
                .child(
                    SubmenuItem::new("habit-checkin-style", strings.checked_in_style)
                        .icon(IconName::Smile)
                        .height(px(34.))
                        .open(self.detail_submenu == Some(DetailSubmenu::CheckinStyle))
                        .track_bounds(style_bounds)
                        .on_hover(cx.listener(|this, hovered, _, cx| {
                            if *hovered && this.detail_submenu != Some(DetailSubmenu::CheckinStyle) {
                                this.detail_submenu = Some(DetailSubmenu::CheckinStyle);
                                cx.notify();
                            }
                        }))
                        .items([
                            ListItem::new("style-check")
                                .height(px(34.))
                                .child(Label::new(strings.checkin_style_checkmark))
                                .when(!icon_checkins, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.store.set_habit_icon_checkins(id, false).ok();
                                    this.detail_menu_open = false;
                                    this.detail_submenu = None;
                                    this.notify_data_changed(cx);
                                    this.reload(cx);
                                })),
                            ListItem::new("style-icon")
                                .height(px(34.))
                                .child(Label::new(strings.checkin_style_icon))
                                .when(icon_checkins, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.store.set_habit_icon_checkins(id, true).ok();
                                    this.detail_menu_open = false;
                                    this.detail_submenu = None;
                                    this.notify_data_changed(cx);
                                    this.reload(cx);
                                })),
                        ]),
                )
                .child(self.menu_entry(
                    "habit-archive-entry",
                    ListItem::new("habit-archive")
                        .height(px(34.))
                        .start_slot(
                            Icon::new(if archived { IconName::ArchiveRestore } else { IconName::Archive })
                                .color(Color::Muted),
                        )
                        .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_archived(id, !archived, cx))),
                    cx,
                ))
                .child(self.menu_entry(
                    "habit-delete-entry",
                    ListItem::new("habit-delete")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete).color(Color::Error))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.detail_menu_open = false;
                            this.detail_submenu = None;
                            this.confirm = Some(ConfirmAction::DeleteHabit { habit_id: id });
                            cx.notify();
                        })),
                    cx,
                )),
        )
    }

    pub(crate) fn handle_detail_calendar_click(
        &mut self,
        habit_id: fern_core::Id,
        date: NaiveDate,
        has_goal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.habits.iter().find(|h| h.id == habit_id).is_none_or(|h| h.archived) {
            return;
        }
        let today = Local::now().date_naive();
        if date > today {
            return;
        }
        if (today - date).num_days() > MAX_EDITABLE_PAST_DAYS {
            self.detail_notice = Some(i18n::t(cx).habit_past_limit.to_owned());
            cx.notify();
            return;
        }
        self.detail_notice = None;
        if has_goal {
            self.open_amount_modal(habit_id, date, false, window, cx);
        } else {
            self.handle_dot_click(habit_id, date, window, cx);
        }
    }

    fn render_daily_goals_chart(
        &self,
        habit: &Habit,
        goal_amount: f64,
        records: &[fern_core::Checkin],
        cx: &App,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let first = calendar::month_start(self.month);
        let next = calendar::add_months(first, 1);
        let days_in_month = (next - first).num_days().max(1) as usize;

        let mut values = vec![0.0f64; days_in_month];
        let mut highest = 0.0f64;
        for rec in records {
            if rec.date >= first && rec.date < next && !rec.is_undone() {
                let idx = (rec.date - first).num_days() as usize;
                if idx < values.len() {
                    values[idx] = rec.value;
                    highest = highest.max(rec.value);
                }
            }
        }
        let top = habit::chart_top(goal_amount, highest);
        let bar_color = desktop_common::colors::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent);
        let goal_line_color = theme.colors().text_muted.opacity(0.45);
        let track_color = theme.colors().border.opacity(0.25);
        let values_paint = values.clone();
        let unit = habit.goal.as_ref().map(|g| g.unit.as_str()).unwrap_or("");
        let unit_suffix = if unit.is_empty() { String::new() } else { format!(" {unit}") };
        let goal_str = format_amount(goal_amount);
        let hover_bg = theme.colors().element_hover.opacity(0.35);

        v_flex()
            .p_3()
            .gap_2()
            .rounded(px(8.))
            .bg(theme.colors().surface_background)
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new(strings.habit_daily_goals).size(LabelSize::Small).weight(FontWeight::MEDIUM))
                    .child(
                        Label::new(format!("{goal_str}{unit_suffix}"))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                div()
                    .relative()
                    .h(px(110.))
                    .w_full()
                    .child(
                        gpui::canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let n = values_paint.len().max(1);
                                let slot_w = bounds.size.width / n as f32;
                                let bar_w = (slot_w * 0.6).clamp(px(3.), px(14.));
                                let chart_h = bounds.size.height - px(4.);
                                if chart_h <= px(4.) {
                                    return;
                                }
                                // Baseline
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        gpui::point(bounds.origin.x, bounds.origin.y + chart_h),
                                        gpui::size(bounds.size.width, px(1.)),
                                    ),
                                    track_color,
                                ));
                                // Goal reference line
                                let goal_ratio = (goal_amount / top).clamp(0.0, 1.0) as f32;
                                let goal_y = bounds.origin.y + chart_h * (1.0 - goal_ratio);
                                window.paint_quad(gpui::fill(
                                    gpui::Bounds::new(
                                        gpui::point(bounds.origin.x, goal_y),
                                        gpui::size(bounds.size.width, px(1.)),
                                    ),
                                    goal_line_color,
                                ));
                                // Bars
                                for (i, &val) in values_paint.iter().enumerate() {
                                    if val <= 0.0 {
                                        continue;
                                    }
                                    let ratio = (val / top).clamp(0.0, 1.0) as f32;
                                    let h = (chart_h * ratio).max(px(3.));
                                    let x = bounds.origin.x + slot_w * i as f32 + (slot_w - bar_w) * 0.5;
                                    let y = bounds.origin.y + chart_h - h;
                                    let fill = if val >= goal_amount { bar_color } else { bar_color.opacity(0.55) };
                                    window.paint_quad(
                                        gpui::fill(gpui::Bounds::new(gpui::point(x, y), gpui::size(bar_w, h)), fill)
                                            .corner_radii(gpui::Corners::all(px(3.))),
                                    );
                                }
                            },
                        )
                        .size_full(),
                    )
                    .child(h_flex().absolute().inset_0().children((0..days_in_month).map(|i| {
                        let day = first + chrono::Duration::days(i as i64);
                        let tip = format!(
                            "{} · {}/{}{}",
                            calendar::format_date(day, strings),
                            format_amount(values[i]),
                            goal_str,
                            unit_suffix
                        );
                        div()
                            .id(SharedString::from(format!("habit-goal-bar-{i}")))
                            .flex_1()
                            .h_full()
                            .rounded(px(3.))
                            .hover(move |s| s.bg(hover_bg))
                            .tooltip(Tooltip::text(tip))
                    }))),
            )
            .child(
                h_flex().justify_between().children([1, 5, 10, 15, 20, 25, days_in_month].into_iter().map(|d| {
                    Label::new(d.to_string()).size(LabelSize::XSmall).color(Color::Muted)
                })),
            )
            .into_any_element()
    }
}
