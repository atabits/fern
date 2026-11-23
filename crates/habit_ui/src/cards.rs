use std::collections::HashSet;

use chrono::NaiveDate;
use fern_core::Id;
use fern_core::habit::{Habit, HabitSection, format_amount, stats_with};
use gpui::{FontWeight, Point};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use desktop_common::habit_helpers::progress_ring;
use desktop_common::{calendar, colors as lists_panel};
use fern_i18n as i18n;

use crate::dialogs::ConfirmAction;
use crate::{HabitView, IN_TODAY_SETTING, SORT_SETTING, StartFocus, avatar};

pub(crate) const CARD_WIDTH: f32 = 340.;
pub(crate) const CARD_HEIGHT: f32 = 108.;
pub(crate) const COMPACT_CARD_HEIGHT: f32 = 60.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderMore {
    Menu,
    Settings,
}

#[derive(Clone, Copy)]
pub(crate) struct CardContextMenu {
    pub habit_id: Id,
    pub position: Point<Pixels>,
}

#[derive(Clone, Copy)]
pub(crate) struct DayContextMenu {
    pub habit_id: Id,
    pub date: NaiveDate,
    pub position: Point<Pixels>,
}

#[derive(Clone)]
struct DraggedHabit {
    id: Id,
    name: String,
}

impl Render for DraggedHabit {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().accent)
            .bg(theme.colors().elevated_surface_background)
            .shadow_md()
            .child(Label::new(self.name.clone()).size(LabelSize::Small))
    }
}

#[derive(Clone)]
struct DraggedSection {
    id: Id,
    name: String,
}

impl Render for DraggedSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.colors().accent)
            .bg(theme.colors().elevated_surface_background)
            .shadow_md()
            .child(Label::new(self.name.clone()).size(LabelSize::Small))
    }
}

impl HabitView {
    pub(crate) fn menu_entry(&self, id: &'static str, item: ListItem, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if *hovered && this.detail_submenu.take().is_some() {
                    cx.notify();
                }
            }))
            .child(item)
    }

    /// Renders a single day's check-in dot/ring (Done, Undone, Partial ring, Skipped, Rest, Unchecked).
    pub(crate) fn checkin_mark(&self, habit: &Habit, date: NaiveDate, size: Pixels, cx: &App) -> Div {
        let theme = cx.theme();
        let status_colors = theme.status();
        let accent = lists_panel::parse_color(habit.color.as_deref()).unwrap_or(theme.colors().accent);
        let rec = self.records.get(&habit.id).and_then(|r| r.iter().find(|c| c.date == date));
        let done_set = self.checkins.get(&habit.id);
        let empty = HashSet::new();
        let is_skipped = habit.is_skipped(date);
        let is_due = habit.is_due_on(date, done_set.unwrap_or(&empty), calendar::week_start());

        let base = div().size(size).rounded_full().flex().items_center().justify_center();

        if let Some(c) = rec {
            if c.is_done() {
                if habit.icon_checkins {
                    let glyph = habit
                        .icon
                        .clone()
                        .unwrap_or_else(|| habit.name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default());
                    return base
                        .bg(accent.opacity(0.24))
                        .border_1()
                        .border_color(accent)
                        .text_size(size * 0.55)
                        .child(glyph);
                }
                return base
                    .bg(accent)
                    .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(Color::IconOnAccent));
            }
            if c.is_undone() {
                return base
                    .bg(status_colors.error.opacity(0.16))
                    .border_1()
                    .border_color(status_colors.error.opacity(0.65))
                    .child(Icon::new(IconName::X).size(IconSize::XSmall).color(Color::Error));
            }
            if c.is_partial() {
                return base.relative().child(progress_ring(
                    size,
                    c.progress(),
                    accent,
                    theme.colors().border.opacity(0.55),
                    px(2.5),
                ));
            }
        }

        if is_skipped {
            return base
                .border_1()
                .border_color(theme.colors().border)
                .bg(theme.colors().surface_background.opacity(0.6))
                .child(Icon::new(IconName::Minus).size(IconSize::XSmall).color(Color::Muted));
        }

        if !is_due {
            return base
                .border_1()
                .border_color(theme.colors().border.opacity(0.35))
                .bg(theme.colors().border.opacity(0.12));
        }

        base.border_2().border_color(theme.colors().text_muted.opacity(0.55))
    }

    pub(crate) fn render_header_more(&self, more: HeaderMore, cx: &mut Context<Self>) -> Div {
        let strings = i18n::t(cx);
        let in_today = self.flag(IN_TODAY_SETTING, true);
        let sort_status = self.flag(SORT_SETTING, false);
        match more {
            HeaderMore::Menu => popover(cx)
                .min_w(px(176.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.header_more = None;
                    cx.notify();
                }))
                .child(
                    ListItem::new("habit-open-settings")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Settings).color(Color::Muted))
                        .child(Label::new(strings.habit_settings))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.header_more = Some(HeaderMore::Settings);
                            cx.notify();
                        })),
                )
                .child(
                    ListItem::new("habit-export-csv")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Share).color(Color::Muted))
                        .child(Label::new(strings.export))
                        .on_click(cx.listener(|this, _, _, cx| this.export_csv(cx))),
                ),
            HeaderMore::Settings => {
                let hover_bg = cx.theme().colors().element_hover;
                popover(cx)
                    .w(px(270.))
                    .p_3()
                    .gap_3()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.header_more = None;
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .id("habit-set-in-today")
                            .px_1p5()
                            .py_1()
                            .rounded(px(5.))
                            .hover(move |s| s.bg(hover_bg))
                            .justify_between()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.set_flag(IN_TODAY_SETTING, !in_today, cx)))
                            .child(Label::new(strings.habits_in_today).size(LabelSize::Small))
                            .child(Checkbox::new("habit-in-today-box").toggle_state(in_today)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .id("habit-set-sort")
                                    .px_1p5()
                                    .py_1()
                                    .rounded(px(5.))
                                    .hover(move |s| s.bg(hover_bg))
                                    .justify_between()
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_flag(SORT_SETTING, !sort_status, cx);
                                    }))
                                    .child(Label::new(strings.habits_sort_by_status).size(LabelSize::Small))
                                    .child(Checkbox::new("habit-sort-box").toggle_state(sort_status)),
                            )
                            .child(div().px_1p5().child(Label::new(strings.habits_sort_hint).size(LabelSize::XSmall).color(Color::Muted))),
                    )
            }
        }
    }

    pub(crate) fn render_section_header(
        &self,
        section: HabitSection,
        title: String,
        count: usize,
        allow_add: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let key = section.key();
        let collapsed = self.collapsed_sections.contains(&key);
        let custom_id = match section {
            HabitSection::Custom(id) => Some(id),
            _ => None,
        };
        let is_renaming = custom_id.is_some() && self.renaming_section.as_ref().is_some_and(|(id, _)| Some(*id) == custom_id);

        let key_toggle = key.clone();
        let title_for_drag = title.clone();
        h_flex()
            .id(SharedString::from(format!("habit-sec-hdr-{key}")))
            .h(px(30.))
            .gap_1p5()
            .items_center()
            .rounded(px(6.))
            .when_some(custom_id, |el, cid| {
                el.on_drag(DraggedSection { id: cid, name: title_for_drag.clone() }, |dragged, _, _, cx| {
                    cx.new(|_| dragged.clone())
                })
                .drag_over::<DraggedSection>(|s, _, _, cx| s.border_t_2().border_color(cx.theme().colors().accent))
                .on_drop(cx.listener(move |this, dragged: &DraggedSection, _, cx| {
                    this.reorder_custom_section_drop(dragged.id, cid, cx);
                }))
            })
            .drag_over::<DraggedHabit>(|s, _, _, cx| s.bg(cx.theme().colors().accent.opacity(0.12)))
            .on_drop(cx.listener(move |this, dragged: &DraggedHabit, _, cx| {
                this.reorder_habit_drop(dragged.id, section, None, cx);
            }))
            .child(
                h_flex()
                    .id(SharedString::from(format!("habit-sec-toggle-{key}")))
                    .gap_1()
                    .px_1()
                    .rounded(px(4.))
                    .items_center()
                    .when(!is_renaming, |el| {
                        el.hover(|s| s.bg(theme.colors().ghost_element_hover))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_section_collapsed(key_toggle.clone(), cx);
                            }))
                    })
                    .child(
                        Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(if is_renaming {
                        match &self.renaming_section {
                            Some((_, input)) => h_flex()
                                .w(px(160.))
                                .h(px(24.))
                                .px_1p5()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme.colors().accent)
                                .bg(theme.colors().background)
                                .child(input.clone())
                                .into_any_element(),
                            None => div().into_any_element(),
                        }
                    } else {
                        Label::new(title).size(LabelSize::Small).weight(FontWeight::MEDIUM).color(Color::Muted).into_any_element()
                    })
                    .child(Label::new(count.to_string()).size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(div().flex_1())
            .when(allow_add && !self.show_archived, |el| {
                el.child(
                    IconButton::new(SharedString::from(format!("sec-add-{key}")), IconName::Plus)
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::text(strings.habit_add_title))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_create_modal(section, window, cx);
                        })),
                )
            })
            .when_some(custom_id, |el, cid| {
                el.child(
                    div()
                        .relative()
                        .child(
                            IconButton::new(SharedString::from(format!("sec-more-{cid}")), IconName::Ellipsis)
                                .icon_size(IconSize::XSmall)
                                .tooltip(Tooltip::text(strings.more))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.section_menu = Some(cid);
                                    cx.notify();
                                })),
                        )
                        .when(self.section_menu == Some(cid), |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(140.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.section_menu = None;
                                        cx.notify();
                                    }))
                                    .child(
                                        ListItem::new("sec-rename")
                                            .height(px(32.))
                                            .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                                            .child(Label::new(strings.rename_section))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.start_rename_section(cid, window, cx);
                                            })),
                                    )
                                    .child(
                                        ListItem::new("sec-delete")
                                            .height(px(32.))
                                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                                            .child(Label::new(strings.delete).color(Color::Error))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.section_menu = None;
                                                this.confirm = Some(ConfirmAction::DeleteSection { section_id: cid });
                                                cx.notify();
                                            })),
                                    ),
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    pub(crate) fn render_card(
        &self,
        habit: &Habit,
        week: [NaiveDate; 7],
        today: NaiveDate,
        allow_dnd: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let id = habit.id;
        let empty = HashSet::new();
        let checked = self.checkins.get(&id).unwrap_or(&empty);
        let stats = stats_with(&habit.frequency, checked, today, calendar::week_start());
        let compact = self.selected_date.is_some() || habit.archived;
        let list = self.list_layout;
        let target_single_day = self.selected_date.unwrap_or(today);
        let single_day_rec = self
            .records
            .get(&id)
            .and_then(|r| r.iter().find(|c| c.date == target_single_day));
        let single_day_handled = single_day_rec.is_some_and(|c| c.is_handled());

        let subtitle_el: AnyElement = if let Some(date) = self.selected_date
            && let Some(goal) = habit.goal.as_ref()
        {
            let current = self
                .records
                .get(&id)
                .and_then(|r| r.iter().find(|c| c.date == date))
                .map_or(0.0, |c| c.value);
            Label::new(format!("{}/{} {}", format_amount(current), format_amount(goal.amount), goal.unit))
                .size(LabelSize::XSmall)
                .color(Color::Muted)
                .truncate()
                .into_any_element()
        } else {
            let total_text = if self.selected_date.is_none() && !habit.archived {
                match habit.goal_days {
                    Some(goal) => format!("{}/{goal} {}", stats.total_days, strings.habit_days),
                    None => format!("{} {}", stats.total_days, strings.habit_days),
                }
            } else {
                format!("{} {}", stats.total_days, strings.habit_days)
            };
            let (streak_count, streak_label) = if habit.archived && self.selected_date.is_none() {
                (stats.best_streak, strings.habit_best_streak)
            } else {
                (stats.streak, strings.habit_streak)
            };
            h_flex()
                .gap_2()
                .items_center()
                .overflow_hidden()
                .child(
                    h_flex()
                        .gap_0p5()
                        .items_center()
                        .child(
                            Icon::new(IconName::Zap)
                                .size(IconSize::XSmall)
                                .color(if stats.total_days > 0 { Color::Accent } else { Color::Muted }),
                        )
                        .child(Label::new(total_text).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .child(
                    h_flex()
                        .gap_0p5()
                        .items_center()
                        .child(
                            Icon::new(IconName::Flame)
                                .size(IconSize::XSmall)
                                .color(if streak_count > 0 { Color::Warning } else { Color::Muted }),
                        )
                        .child(Label::new(format!("{streak_count} {streak_label}")).size(LabelSize::XSmall).color(Color::Muted)),
                )
                .into_any_element()
        };

        let habit_section = habit.section;
        let habit_name = habit.name.clone();

        let card_el = div()
            .id(SharedString::from(format!("habit-card-{id}")))
            .flex()
            .map(|el| {
                if list || compact {
                    el.flex_row()
                        .when(list, |el| el.w_full())
                        .when(!list, |el| el.w(px(CARD_WIDTH)))
                        .h(px(COMPACT_CARD_HEIGHT))
                        .px_3()
                        .gap_3()
                        .items_center()
                } else {
                    el.flex_col()
                        .w(px(CARD_WIDTH))
                        .h(px(CARD_HEIGHT))
                        .p_3()
                        .justify_between()
                }
            });

        let header_info = h_flex()
            .gap_2()
            .flex_1()
            .min_w_0()
            .items_center()
            .child(avatar(habit, px(30.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        Label::new(habit.name.clone())
                            .weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .when(self.selected_date.is_some() && single_day_handled, |l| l.color(Color::Muted)),
                    )
                    .child(subtitle_el),
            );

        let dots_el: AnyElement = if let Some(single_day) = self.selected_date {
            self.checkin_mark(habit, single_day, px(24.), cx)
                .id(SharedString::from(format!("checkin-{id}-{single_day}")))
                .when(!habit.archived && single_day <= today, |el| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.handle_dot_click(id, single_day, window, cx);
                        }))
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_day_menu(id, single_day, ev.position, cx);
                            }),
                        )
                })
                .into_any_element()
        } else if habit.archived {
            div().into_any_element()
        } else {
            h_flex()
                .justify_between()
                .px_1()
                .when(list, |el| el.w(px(210.)).flex_none())
                .children(week.map(|date| {
                    let blocked = habit.archived || date > today;
                    self.checkin_mark(habit, date, px(if list { 22. } else { 18. }), cx)
                        .id(SharedString::from(format!("checkin-{id}-{date}")))
                        .when(date > today, |el| el.opacity(0.4))
                        .when(!blocked, |el| {
                            el.cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.handle_dot_click(id, date, window, cx);
                                }))
                                .on_mouse_down(
                                    gpui::MouseButton::Right,
                                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.open_day_menu(id, date, ev.position, cx);
                                    }),
                                )
                        })
                }))
                .into_any_element()
        };

        card_el
            .flex_none()
            .rounded(px(8.))
            .border_1()
            .border_color(if self.selected == Some(id) {
                theme.colors().accent
            } else {
                gpui::transparent_black()
            })
            .bg(theme.colors().surface_background)
            .hover(|s| s.bg(theme.colors().element_hover))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_card_menu(id, ev.position, cx);
                }),
            )
            .when(allow_dnd && !habit.archived, |el| {
                el.on_drag(DraggedHabit { id, name: habit_name.clone() }, |dragged, _, _, cx| {
                    cx.new(|_| dragged.clone())
                })
                .drag_over::<DraggedHabit>(|s, _, _, cx| s.border_color(cx.theme().colors().accent))
                .on_drop(cx.listener(move |this, dragged: &DraggedHabit, _, cx| {
                    if dragged.id != id {
                        this.reorder_habit_drop(dragged.id, habit_section, Some(id), cx);
                    }
                }))
            })
            .child(header_info)
            .child(dots_el)
            .into_any_element()
    }

    pub(crate) fn render_card_context_menu(&self, menu: CardContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let Some(habit) = self.habits.iter().find(|h| h.id == menu.habit_id).cloned() else {
            return div().into_any_element();
        };
        let strings = i18n::t(cx);
        let id = habit.id;
        let name = habit.name.clone();
        let archived = habit.archived;

        div()
            .id("habit-card-ctx")
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .child(
                popover(cx)
                    .min_w(px(176.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.card_menu = None;
                        cx.notify();
                    }))
                    .child(
                        ListItem::new("ctx-habit-edit")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Pencil).color(Color::Muted))
                            .child(Label::new(strings.edit_habit))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_edit_modal(id, window, cx))),
                    )
                    .when(!archived, |el| {
                        let name_pomo = name.clone();
                        let name_sw = name.clone();
                        el.child(
                            ListItem::new("ctx-habit-pomo")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Timer).color(Color::Muted))
                                .child(Label::new(strings.habit_start_pomo))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.card_menu = None;
                                    cx.emit(StartFocus {
                                        habit_id: Some(id),
                                        title: name_pomo.clone(),
                                        stopwatch: false,
                                    });
                                    cx.notify();
                                })),
                        )
                        .child(
                            ListItem::new("ctx-habit-sw")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::Clock).color(Color::Muted))
                                .child(Label::new(strings.habit_start_stopwatch))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.card_menu = None;
                                    cx.emit(StartFocus {
                                        habit_id: Some(id),
                                        title: name_sw.clone(),
                                        stopwatch: true,
                                    });
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        ListItem::new("ctx-habit-archive")
                            .height(px(32.))
                            .start_slot(
                                Icon::new(if archived { IconName::ArchiveRestore } else { IconName::Archive })
                                    .color(Color::Muted),
                            )
                            .child(Label::new(if archived { strings.habit_unarchive } else { strings.habit_archive }))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_archived(id, !archived, cx))),
                    )
                    .child(
                        ListItem::new("ctx-habit-delete")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                            .child(Label::new(strings.delete).color(Color::Error))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.card_menu = None;
                                this.confirm = Some(ConfirmAction::DeleteHabit { habit_id: id });
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_day_context_menu(&self, menu: DayContextMenu, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let id = menu.habit_id;
        let date = menu.date;
        let skipped = self.habits.iter().find(|h| h.id == id).is_some_and(|h| h.is_skipped(date));
        let rec = self.records.get(&id).and_then(|r| r.iter().find(|c| c.date == date));
        let is_handled_or_partial = rec.is_some_and(|c| c.is_handled() || c.is_partial());

        div()
            .id("habit-day-ctx")
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .child(
                popover(cx)
                    .min_w(px(164.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.day_menu = None;
                        cx.notify();
                    }))
                    .child(
                        ListItem::new("day-ctx-log")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::MessageSquareText).color(Color::Muted))
                            .child(Label::new(strings.habit_log_on.trim_end_matches([' ', ':', '-', '—'])))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_log_modal(id, date, false, window, cx);
                            })),
                    )
                    .child(
                        ListItem::new("day-ctx-skip")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Minus).color(Color::Muted))
                            .child(Label::new(if skipped { strings.habit_unskip } else { strings.habit_skip }))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_skip(id, date, cx))),
                    )
                    .child(
                        ListItem::new("day-ctx-undone")
                            .height(px(32.))
                            .start_slot(Icon::new(IconName::Ban).color(Color::Muted))
                            .child(Label::new(strings.habit_unachieved))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.mark_undone(id, date, window, cx);
                            })),
                    )
                    .when(is_handled_or_partial || skipped, |el| {
                        el.child(
                            ListItem::new("day-ctx-reset")
                                .height(px(32.))
                                .start_slot(Icon::new(IconName::RotateCcw).color(Color::Muted))
                                .child(Label::new(strings.habit_reset))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.day_menu = None;
                                    if let Err(err) = this.store.reset_checkin(id, date, true) {
                                        eprintln!("failed to reset check-in: {err:#}");
                                    }
                                    this.notify_data_changed(cx);
                                    this.reload(cx);
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}
