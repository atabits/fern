use super::*;

/// Visual card-slice role of a visible kanban row:
/// a root card and its expanded descendants share one continuous outer border and radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KanbanItemUi {
    Root,
    RootHasChildren,
    Child,
    LastChild,
}

pub(super) struct KanbanVisibleRow {
    pub(super) task: Task,
    pub(super) depth: usize,
    pub(super) has_children: bool,
    pub(super) collapsed: bool,
    pub(super) descendant_done: usize,
    pub(super) descendant_total: usize,
    pub(super) ui: KanbanItemUi,
}

impl TodoView {
    /// Flattens a kanban subsection's cards into visible rows preserving real multi-level
    /// nesting depth, per-parent collapse state, and `KanbanItemUi` card-slice role.
    pub(super) fn kanban_visible_rows(&self, cards: &[BoardCard]) -> Vec<KanbanVisibleRow> {
        let mut out = Vec::new();
        for (root, subs) in cards {
            let mut card_tasks = Vec::with_capacity(subs.len() + 1);
            card_tasks.push(root.clone());
            card_tasks.extend(subs.iter().cloned());
            let tree = fern_core::TaskTree::build(&card_tasks);
            for (task, depth) in tree.flatten_with_depth(|id| !self.collapsed_tasks.contains(&id)) {
                let has_children = tree.has_children(task.id);
                let collapsed = has_children && self.collapsed_tasks.contains(&task.id);
                let (descendant_done, descendant_total) = if collapsed {
                    let descs = tree.descendants(task.id);
                    (descs.iter().filter(|d| d.is_completed()).count(), descs.len())
                } else {
                    (0, 0)
                };
                out.push(KanbanVisibleRow {
                    task: task.clone(),
                    depth,
                    has_children,
                    collapsed,
                    descendant_done,
                    descendant_total,
                    ui: KanbanItemUi::Root,
                });
            }
        }

        for i in 0..out.len() {
            let next_is_child = out.get(i + 1).is_some_and(|n| n.depth > 0);
            out[i].ui = if out[i].depth == 0 {
                if next_is_child { KanbanItemUi::RootHasChildren } else { KanbanItemUi::Root }
            } else if next_is_child {
                KanbanItemUi::Child
            } else {
                KanbanItemUi::LastChild
            };
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_kanban_row(
        &self,
        column: usize,
        row: &KanbanVisibleRow,
        tag_colors: &HashMap<String, Hsla>,
        column_tint: Option<Hsla>,
        show_list_name: bool,
        today: NaiveDate,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let task = &row.task;
        let id = task.id;
        let depth = row.depth;
        let has_children = row.has_children;
        let collapsed = row.collapsed;
        let ui = row.ui;

        let theme = cx.theme().clone();
        let dark = theme.is_dark();
        let selected = self.selected == Some(id) || self.card == Some(id);
        let settled_done = task.is_completed();
        let done = settled_done || self.completing.contains_key(&id);
        let details_line = self
            .show_details
            .then(|| task.notes.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_owned()))
            .flatten();
        let has_note_icon = details_line.is_none() && !task.notes.trim().is_empty();
        let due = task.due_date.map(|date| {
            task_rows::row_due_label(
                date,
                task.due_time,
                today,
                strings,
                self.preferences.countdown_dates,
                cx,
            )
        });
        let horizon_badge = task_rows::horizon_badge(task, today, strings);
        let has_reminders = !task.reminders.is_empty() && !settled_done;
        let has_location = task.location.as_deref().is_some_and(|l| !l.trim().is_empty());
        let checklist = task.checklist.filter(|_| !settled_done);
        let list_info = (depth == 0 && show_list_name).then(|| {
            task.list_id
                .and_then(|lid| self.lists.iter().find(|l| l.id == lid))
                .map_or((SharedString::new_static(strings.inbox), None), |l| {
                    (SharedString::from(l.name.clone()), lists_panel::parse_color(l.color.as_deref()))
                })
        });
        let has_badges_row = !task.tags.is_empty() || horizon_badge.is_some();
        let has_tips_row = due.is_some()
            || has_note_icon
            || has_location
            || task.repeat.is_some()
            || has_reminders
            || checklist.is_some()
            || (collapsed && has_children)
            || list_info.is_some();
        let has_meta = details_line.is_some() || has_badges_row || has_tips_row;

        let indent_px = depth as f32 * INDENT;
        let row_pl = px(KANBAN_GUTTER + indent_px);
        let py_top = if depth == 0 { px(10.) } else { px(8.) };
        let py_bottom = if has_meta {
            px(4.)
        } else if depth == 0 {
            px(10.)
        } else {
            px(8.)
        };

        let title_input = self
            .title_edit
            .as_ref()
            .filter(|(editing, _, _)| *editing == id)
            .map(|(_, input, _)| input.clone());
        let read_only = self.is_task_read_only(task);
        let draggable = self.view != View::Trash && !read_only && title_input.is_none();
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let dragging_active = cx.has_active_drag();
        let is_being_dragged = dragging_active && self.panel_dnd.task_drag_origin.is_some_and(|(d, _)| d == id);
        let drop_indicator = dragging_active
            .then(|| self.panel_dnd.task_drop.as_ref().filter(|plan| plan.target_id == id).cloned())
            .flatten();

        // Match TickTick's `.kanban-bg-el`: `bg-main-background`, subtle dark/light border,
        // `bg-primary-10 dark:bg-grey-6` when selected (no accent border), and soft shadow in light mode.
        let base_surface = theme.colors().elevated_surface_background;
        let card_border = if let Some(tint) = column_tint {
            tint.opacity(if dark { 0.24 } else { 0.16 })
        } else if dark {
            theme.colors().border
        } else {
            theme.colors().border_variant
        };
        let card_bg = if selected {
            if dark {
                base_surface.blend(theme.colors().text.opacity(0.06))
            } else {
                base_surface.blend(theme.colors().accent.opacity(0.10))
            }
        } else if let Some(tint) = column_tint {
            if dark { base_surface.blend(tint.opacity(0.12)) } else { base_surface }
        } else {
            base_surface
        };
        let card_hover = if selected {
            card_bg
        } else if let Some(tint) = column_tint {
            base_surface.blend(tint.opacity(if dark { 0.16 } else { 0.04 }))
        } else {
            base_surface.blend(theme.colors().ghost_element_hover)
        };
        let accent = theme.colors().accent;
        let row_bounds = self.kanban.row_bounds.clone();
        let card_open_here = self.card == Some(id);

        let content_line = h_flex()
            .relative()
            .items_start()
            .pl(row_pl)
            .pr(px(12.))
            .pt(py_top)
            .pb(py_bottom)
            .gap(px(9.))
            .when(has_children, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("card-fold-{id}")))
                        .absolute()
                        .left(px(1. + indent_px))
                        .top(if depth == 0 { px(11.) } else { px(9.) })
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(chevron(collapsed).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if cx.has_active_drag() {
                                return;
                            }
                            cx.stop_propagation();
                            if !this.collapsed_tasks.remove(&id) {
                                this.collapsed_tasks.insert(id);
                            }
                            this.invalidate_rows();
                            cx.notify();
                        })),
                )
            })
            // A note shows the note icon, as the original's cards, not a box.
            .map(|el| {
                if task.kind == Kind::Note {
                    el.child(div().pt(px(2.)).child(Icon::new(IconName::FileText).size(IconSize::Small).color(Color::Muted)))
                } else {
                    let checkbox = Checkbox::new(SharedString::from(format!("card-check-{id}")))
                        .toggle_state(done)
                        .animate_completion(crate::completion_mark::started(id, cx))
                        .in_progress(task.is_in_progress())
                        .tint(task_rows::priority_color(task.priority, &theme))
                        .when(!read_only, |cb| {
                            cb.on_click(cx.listener(move |this, _, _, cx| {
                                if cx.has_active_drag() {
                                    return;
                                }
                                cx.stop_propagation();
                                this.toggle(id, !done, cx);
                            }))
                        });
                    // A finger gets a wider target around the box, which does the same.
                    el.child(
                        div()
                            .id(SharedString::from(format!("card-box-{id}")))
                            .pt(px(2.))
                            .debug_selector(move || format!("kanban-checkbox-{id}"))
                            .when(crate::touch::active() && !read_only, |el| {
                                el.p(px(8.)).m(px(-8.)).on_click(cx.listener(move |this, _, _, cx| {
                                    if cx.has_active_drag() {
                                        return;
                                    }
                                    cx.stop_propagation();
                                    this.toggle(id, !done, cx);
                                }))
                            })
                            .child(checkbox),
                    )
                }
            })
            .child(if let Some(input) = title_input {
                div()
                    .id(SharedString::from(format!("card-title-edit-{id}")))
                    .flex_1()
                    .min_w_0()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(input)
                    .into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("card-title-{id}")))
                    .flex_1()
                    .min_w_0()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(if done { theme.colors().text_muted } else { theme.colors().text })
                    .when(!read_only, |el| {
                        el.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                                if event.click_count > 1 {
                                    cx.stop_propagation();
                                    this.close_card(cx);
                                    this.start_title_edit(id, Some(event.position), window, cx);
                                }
                            }),
                        )
                    })
                    .child(if task.title.is_empty() { strings.untitled.to_owned() } else { task.title.clone() })
                    .into_any_element()
            });

        let inner = v_flex()
            .relative()
            .min_h(px(if depth == 0 { 40. } else { 36. }))
            .border_color(card_border)
            .map(|el| match ui {
                KanbanItemUi::Root => el.rounded(px(10.)).border_1(),
                KanbanItemUi::RootHasChildren => el.rounded_t(px(10.)).border_t_1().border_l_1().border_r_1(),
                KanbanItemUi::Child => el.border_l_1().border_r_1(),
                KanbanItemUi::LastChild => el.rounded_b(px(10.)).border_l_1().border_r_1().border_b_1(),
            })
            .bg(card_bg)
            .when(!dark && column_tint.is_none() && matches!(ui, KanbanItemUi::Root), |el| el.shadow_xs())
            .hover(move |s| s.bg(card_hover))
            .when(settled_done, |el| el.opacity(0.6))
            .when(depth > 0, |el| {
                el.child(div().ml(px(KANBAN_GUTTER + indent_px + 25.)).mr(px(12.)).h(px(1.)).bg(theme.colors().border_variant))
            })
            .child(content_line)
            // TickTick's `KanbanRow3` (tags) + `KanbanRow2` (`task-tips-more`: date, icons, progress, list).
            .when(has_meta, |el| {
                let done_subs = row.descendant_done;
                let total_subs = row.descendant_total;
                el.child(
                    v_flex()
                        .pl(px(KANBAN_GUTTER + indent_px + 25.))
                        .pr(px(12.))
                        .pb(px(8.))
                        .gap_1()
                        .when_some(details_line, |el, text| {
                            el.child(Label::new(text).size(LabelSize::XSmall).color(Color::Muted).truncate())
                        })
                        .when(has_badges_row, |el| {
                            el.child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(task.tags.iter().map(|tag| {
                                        Chip::new(tag.clone(), tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted))
                                    }))
                                    .when_some(horizon_badge, |el, (badge, past)| {
                                        let (fg, bg, border) =
                                            task_rows::horizon_badge_style(past, done || task.is_wont_do(), &theme);
                                        el.child(
                                            div()
                                                .flex_none()
                                                .px(px(5.))
                                                .py(px(0.5))
                                                .rounded(px(4.))
                                                .bg(bg)
                                                .border_1()
                                                .border_color(border)
                                                .child(Label::new(badge).size(LabelSize::XSmall).color(Color::Custom(fg))),
                                        )
                                    }),
                            )
                        })
                        .when(has_tips_row, |el| {
                            el.child(
                                h_flex()
                                    .min_h(px(18.))
                                    .gap_1p5()
                                    .when_some(due, |el, (label, overdue)| {
                                        let color = if done {
                                            Color::Muted
                                        } else if overdue {
                                            Color::Error
                                        } else {
                                            Color::Accent
                                        };
                                        el.child(
                                            div()
                                                .id(SharedString::from(format!("kanban-due-toggle-{id}")))
                                                .px_0p5()
                                                .rounded(px(3.))
                                                .cursor_pointer()
                                                .hover(|s| s.bg(theme.colors().element_hover))
                                                .child(Label::new(label).size(LabelSize::XSmall).color(color))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.save_preferences(|p| p.countdown_dates = !p.countdown_dates, cx);
                                                })),
                                        )
                                    })
                                    .when(has_note_icon, |el| {
                                        el.child(Icon::new(IconName::FileText).size(IconSize::XSmall).color(Color::Muted))
                                    })
                                    .when(has_location, |el| {
                                        el.child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                                    })
                                    .when(task.repeat.is_some(), |el| {
                                        el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted))
                                    })
                                    .when(has_reminders, |el| {
                                        el.child(
                                            Icon::new(IconName::AlarmClock)
                                                .size(IconSize::XSmall)
                                                .color(if task.constant_reminder { Color::Accent } else { Color::Muted }),
                                        )
                                    })
                                    .when_some(checklist, |el, (done_items, total)| {
                                        el.child(
                                            h_flex()
                                                .gap_0p5()
                                                .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall).color(Color::Muted))
                                                .child(Label::new(format!("{done_items}/{total}")).size(LabelSize::XSmall).color(Color::Muted)),
                                        )
                                    })
                                    .when(collapsed && has_children, |el| {
                                        el.child(
                                            h_flex()
                                                .gap_0p5()
                                                .child(Icon::new(IconName::ListTree).size(IconSize::XSmall).color(Color::Muted))
                                                .child(Label::new(format!("{done_subs}/{total_subs}")).size(LabelSize::XSmall).color(Color::Muted)),
                                        )
                                    })
                                    .when_some(list_info, |el, (name, dot)| {
                                        el.child(div().flex_1()).child(
                                            h_flex()
                                                .min_w_0()
                                                .gap_1()
                                                .when_some(dot, |el, c| el.child(div().size(px(6.)).rounded_full().bg(c).flex_none()))
                                                .child(Label::new(name).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                                        )
                                    }),
                            )
                        }),
                )
            });

        div()
            .id(ElementId::Uuid(id))
            .debug_selector(move || {
                if depth > 0 {
                    format!("card-sub-{id}")
                } else {
                    format!("kanban-card-{id}")
                }
            })
            .relative()
            // Outer vertical padding: 8 px between distinct cards with zero dead gaps between row hit-boxes.
            .map(|el| match ui {
                KanbanItemUi::Root => el.py(px(4.)),
                KanbanItemUi::RootHasChildren => el.pt(px(4.)),
                KanbanItemUi::Child => el,
                KanbanItemUi::LastChild => el.pb(px(4.)),
            })
            .cursor_pointer()
            .when(draggable, |el| {
                let view = cx.entity().downgrade();
                el.on_drag(dragged, move |dragged, cursor_offset, window, cx| {
                    let dragged_id = dragged.id;
                    let fallback_x = window.mouse_position().x;
                    view.update(cx, |this, cx| {
                        let start_x = this
                            .kanban.row_bounds
                            .borrow()
                            .get(&dragged_id)
                            .copied()
                            .map_or(fallback_x, |area| area.left() + cursor_offset.x);
                        this.panel_dnd.task_drag_origin = Some((dragged_id, start_x));
                        this.panel_dnd.task_drop = None;
                        this.kanban.hover_col = None;
                        cx.notify();
                    })
                    .ok();
                    cx.new(|_| dragged.clone())
                })
            })
            .when(is_being_dragged, |el| el.opacity(0.45))
            .on_click(cx.listener(move |this, _, window, cx| {
                if cx.has_active_drag()
                    || this.title_edit.as_ref().is_some_and(|(editing, _, _)| *editing == id)
                {
                    return;
                }
                window.focus(&this.list_focus, cx);
                this.open_card(id, cx);
            }))
            .when(!read_only || task.is_deleted(), |el| {
                el.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_context_menu(id, event.position, false, window, cx);
                    }),
                )
            })
            // As the original's, the card opens on the right of the first columns, on the left of
            // the others.
            .when(card_open_here, |el| {
                let placement = if column < 2 { Placement::RightOf } else { Placement::LeftOf };
                el.child(PopoverAttachment::new(placement, self.render_card_details(cx)))
            })
            .child(
                gpui::canvas(
                    move |area, _, _| {
                        row_bounds.borrow_mut().insert(id, area);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(inner)
            .when_some(drop_indicator, |el, plan| {
                el.child(Self::render_drop_indicator(&plan, KANBAN_GUTTER, accent, theme.colors().background))
            })
            .into_any_element()
    }

    /// The kanban's Habit column in Today and Next 7 Days, as the original's: a card per habit
    /// due today with its icon and name and "Today 0 / 2 unit" (or "0 / 1" without a goal); a
    /// click checks it in or adds a step.
    pub(super) fn render_habit_column(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits = Rc::clone(&self.today_habits);
        if habits.is_empty() {
            return None;
        }
        let count = habits.len();
        let dark = theme.is_dark();
        let base_surface = theme.colors().elevated_surface_background;
        let habit_border = theme.colors().border_variant;
        let habit_hover = base_surface.blend(theme.colors().ghost_element_hover);
        let cards: Vec<AnyElement> = habits
            .iter()
            .map(|(habit, checked, amount)| {
                let checked = *checked;
                let amount = *amount;
                let id = habit.id;
                let has_goal = habit.goal.is_some();
                let (target, unit) = habit.goal.as_ref().map_or((1.0, String::new()), |g| (g.amount, g.unit.clone()));
                let progress_text = format!(
                    "{} / {} {}",
                    fern_core::habit::format_amount(amount),
                    fern_core::habit::format_amount(target),
                    unit
                )
                .trim()
                .to_owned();
                v_flex()
                    .id(SharedString::from(format!("habit-card-{id}")))
                    .pl(px(14.))
                    .pr(px(12.))
                    .py(px(10.))
                    .gap_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(habit_border)
                    .bg(base_surface)
                    .when(checked, |el| el.opacity(0.62))
                    .when(!dark && !self.kanban.column_color, |el| el.shadow_xs())
                    .hover(move |s| s.bg(habit_hover))
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .gap(px(7.))
                            .child(crate::habit_view::avatar(habit, px(20.), cx))
                            .child(
                                div().flex_1().min_w_0().child(
                                    Label::new(habit.name.clone())
                                        .color(if checked { Color::Muted } else { Color::Default })
                                        .truncate(),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .pl(px(27.))
                            .gap_1()
                            .child(Label::new(strings.today).size(LabelSize::Small).color(Color::Accent))
                            .child(Label::new(progress_text).size(LabelSize::Small).color(Color::Muted)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let result = if has_goal {
                            this.store.step_checkin(id, today).map(|_| ())
                        } else {
                            this.store.toggle_checkin(id, today).map(|_| ())
                        };
                        this.check(result);
                        desktop_common::data_version::data_changed(cx.entity_id(), cx);
                        this.reload_today_habits(cx);
                        cx.notify();
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.overlays.habit_menu = Some((id, ev.position));
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            })
            .collect();
        let col_width = self.kanban_column_width();
        let dark = theme.is_dark();
        let tint = self.kanban.column_color.then(|| lists_panel::parse_color(Some("#A5A5A5"))).flatten();
        let card_px = if self.kanban.column_color { px(10.) } else { px(12.) };
        Some(
            v_flex()
                .w(px(col_width))
                .flex_none()
                .max_h_full()
                .rounded(px(14.))
                .when_some(tint, |el, c| el.bg(c.opacity(if dark { 0.08 } else { 0.10 })))
                .child(
                    h_flex()
                        .h(px(38.))
                        .pt(px(12.))
                        .pb(px(6.))
                        .pl(px(14.))
                        .pr(card_px)
                        .gap_1()
                        .child(Label::new(strings.section_habits).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)),
                )
                .child(
                    v_scroll(
                        "kanban-habits-scroll",
                        v_flex()
                            .id("kanban-habits")
                            .px(card_px)
                            .pt(px(4.))
                            .pb(px(12.))
                            .gap_2()
                            .children(cards),
                    )
                    .mt(px(2.))
                    .min_h_0(),
                )
                .into_any_element(),
        )
    }
}
