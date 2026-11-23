use chrono::NaiveDate;
use desktop_common::{calendar, colors, data_version, layout};
use fern_core::{Id, ScheduleBucket, Task};
use fern_i18n::Strings;
use gpui::{AnyElement, Context, FontWeight, Hsla, SharedString, div, px};
use task_ui::drag::DraggedTask;
use task_ui::task_rows;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, EmptyState, Icon, IconButton, IconName, IconSize, Label,
    LabelSize, Tooltip, v_scroll,
};

use super::bottom_bar::{self, BottomBarProps};
use super::inline_card::{self, CollapsedRowProps, ExpandedCardProps};
use super::magic_plus::{DraggedMagicPlus, MagicPlusDropTarget};
use super::sidebar;
use super::when_picker::{self, DeadlineTarget, WhenPickerState};
use super::{PopoverAnchor, ThingsEvent, ThingsView, ThingsViewKind};

impl ThingsView {
    fn empty_state_spec(&self, strings: &'static Strings) -> (IconName, &'static str) {
        match self.view {
            ThingsViewKind::Inbox => (IconName::Inbox, strings.things_empty_inbox),
            ThingsViewKind::Today => (IconName::Sun, strings.things_empty_today),
            ThingsViewKind::Upcoming => (IconName::CalendarDays, strings.things_empty_upcoming),
            ThingsViewKind::Anytime => (IconName::Layers, strings.things_empty_anytime),
            ThingsViewKind::Someday => (IconName::Archive, strings.things_empty_someday),
            ThingsViewKind::Logbook => (IconName::CircleCheck, strings.things_empty_logbook),
            ThingsViewKind::Project(_) => (IconName::CircleDot, strings.things_empty_project),
            ThingsViewKind::Area(_) => (IconName::Folder, strings.things_empty_area),
        }
    }

    fn render_header(&self, today: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let theme_colors = theme.colors().clone();
        let status = theme.status().clone();

        let (icon_el, title, subtitle): (AnyElement, SharedString, Option<String>) = match self.view {
            ThingsViewKind::Inbox => (
                Icon::new(IconName::Inbox).size(IconSize::XLarge).color(Color::Custom(status.info)).into_any_element(),
                strings.things_inbox.into(),
                None,
            ),
            ThingsViewKind::Today => (
                Icon::new(IconName::Sun).size(IconSize::XLarge).color(Color::Custom(status.warning)).into_any_element(),
                strings.things_today.into(),
                Some(calendar::day_header(today, today, strings)),
            ),
            ThingsViewKind::Upcoming => (
                Icon::new(IconName::CalendarDays).size(IconSize::XLarge).color(Color::Custom(status.error)).into_any_element(),
                strings.things_upcoming.into(),
                None,
            ),
            ThingsViewKind::Anytime => (
                Icon::new(IconName::Layers).size(IconSize::XLarge).color(Color::Accent).into_any_element(),
                strings.things_anytime.into(),
                None,
            ),
            ThingsViewKind::Someday => (
                Icon::new(IconName::Archive).size(IconSize::XLarge).color(Color::IconMuted).into_any_element(),
                strings.things_someday.into(),
                None,
            ),
            ThingsViewKind::Logbook => (
                Icon::new(IconName::CircleCheck).size(IconSize::XLarge).color(Color::Custom(status.success)).into_any_element(),
                strings.things_logbook.into(),
                None,
            ),
            ThingsViewKind::Area(fid) => {
                let name = self
                    .folders
                    .iter()
                    .find(|f| f.id == fid)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| strings.things_areas.to_owned());
                (
                    Icon::new(IconName::Folder).size(IconSize::XLarge).color(Color::Icon).into_any_element(),
                    name.into(),
                    None,
                )
            }
            ThingsViewKind::Project(lid) => {
                let proj = self.lists.iter().find(|l| l.id == lid);
                let name = proj
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| strings.things_projects.to_owned());
                let prog = self.progress.get(&lid).copied().unwrap_or_default();
                let ring_color = proj
                    .and_then(|l| colors::parse_color(l.color.as_deref()))
                    .unwrap_or(theme_colors.accent);
                let completed = proj.is_some_and(|l| l.completed_at.is_some());
                (
                    sidebar::project_donut(prog, completed, ring_color, px(22.), px(3.)).into_any_element(),
                    name.into(),
                    None,
                )
            }
        };

        let project_controls = if let ThingsViewKind::Project(list_id) = self.view {
            self.lists.iter().find(|l| l.id == list_id).map(|proj| {
                let is_completed = proj.completed_at.is_some();
                let deadline_label = proj
                    .deadline
                    .map(|d| task_rows::due_label(d, None, today, strings).0)
                    .unwrap_or_else(|| strings.things_deadline.to_owned());
                let deadline_open = self
                    .deadline_picker
                    .as_ref()
                    .is_some_and(|(a, _)| *a == PopoverAnchor::ProjectHeader);
                let deadline_pop = deadline_open.then(|| {
                    self.deadline_picker.as_ref().map(|(_, state)| {
                        when_picker::render_deadline_popover(state, today, strings, cx, |this, act, window, cx| {
                            this.handle_deadline_action(act, window, cx);
                        })
                    })
                }).flatten();

                v_flex()
                    .px_6()
                    .pb_2()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        Button::new("things-proj-deadline", deadline_label)
                                            .variant(ButtonVariant::Secondary)
                                            .size(ButtonSize::Sm)
                                            .icon(IconName::Flag)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if this
                                                    .deadline_picker
                                                    .as_ref()
                                                    .is_some_and(|(a, _)| *a == PopoverAnchor::ProjectHeader)
                                                {
                                                    this.close_all_popovers();
                                                    this.restore_focus(window, cx);
                                                    cx.notify();
                                                } else {
                                                    this.open_deadline_picker(
                                                        PopoverAnchor::ProjectHeader,
                                                        DeadlineTarget::Project(list_id),
                                                        cx,
                                                    );
                                                }
                                            })),
                                    )
                                    .when_some(deadline_pop, |el, pop| {
                                        el.child(ui::PopoverAttachment::new(ui::Placement::BelowLeft, pop))
                                    }),
                            )
                            .child(
                                Button::new(
                                    "things-proj-complete",
                                    if is_completed {
                                        strings.things_reopen_project
                                    } else {
                                        strings.things_complete_project
                                    },
                                )
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Sm)
                                .icon(if is_completed {
                                    IconName::RotateCcw
                                } else {
                                    IconName::Check
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(p) = this.lists.iter().find(|l| l.id == list_id) {
                                        let next_at = if p.completed_at.is_some() {
                                            None
                                        } else {
                                            Some(chrono::Utc::now().timestamp_millis())
                                        };
                                        if this
                                            .store
                                            .set_list_project_fields(list_id, &p.notes, p.deadline, next_at)
                                            .is_ok()
                                        {
                                            data_version::data_changed(cx.entity_id(), cx);
                                            this.reload(cx);
                                        }
                                    }
                                })),
                            ),
                    )
                    .when_some(self.project_notes.as_ref().map(|(_, i, _)| i.clone()), |col, input| {
                        col.child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(6.))
                                .bg(theme_colors.surface_background.opacity(0.6))
                                .text_size(px(13.))
                                .child(input),
                        )
                    })
                    .into_any_element()
            })
        } else {
            None
        };

        v_flex()
            .flex_none()
            .child(
                h_flex()
                    .mt(layout::header_inset())
                    .h(px(54.))
                    .px_6()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2p5()
                            .child(div().flex_none().child(icon_el))
                            .child(
                                div()
                                    .min_w_0()
                                    .when(cfg!(test), |el| el.debug_selector(|| "things-view-title".to_owned()))
                                    .child(
                                        Label::new(title)
                                            .size(LabelSize::Large)
                                            .weight(FontWeight::SEMIBOLD)
                                            .truncate(),
                                    ),
                            )
                            .when_some(subtitle, |el, sub| {
                                el.child(
                                    div().flex_none().child(
                                        Label::new(sub)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    ),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .child(Chip::new(strings.beta_badge, status.warning))
                            .child(
                                div()
                                    .flex_none()
                                    .when(cfg!(test), |el| {
                                        el.debug_selector(|| "things-header-search".to_owned())
                                    })
                                    .child(
                                        IconButton::new("things-header-search", IconName::Search)
                                            .size(px(28.))
                                            .icon_size(IconSize::Small)
                                            .icon_color(Color::Muted)
                                            .tooltip(Tooltip::text(strings.section_search))
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.emit(ThingsEvent::OpenSearch);
                                            })),
                                    ),
                            ),
                    ),
            )
            .children(project_controls)
            .into_any_element()
    }

    fn render_when_popover_with_error(
        &self,
        state: &WhenPickerState,
        today: NaiveDate,
        strings: &'static Strings,
        error_above: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pop = when_picker::render_when_popover(state, today, strings, cx, |this, act, window, cx| {
            this.handle_when_action(act, window, cx);
        });
        if !self.when_date_error {
            return pop;
        }
        let theme = cx.theme();
        let colors = theme.colors();
        let err_color = theme.status().error;
        let error_banner = h_flex()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "things-when-date-error".to_owned())
            })
            .w(px(244.))
            .px_2p5()
            .py_1p5()
            .gap_1p5()
            .rounded(px(8.))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(err_color.opacity(0.65))
            .shadow_md()
            .occlude()
            .child(Icon::new(IconName::CircleAlert).size(IconSize::XSmall).color(Color::Error))
            .child(
                div().flex_1().min_w_0().child(
                    Label::new(strings.calendar_invalid_year_date)
                        .size(LabelSize::XSmall)
                        .color(Color::Error)
                        .truncate(),
                ),
            );
        if error_above {
            v_flex().w(px(244.)).gap_1().child(error_banner).child(pop).into_any_element()
        } else {
            v_flex().w(px(244.)).gap_1().child(pop).child(error_banner).into_any_element()
        }
    }

    fn render_task_item(&self, task: &Task, today: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        if let Some(card) = &self.expanded
            && card.task_id == task.id
        {
            let when_pop = self
                .when_picker
                .as_ref()
                .filter(|(a, s)| *a == PopoverAnchor::Card && s.task_id == task.id)
                .map(|(_, state)| self.render_when_popover_with_error(state, today, strings, false, cx));
            let deadline_pop = self
                .deadline_picker
                .as_ref()
                .filter(|(a, s)| *a == PopoverAnchor::Card && s.target == DeadlineTarget::Task(task.id))
                .map(|(_, state)| {
                    when_picker::render_deadline_popover(state, today, strings, cx, |this, act, window, cx| {
                        this.handle_deadline_action(act, window, cx);
                    })
                });

            return inline_card::render_expanded_card(
                ExpandedCardProps {
                    task,
                    state: card,
                    today,
                    lists: &self.lists,
                    folders: &self.folders,
                    all_tags: &self.tags,
                    tag_colors: &self.tag_colors,
                    strings,
                    when_popover: when_pop,
                    deadline_popover: deadline_pop,
                },
                cx,
                |this, act, window, cx| this.handle_card_action(act, window, cx),
            );
        }

        inline_card::render_collapsed_row(
            CollapsedRowProps {
                task,
                view: self.view,
                selected: self.selected_id == Some(task.id),
                today,
                list_names: &self.list_names,
                tag_colors: &self.tag_colors,
                strings,
            },
            cx,
            |this, act, window, cx| this.handle_card_action(act, window, cx),
        )
    }

    pub(super) fn render_task_pane(&self, today: NaiveDate, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors().clone();
        let accent = colors.accent;
        let groups = self.groups_rc(today, strings);
        let active_drag = cx.has_active_drag();
        self.row_bounds.borrow_mut().clear();

        let logged_tasks: Vec<&Task> = match self.view {
            ThingsViewKind::Project(_) => {
                self.tasks.iter().filter(|t| t.is_closed()).collect()
            }
            _ => Vec::new(),
        };
        let has_any_visible_tasks = groups.iter().any(|g| !g.tasks.is_empty());

        let mut group_elements: Vec<AnyElement> = Vec::new();
        for group in groups.iter() {
            let show_when_empty = matches!(self.view, ThingsViewKind::Area(_))
                || (active_drag
                    && (group.id.as_ref() == "group-today-evening"
                        || group.id.as_ref() == "group-today-day"
                        || matches!(self.view, ThingsViewKind::Project(_))));
            if group.tasks.is_empty() && !show_when_empty {
                continue;
            }
            let drop_for_task = group.drop.clone();
            let drop_for_magic = group.drop.clone();
            let is_evening_group = group.id.as_ref() == "group-today-evening";
            let is_today_day_group = group.id.as_ref() == "group-today-day";

            let header = group.title.as_ref().map(|title| {
                let click_proj = group.click_project;
                let task_count = group.tasks.len();
                h_flex()
                    .id(SharedString::from(format!("hdr-{}", group.id)))
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .gap_2()
                    .when(click_proj.is_some(), |el| el.cursor_pointer())
                    .when_some(click_proj, |el, pid| {
                        el.on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.select_view(ThingsViewKind::Project(pid), window, cx);
                        }))
                    })
                    .when_some(group.donut, |el, (prog, done, color)| {
                        let c = if color == Hsla::default() { accent } else { color };
                        el.child(div().flex_none().child(sidebar::project_donut(prog, done, c, px(14.), px(2.2))))
                    })
                    .when_some(group.icon, |el, icon| {
                        el.child(
                            div().flex_none().child(
                                Icon::new(icon)
                                    .size(IconSize::Small)
                                    .color(group.icon_color.unwrap_or(Color::Muted)),
                            ),
                        )
                    })
                    .child(
                        div().min_w_0().child(
                            Label::new(title.clone())
                                .size(LabelSize::Small)
                                .weight(FontWeight::SEMIBOLD)
                                .color(if is_evening_group {
                                    Color::Accent
                                } else {
                                    Color::Muted
                                })
                                .truncate(),
                        ),
                    )
                    .when(task_count > 0, |el| {
                        el.child(
                            div().flex_none().child(
                                Label::new(task_count.to_string())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            ),
                        )
                    })
            });

            let rendered_tasks: Vec<AnyElement> = group
                .tasks
                .iter()
                .map(|t| self.render_task_item(t, today, strings, cx))
                .collect();
            let task_ids: Vec<Id> = group.tasks.iter().map(|t| t.id).collect();
            let row_bounds = self.row_bounds.clone();
            let list_scroll = self.list_scroll.clone();
            let tasks_container = v_flex()
                .children(rendered_tasks)
                .on_children_prepainted(move |children_bounds, _, _| {
                    let offset = list_scroll.offset();
                    let mut map = row_bounds.borrow_mut();
                    for (idx, task_id) in task_ids.iter().enumerate() {
                        if let Some(bounds) = children_bounds.get(idx) {
                            map.insert(
                                *task_id,
                                gpui::Bounds::new(bounds.origin - offset, bounds.size),
                            );
                        }
                    }
                });

            let empty_drop_slot = (group.tasks.is_empty() && show_when_empty).then(|| {
                let slot_label = if is_evening_group {
                    strings.things_drop_evening_hint
                } else if is_today_day_group {
                    strings.things_today
                } else if matches!(self.view, ThingsViewKind::Area(_)) {
                    strings.things_empty_project
                } else {
                    strings.no_tasks
                };
                div()
                    .mx_3()
                    .my_1()
                    .h(px(32.))
                    .px_3()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(colors.border_variant)
                    .flex()
                    .items_center()
                    .child(
                        Label::new(slot_label)
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
            });

            group_elements.push(
                v_flex()
                    .id(group.id.clone())
                    .rounded(px(8.))
                    .when_some(drop_for_task, |el, d| {
                        el.drag_over::<DraggedTask>(move |style, _, _, _| {
                            style.bg(accent.opacity(0.08))
                        })
                        .on_drop(cx.listener(move |this, dragged: &DraggedTask, window, cx| {
                            this.apply_group_drop(dragged.id, d.clone(), window, cx);
                        }))
                    })
                    .when_some(drop_for_magic, |el, d| {
                        el.drag_over::<DraggedMagicPlus>(move |style, _, _, _| {
                            style.bg(accent.opacity(0.08)).border_1().border_color(accent.opacity(0.4))
                        })
                        .on_drop(cx.listener(move |this, _: &DraggedMagicPlus, window, cx| {
                            let target = match &d {
                                super::GroupDrop::Schedule {
                                    start_date,
                                    bucket,
                                    list_id,
                                } => MagicPlusDropTarget {
                                    list_id: list_id.flatten(),
                                    section_id: None,
                                    start_date: *start_date,
                                    schedule_bucket: *bucket,
                                    after_task: None,
                                },
                                super::GroupDrop::Project {
                                    list_id,
                                    section_id,
                                } => MagicPlusDropTarget {
                                    list_id: Some(*list_id),
                                    section_id: *section_id,
                                    start_date: None,
                                    schedule_bucket: ScheduleBucket::None,
                                    after_task: None,
                                },
                            };
                            this.create_todo_at(target, window, cx);
                        }))
                    })
                    .children(header)
                    .child(tasks_container)
                    .children(empty_drop_slot)
                    .into_any_element(),
            );
        }

        let logged_section = (!logged_tasks.is_empty()).then(|| {
            let open = self.show_logged_in_project;
            let count = logged_tasks.len();
            v_flex()
                .mt_4()
                .child(
                    div()
                        .id("things-logged-toggle")
                        .when(cfg!(test), |el| {
                            el.debug_selector(|| "things-logged-toggle".to_owned())
                        })
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .rounded(px(6.))
                        .hover(|s| s.bg(colors.ghost_element_hover))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.show_logged_in_project = !this.show_logged_in_project;
                            if !this.show_logged_in_project {
                                if let Some(expanded_id) = this.expanded.as_ref().map(|e| e.task_id)
                                    && this.tasks.iter().any(|t| t.id == expanded_id && t.is_closed())
                                {
                                    this.close_all_popovers();
                                    this.finish_expanded_card(cx);
                                    window.focus(&this.focus_handle, cx);
                                }
                                if let Some(sel) = this.selected_id
                                    && this.tasks.iter().any(|t| t.id == sel && t.is_closed())
                                {
                                    this.selected_id = this.visible_order.first().copied();
                                }
                            }
                            cx.notify();
                        }))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .child(
                                    Icon::new(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                                )
                                .child(
                                    Label::new(format!("{} ({count})", strings.things_logged_items))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        ),
                )
                .when(open, |col| {
                    col.children(
                        logged_tasks
                            .iter()
                            .copied()
                            .map(|t| self.render_task_item(t, today, strings, cx)),
                    )
                })
        });

        let selected_task = self
            .active_target_task_id()
            .and_then(|id| self.tasks.iter().find(|t| t.id == id));

        let bottom_when_pop = self
            .when_picker
            .as_ref()
            .filter(|(a, _)| *a == PopoverAnchor::BottomBar)
            .map(|(_, state)| self.render_when_popover_with_error(state, today, strings, true, cx));

        let (empty_icon, empty_msg) = self.empty_state_spec(strings);
        let default_target = self.default_drop_target_for_view();
        let allow_magic_drop = self.view != ThingsViewKind::Logbook;
        let empty_area_folder = match self.view {
            ThingsViewKind::Area(folder_id) if group_elements.is_empty() => Some(folder_id),
            _ => None,
        };
        let show_empty_state = !has_any_visible_tasks
            && !active_drag
            && logged_tasks.is_empty()
            && group_elements.is_empty();
        let view_weak = cx.entity().downgrade();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(colors.background)
            .child(self.render_header(today, strings, cx))
            .child(
                v_scroll(
                    "things-main-scroll",
                    v_flex()
                        .id("things-main-list")
                        .when(cfg!(test), |el| {
                            el.debug_selector(|| "things-main-list".to_owned())
                        })
                        .flex_1()
                        .px_4()
                        .pb_6()
                        .gap_1()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.expanded.is_some() {
                                this.dismiss_transient(window, cx);
                            } else {
                                window.focus(&this.focus_handle, cx);
                            }
                        }))
                        .when(allow_magic_drop, |el| {
                            el.drag_over::<DraggedMagicPlus>(move |style, _, _, _| {
                                style.bg(accent.opacity(0.04))
                            })
                            .on_drop(cx.listener(
                                move |this, _: &DraggedMagicPlus, window, cx| {
                                    this.create_todo_at(default_target, window, cx);
                                },
                            ))
                        })
                        .when(show_empty_state, |col| {
                            col.child(
                                div()
                                    .flex_1()
                                    .min_h(px(220.))
                                    .when(cfg!(test), |el| {
                                        el.debug_selector(|| "things-empty-state".to_owned())
                                    })
                                    .child(
                                        EmptyState::new(empty_icon, empty_msg)
                                            .when(self.view != ThingsViewKind::Logbook, |es| {
                                                let view_weak = view_weak.clone();
                                                if let Some(folder_id) = empty_area_folder {
                                                    es.action(
                                                        "things-empty-add",
                                                        strings.things_new_project,
                                                        move |_, window, cx| {
                                                            cx.stop_propagation();
                                                            let _ = view_weak.update(cx, |this, cx| {
                                                                this.open_new_project_dialog(
                                                                    Some(folder_id),
                                                                    window,
                                                                    cx,
                                                                );
                                                            });
                                                        },
                                                    )
                                                } else {
                                                    es.action(
                                                        "things-empty-add",
                                                        strings.things_new_todo,
                                                        move |_, window, cx| {
                                                            cx.stop_propagation();
                                                            let _ = view_weak.update(cx, |this, cx| {
                                                                this.create_new_todo_default(window, cx);
                                                            });
                                                        },
                                                    )
                                                }
                                            }),
                                    ),
                            )
                        })
                        .when(!group_elements.is_empty(), |col| {
                            col.children(group_elements)
                        })
                        .children(logged_section),
                )
                .track_scroll(&self.list_scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(bottom_bar::render_bottom_bar(
                BottomBarProps {
                    selected_task,
                    lists: &self.lists,
                    folders: &self.folders,
                    strings,
                    when_popover: bottom_when_pop,
                    move_open: self.bottom_move_open,
                },
                cx,
                |this, act, window, cx| this.handle_bottom_bar_action(act, window, cx),
            ))
            .into_any_element()
    }
}
