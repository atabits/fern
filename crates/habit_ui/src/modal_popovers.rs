use chrono::{Datelike, Local, NaiveDate, NaiveTime, Timelike};
use fern_core::HabitFrequency;
use fern_core::habit::{CheckinMode, GOAL_DAYS_PRESETS, HabitSection, MAX_CUSTOM_SECTIONS};
use gpui::{Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Button, ButtonSize, ButtonVariant, popover};

use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use desktop_common::{calendar, colors as lists_panel};
use fern_i18n as i18n;

use crate::modal::{HabitEdit, day_unit_str};
use crate::{HABIT_EMOJI, HabitView, PendingFocus};

impl HabitView {
    pub(crate) fn edit_input_focused(&self, check: impl FnOnce(&HabitEdit) -> bool) -> bool {
        self.editing.as_ref().is_some_and(check)
    }

    pub(crate) fn update_edit_and_restore_name(
        &mut self,
        restore_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut HabitEdit),
    ) {
        self.update_edit(cx, change);
        if restore_focus {
            self.focus_or_defer(PendingFocus::EditName, Some(window), cx);
        }
    }

    pub(crate) fn open_goal_days_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, "1–999", cx));
        cx.subscribe_in(&input, window, |this, _, Submitted(text), window, cx| {
            if let Ok(days) = text.trim().parse::<u32>()
                && (1..=999).contains(&days)
            {
                this.update_edit(cx, |e| {
                    e.goal_days = Some(days);
                    e.goal_days_input = None;
                    e.open = None;
                });
                this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
            }
        })
        .detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.update_edit(cx, |e| e.goal_days_input = None);
            this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
        })
        .detach();
        self.pending_focus = None;
        window.focus(&input.focus_handle(cx), cx);
        self.update_edit(cx, |e| e.goal_days_input = Some(input));
    }

    pub(crate) fn open_new_section_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.habit_section_name, cx));
        cx.subscribe_in(&input, window, |this, _, Submitted(text), window, cx| {
            if let Ok(section) = this.store.add_habit_section(text) {
                this.sections = this.store.habit_sections().unwrap_or_default();
                this.notify_data_changed(cx);
                this.update_edit(cx, |e| {
                    e.section = HabitSection::Custom(section.id);
                    e.new_section = None;
                    e.open = None;
                });
                this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
            }
        })
        .detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.update_edit(cx, |e| e.new_section = None);
            this.focus_or_defer(PendingFocus::EditName, Some(window), cx);
        })
        .detach();
        self.pending_focus = None;
        window.focus(&input.focus_handle(cx), cx);
        self.update_edit(cx, |e| e.new_section = Some(input));
    }

    pub(crate) fn render_reminder_time_picker(
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
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                let restore = this.edit_input_focused(|e| e.reminder_input.focus_handle(cx).is_focused(window));
                this.update_edit_and_restore_name(restore, window, cx, |e| e.reminder_picker = None);
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let text = rem_input.read(cx).text().to_owned();
                                this.commit_reminder_text(&text, &rem_input, Some(window), cx);
                            })),
                    ),
            )
            .child(
                v_scroll(
                    "habit-rem-time-scroll",
                    v_flex().id("habit-rem-time-list").children(slots.into_iter().map(|time| {
                        let selected = current_parsed == Some(time);
                        let label = calendar::format_time(time, strings, cx);
                        ListItem::new(SharedString::from(format!("rem-slot-{:02}{:02}", time.hour(), time.minute())))
                            .height(px(28.))
                            .toggle_state(selected)
                            .child(
                                Label::new(label)
                                    .size(LabelSize::Small)
                                    .color(if selected { Color::Accent } else { Color::Default }),
                            )
                            .when(selected, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_reminder_time(slot, time, Some(window), cx);
                            }))
                    })),
                )
                .max_h(px(210.))
                .compact(),
            )
    }

    pub(crate) fn render_frequency_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
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

    pub(crate) fn render_goal_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
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
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                let restore = this.edit_input_focused(|e| e.any_goal_input_focused(window, cx));
                this.update_edit_and_restore_name(restore, window, cx, |e| e.open = None);
            }))
            .child(
                ListItem::new("goal-opt-all")
                    .height(px(30.))
                    .child(Label::new(strings.habit_goal_all).size(LabelSize::Small))
                    .when(!edit.has_goal, |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let restore = this.edit_input_focused(|e| e.any_goal_input_focused(window, cx));
                        this.update_edit_and_restore_name(restore, window, cx, |e| {
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

    pub(crate) fn render_start_date_popover(&self, edit: &HabitEdit, today: NaiveDate, cx: &mut Context<Self>) -> Div {
        use desktop_common::month_calendar::{
            MonthGridConfig, MonthHeaderConfig, WeekdayStyle, render_month_grid, render_month_header,
            render_weekday_row,
        };

        let strings = i18n::t(cx);
        let shown_month = edit.start_calendar.unwrap_or_else(|| calendar::month_start(edit.start_date.unwrap_or(today)));
        let month_label = format!("{} {}", strings.months[shown_month.month0() as usize], shown_month.year());
        let entity_shift = cx.entity();
        let entity_pick = cx.entity();
        let mut header = MonthHeaderConfig::surface(shown_month, strings, "start-cal-prev", "start-cal-next");
        header.custom_title = Some(month_label);
        header.small_title = true;
        header.title_weight = FontWeight::MEDIUM;

        let mut grid_cfg = MonthGridConfig::single("start", shown_month, today);
        grid_cfg.selected = edit.start_date;
        grid_cfg.cell_height = Some(px(28.));
        grid_cfg.rounded_full = false;
        grid_cfg.label_size = LabelSize::XSmall;
        grid_cfg.fade_outside_with_opacity = true;
        grid_cfg.highlight_today_bg = false;

        let weeks = render_month_grid(&grid_cfg, cx, move |date, _, cx| {
            entity_pick.update(cx, |this, cx| {
                this.update_edit(cx, |e| {
                    e.start_date = Some(date);
                    e.open = None;
                    e.start_calendar = None;
                });
            });
        });

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
            .child(render_month_header(
                header,
                cx,
                move |delta, _, cx| {
                    entity_shift.update(cx, |this, cx| {
                        this.update_edit(cx, |e| {
                            let base = e
                                .start_calendar
                                .unwrap_or_else(|| calendar::month_start(e.start_date.unwrap_or_else(|| Local::now().date_naive())));
                            e.start_calendar = Some(calendar::add_months(base, delta));
                        });
                    });
                },
                None,
            ))
            .child(render_weekday_row(strings, WeekdayStyle::Short, false, cx))
            .children(weeks)
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

    pub(crate) fn render_goal_days_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(210.))
            .p_1()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                let restore = this.edit_input_focused(|e| {
                    e.goal_days_input.as_ref().is_some_and(|i| i.focus_handle(cx).is_focused(window))
                });
                this.update_edit_and_restore_name(restore, window, cx, |e| {
                    e.open = None;
                    e.goal_days_input = None;
                });
            }))
            .child(
                ListItem::new("gd-forever")
                    .height(px(30.))
                    .child(Label::new(strings.habit_forever).size(LabelSize::Small))
                    .when(edit.goal_days.is_none(), |el| el.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let restore = this.edit_input_focused(|e| {
                            e.goal_days_input.as_ref().is_some_and(|i| i.focus_handle(cx).is_focused(window))
                        });
                        this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let restore = this.edit_input_focused(|e| {
                            e.goal_days_input.as_ref().is_some_and(|i| i.focus_handle(cx).is_focused(window))
                        });
                        this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                        this.open_goal_days_input(window, cx);
                    }))
                    .into_any_element(),
            })
    }

    pub(crate) fn render_section_popover(&self, edit: &HabitEdit, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(220.))
            .p_1()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                let restore = this.edit_input_focused(|e| {
                    e.new_section.as_ref().is_some_and(|i| i.focus_handle(cx).is_focused(window))
                });
                this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let restore = this.edit_input_focused(|e| {
                            e.new_section.as_ref().is_some_and(|i| i.focus_handle(cx).is_focused(window))
                        });
                        this.update_edit_and_restore_name(restore, window, cx, |e| {
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
                            e.new_section = None;
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
                            this.open_new_section_input(window, cx);
                        }))
                        .into_any_element(),
                })
            })
    }

    pub(crate) fn render_icon_picker(&self, edit: &HabitEdit, tab: usize, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        popover(cx)
            .w(px(276.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                let restore = this.edit_input_focused(|e| e.icon_text.focus_handle(cx).is_focused(window));
                this.update_edit_and_restore_name(restore, window, cx, |e| e.icon_picker = None);
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
                                this.pending_focus = None;
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let restore = this.edit_input_focused(|e| e.icon_text.focus_handle(cx).is_focused(window));
                                this.update_edit_and_restore_name(restore, window, cx, |e| {
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
