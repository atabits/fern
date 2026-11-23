use super::*;
use super::kanban::KanbanSub;

impl TodoView {
    /// A column's header, as the original's: the name and count, then "+" and, for a section,
    /// "…" with Rename, Add Section to Left / Right, Move to › and Delete.
    pub(super) fn render_column_header(
        &self,
        column: usize,
        title: String,
        count: usize,
        target: Option<DropTarget>,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let overdue = target.is_none() && title == strings.group_overdue && count > 0;
        let section = match target {
            Some(DropTarget::Section(Some(id))) => Some(id),
            _ => None,
        };
        let renaming = self.section_rename.as_ref().filter(|(id, _)| Some(*id) == section).map(|(_, input)| input.clone());
        let menu_open = self.column_menu.is_some_and(|(id, _)| Some(id) == section);
        let drag_title: SharedString = title.clone().into();
        h_flex()
            .id(SharedString::from(format!("kanban-header-{column}")))
            .h(px(38.))
            .pt(px(12.))
            .pb(px(6.))
            .pl(px(10.))
            .pr(if self.kanban_column_color { px(10.) } else { px(12.) })
            .gap_1()
            .when_some(section.filter(|_| renaming.is_none()), |el, id| {
                let dragged = DraggedColumn { id, title: drag_title.clone() };
                el.cursor_grab()
                    .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.column_menu = Some((id, false));
                        cx.notify();
                    }))
            })
            .map(|el| match renaming {
                Some(input) => {
                    let input_focus = input.focus_handle(cx);
                    el.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(2.))
                            .debug_selector(|| "kanban-section-rename".to_owned())
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                cx.stop_propagation();
                                window.focus(&input_focus, cx);
                            })
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_section_rename(cx)))
                            .child(input),
                    )
                }
                None => el
                    .child(
                        div()
                            .id(SharedString::from(format!("kanban-title-{column}")))
                            .px(px(4.))
                            .rounded(px(4.))
                            .min_w_0()
                            .when_some(section, |el, id| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.start_kanban_section_rename(id, window, cx);
                                    }))
                            })
                            .child(Label::new(title).weight(FontWeight::SEMIBOLD).truncate()),
                    )
                    .child(div().ml(px(2.)).child(Label::new(count.to_string()).size(LabelSize::Small).color(Color::Muted)))
                    .child(div().flex_1()),
            })
            .when(overdue, |el| {
                el.child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id("kanban-postpone")
                                .cursor_pointer()
                                .child(Label::new(strings.postpone).size(LabelSize::Small).color(Color::Accent))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.postpone_menu = true;
                                    cx.notify();
                                })),
                        )
                        .when(self.postpone_menu, |el| {
                            el.child(PopoverAttachment::new(
                                Placement::BelowRight,
                                popover(cx)
                                    .min_w(px(180.))
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.postpone_menu = false;
                                        cx.notify();
                                    }))
                                    .children(fern_core::DefaultDate::ALL.iter().zip(strings.default_dates).map(|(&choice, name)| {
                                        ListItem::new(SharedString::from(format!("kanban-postpone-{name}")))
                                            .child(Label::new(name))
                                            .on_click(cx.listener(move |this, _, _, cx| this.postpone_overdue(Some(choice.date(Self::today())), cx)))
                                    }))
                                    .child(Divider::horizontal())
                                    .child(
                                        ListItem::new("kanban-postpone-clear")
                                            .start_slot(Icon::new(IconName::CalendarX).size(IconSize::XSmall).color(Color::Muted))
                                            .child(Label::new(strings.clear).color(Color::Muted))
                                            .on_click(cx.listener(|this, _, _, cx| this.postpone_overdue(None, cx))),
                                    ),
                            ))
                        }),
                )
            })
            .when_some(target.filter(|t| !matches!(t, DropTarget::Complete) && !self.kanban_quick_add && !self.is_current_view_archived_list()), |el, target| {
                el.child(
                    IconButton::new(SharedString::from(format!("kanban-add-{column}")), IconName::Plus)
                        .size(px(22.))
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.add_task))
                        .on_click(cx.listener(move |this, _, window, cx| this.start_column_add(column, target.clone(), window, cx))),
                )
            })
            .when_some(section.filter(|_| !self.is_current_view_archived_list()), |el, id| {
                el.child(
                    div()
                        .relative()
                        .child(
                            IconButton::new(SharedString::from(format!("kanban-menu-{column}")), IconName::Ellipsis)
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Muted)
                                .tooltip(Tooltip::text(strings.more))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.column_menu = if this.column_menu.is_some_and(|(open, _)| open == id) { None } else { Some((id, false)) };
                                    cx.notify();
                                })),
                        )
                        .when(menu_open, |el| el.child(PopoverAttachment::new(Placement::BelowRight, self.render_column_menu(id, column, strings, cx)))),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_column_menu(&self, section: Id, column: usize, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let item = |key: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(key).height(px(34.)).start_slot(Icon::new(icon).color(Color::Muted)).child(Label::new(label))
        };
        // The section's place among the list's sections, for Add Section to Left / Right.
        let index = self.sections.iter().position(|s| s.id == section).unwrap_or(column);
        let side = self.column_menu.is_some_and(|(_, side)| side);
        // Move to: the Inbox and the lists, but not where the section already is.
        let owner = self.section_owner();
        let lists: Vec<ListItem> = std::iter::once((None, SharedString::new_static(strings.inbox), IconName::Inbox))
            .chain(self.active_lists().into_iter().map(|l| (Some(l.id), SharedString::from(l.name), IconName::List)))
            .filter(|(to, _, _)| owner != Some(*to))
            .map(|(to, name, icon)| {
                ListItem::new(SharedString::from(format!("section-move-{to:?}")))
                    .start_slot(Icon::new(icon).color(Color::Muted))
                    .child(Label::new(name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.column_menu = None;
                        let result = this.store.move_list_section(section, to);
                        this.check(result);
                        this.reload(cx);
                    }))
            })
            .collect();
        let view = cx.entity().downgrade();
        let theme = cx.theme().clone();
        let current_color = self.kanban_section_colors.get(&section).map(String::as_str);
        let move_bounds = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
        let move_bounds_out = move_bounds.clone();
        popover(cx)
            .min_w(px(220.))
            .on_mouse_down_out(cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                if move_bounds_out.get().is_some_and(|b| b.contains(&event.position)) {
                    return;
                }
                this.column_menu = None;
                cx.notify();
            }))
            .child(item("section-rename", IconName::Pencil, strings.rename_section).on_click(cx.listener(move |this, _, window, cx| {
                this.start_kanban_section_rename(section, window, cx);
            })))
            .when(self.kanban_column_color, |el| {
                let swatch = |id: SharedString, hex: Option<&'static str>, cx: &mut Context<Self>| {
                    let active = current_color == hex;
                    let fill = hex.and_then(|c| lists_panel::parse_color(Some(c)));
                    div()
                        .id(id)
                        .size(px(18.))
                        .rounded_full()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(if active { theme.colors().accent } else { theme.colors().border })
                        .when_some(fill, |el, c| el.bg(c))
                        .when(active, |el| {
                            el.child(Icon::new(IconName::Check).size(IconSize::XSmall).color(if fill.is_some() { Color::IconOnAccent } else { Color::Accent }))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.set_section_color(section, hex, cx)))
                };
                el.child(
                    v_flex()
                        .px_2()
                        .py_1p5()
                        .gap_1()
                        .child(Label::new(strings.kanban_column_color).size(LabelSize::XSmall).color(Color::Muted))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .child(swatch("section-color-default".into(), None, cx))
                                .children(Self::KANBAN_COLUMN_COLORS.iter().map(|&hex| {
                                    swatch(SharedString::from(format!("section-color-{hex}")), Some(hex), cx)
                                })),
                        ),
                )
            })
            .child(item("section-left", IconName::BetweenVerticalStart, strings.add_section_left).on_click(cx.listener(move |this, _, window, cx| {
                this.column_menu = None;
                this.start_new_section(window, cx);
                this.new_section_index = Some(index);
            })))
            .child(item("section-right", IconName::BetweenVerticalEnd, strings.add_section_right).on_click(cx.listener(move |this, _, window, cx| {
                this.column_menu = None;
                this.start_new_section(window, cx);
                this.new_section_index = Some(index + 1);
            })))
            .child(
                SubmenuItem::new("section-move", strings.move_to)
                    .icon(IconName::Inbox)
                    .height(px(34.))
                    .open(side)
                    .track_bounds(move_bounds)
                    .on_hover(move |hovered: &bool, _, cx: &mut App| {
                        if *hovered && !side {
                            view.update(cx, |this, cx| {
                                this.column_menu = Some((section, true));
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    .items(lists),
            )
            .when(self.sections.len() > 1, |el| {
                let section_name = self
                    .sections
                    .iter()
                    .find(|s| s.id == section)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                el.child(
                    ListItem::new("section-delete")
                        .height(px(34.))
                        .start_slot(Icon::new(IconName::Trash).color(Color::Error))
                        .child(Label::new(strings.delete).color(Color::Error))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.column_menu = None;
                            this.confirm_delete = Some(super::ConfirmDeleteTarget::Section(section, section_name.clone()));
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    pub(super) fn apply_column_value(&self, id: Id, target: DropTarget) -> anyhow::Result<()> {
        let Some(task) = self.store.task(id)? else { return Ok(()) };
        let store = &self.store;
        if matches!(target, DropTarget::Priority(_)) && task.kind == Kind::Note {
            return Ok(());
        }
        match target {
            DropTarget::Section(section) => store.set_section(id, section),
            DropTarget::Kind(kind) => store.set_kind(id, kind),
            DropTarget::Todo => {
                if task.is_completed() {
                    store.set_completed(id, false)?;
                }
                if task.is_in_progress() {
                    store.set_in_progress(id, false)?;
                }
                Ok(())
            }
            DropTarget::InProgress => {
                if task.is_completed() {
                    store.set_completed(id, false)?;
                }
                if !task.is_in_progress() {
                    store.set_in_progress(id, true)?;
                }
                Ok(())
            }
            DropTarget::Complete => store.set_completed(id, true),
            DropTarget::Due(Some(date)) => store.move_dates(id, date),
            DropTarget::Due(None) => store.set_due(id, None, None),
            DropTarget::Priority(priority) => store.set_priority(id, priority),
            DropTarget::Tag(Some(tag)) => {
                let mut tags: Vec<String> = task.tags.iter().skip(1).filter(|t| **t != tag).cloned().collect();
                tags.insert(0, tag);
                store.set_tags(id, &tags)
            }
            DropTarget::Tag(None) => store.set_tags(id, &[]),
            DropTarget::List(list) => store.set_list(id, list),
        }
    }

    pub(super) fn apply_column_drop(&self, id: Id, target: DropTarget, unparent: bool) -> anyhow::Result<()> {
        let Some(task) = self.store.task(id)? else { return Ok(()) };
        if matches!(target, DropTarget::Priority(_)) && task.kind == Kind::Note {
            return Ok(());
        }
        if unparent && task.parent_id.is_some() {
            self.store.set_parent(id, None)?;
        }
        if task.is_completed() && target != DropTarget::Complete {
            self.store.set_completed(id, false)?;
        }
        self.apply_column_value(id, target)
    }

    /// A card dropped on a kanban column takes the column's value: its section, kind, date,
    /// priority or tag, or completion. Leaving the Completed subsection reopens the task.
    pub(super) fn drop_on_column(&mut self, id: Id, target: DropTarget, cx: &mut Context<Self>) {
        if self.is_current_view_archived_list() || self.is_task_id_read_only(id) {
            return;
        }
        if target == DropTarget::Complete {
            let result = (|| -> anyhow::Result<()> {
                let Some(task) = self.store.task(id)? else { return Ok(()) };
                if task.parent_id.is_some() {
                    self.store.set_parent(id, None)?;
                }
                let fresh = self.store.task(id)?.unwrap_or(task);
                self.complete_task_from_drop(&fresh, cx)?;
                Ok(())
            })();
            self.check(result);
            self.reload(cx);
            return;
        }
        let result = self.apply_column_drop(id, target, true);
        self.check(result);
        self.reload(cx);
    }

    /// Drops a card onto an in-column subsection header (`Pinned`, `Others`, or `Completed`).
    pub(super) fn drop_on_kanban_subsection(&mut self, id: Id, column_drop: Option<DropTarget>, sub: KanbanSub, cx: &mut Context<Self>) {
        let result = (|| -> anyhow::Result<()> {
            let Some(task) = self.store.task(id)? else { return Ok(()) };
            if task.parent_id.is_some() {
                self.store.set_parent(id, None)?;
            }
            if let Some(col_target) = column_drop
                && col_target != DropTarget::Complete
            {
                self.apply_column_value(id, col_target)?;
            }
            let task = self.store.task(id)?.unwrap_or(task);
            match sub {
                KanbanSub::Pinned => {
                    if task.is_completed() {
                        self.store.set_completed(id, false)?;
                    }
                    if task.pinned_at.is_none() {
                        self.store.set_pinned(id, true)?;
                    }
                }
                KanbanSub::Unpinned => {
                    if task.is_completed() {
                        self.store.set_completed(id, false)?;
                    }
                    if task.pinned_at.is_some() {
                        self.store.set_pinned(id, false)?;
                    }
                }
                KanbanSub::Completed => {
                    self.complete_task_from_drop(&task, cx)?;
                }
            }
            Ok(())
        })();
        self.check(result);
        self.reload(cx);
    }

    /// Tracks a dragged task across the kanban board:
    /// - within the source column: vertical reorder + horizontal indent/outdent via `plan_task_drop`
    /// - over another column: highlight the whole target column (no row insertion line)
    pub(super) fn move_kanban_task_drag(&mut self, event: &gpui::DragMoveEvent<DraggedTask>, _: &mut Window, cx: &mut Context<Self>) {
        if self.view == View::Trash || !self.kanban || self.timeline {
            return;
        }
        let pos = event.event.position;
        if !event.bounds.contains(&pos) {
            if self.task_drop.take().is_some() | self.kanban_hover_col.take().is_some() {
                cx.notify();
            }
            return;
        }
        let dragged_id = event.drag(cx).id;
        let start_x = match self.task_drag_origin {
            Some((id, x)) if id == dragged_id => x,
            _ => {
                self.task_drag_origin = Some((dragged_id, pos.x));
                pos.x
            }
        };

        let source_col_idx = self.kanban_drag_rows.iter().position(|c| c.row_subs.contains_key(&dragged_id));
        // A column's visual card panel shrinks to content height (`max_h_full`), while its drop
        // lane extends down to the bottom of the board.
        let hovered_col_key = {
            let cols = self.kanban_col_bounds.borrow();
            cols.iter()
                .find_map(|(k, b)| (pos.x >= b.left() && pos.x <= b.right() && pos.y >= b.top()).then(|| k.clone()))
        };

        let mut new_plan = None;
        let mut new_hover_col = None;

        if let Some(target_key) = hovered_col_key {
            let source_key = source_col_idx.map(|i| self.kanban_drag_rows[i].key.as_str());
            if source_key != Some(target_key.as_str()) {
                // Cross-column drag: reject row drop line and highlight target column if it accepts drops.
                if let Some(target_col) = self.kanban_drag_rows.iter().find(|c| c.key == target_key)
                    && let Some(drop_target) = &target_col.drop
                {
                    let dragged_is_note = self.tasks.iter().any(|t| t.id == dragged_id && t.kind == Kind::Note);
                    if !(dragged_is_note && matches!(drop_target, DropTarget::Priority(_))) {
                        new_hover_col = Some(target_key);
                    }
                }
            } else if let Some(src_idx) = source_col_idx {
                // Same-column drag: use the exact same tree drop planner as List view.
                let col = &self.kanban_drag_rows[src_idx];
                let row_bounds = self.kanban_row_bounds.borrow();
                let mut hit: Option<(Id, gpui::Bounds<Pixels>)> = None;
                let mut bottommost_y: Option<Pixels> = None;
                for &task_id in col.row_subs.keys() {
                    if let Some(&bounds) = row_bounds.get(&task_id) {
                        bottommost_y = Some(bottommost_y.map_or(bounds.bottom(), |b| b.max(bounds.bottom())));
                        if pos.y >= bounds.top() && pos.y <= bounds.bottom() {
                            hit = Some((task_id, bounds));
                        }
                    }
                }
                drop(row_bounds);

                let allow_nesting = !matches!(self.view, View::Completed | View::WontDo | View::Trash);
                new_plan = if let Some((hovered_id, bounds)) = hit {
                    let mid_y = bounds.top() + bounds.size.height / 2.;
                    let after = pos.y >= mid_y;
                    let dx = f32::from(pos.x - start_x);
                    let rel_x = f32::from(pos.x - bounds.left());
                    task_rows::plan_task_drop(
                        &col.rows,
                        dragged_id,
                        hovered_id,
                        after,
                        dx,
                        rel_x,
                        INDENT,
                        KANBAN_GUTTER,
                        allow_nesting,
                    )
                } else if bottommost_y.is_some_and(|bottom| pos.y > bottom) {
                    let dragged_completed = col.row_subs.get(&dragged_id) == Some(&KanbanSub::Completed);
                    let end_slice = if !dragged_completed {
                        let cutoff = col
                            .rows
                            .iter()
                            .position(|r| matches!(r, Row::Day { drop: Some(DropTarget::Complete), .. }))
                            .unwrap_or(col.rows.len());
                        &col.rows[..cutoff]
                    } else {
                        &col.rows[..]
                    };
                    task_rows::plan_task_drop_at_end(end_slice, dragged_id)
                } else {
                    None
                };
            }
        }

        if self.task_drop != new_plan || self.kanban_hover_col != new_hover_col {
            self.task_drop = new_plan;
            self.kanban_hover_col = new_hover_col;
            cx.notify();
        }
    }

    pub(super) fn drop_task_in_kanban(&mut self, dragged: &DraggedTask, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_current_view_archived_list() || self.is_task_id_read_only(dragged.id) {
            self.task_drop = None;
            self.kanban_hover_col = None;
            self.task_drag_origin = None;
            return;
        }
        let plan = self.task_drop.take();
        let hover_col = self.kanban_hover_col.take();
        self.task_drag_origin = None;
        if let Some(plan) = plan {
            cx.stop_propagation();
            self.apply_kanban_task_drop(dragged.id, plan, cx);
        } else if let Some(col_key) = hover_col {
            cx.stop_propagation();
            if let Some(drop_target) =
                self.kanban_drag_rows.iter().find(|c| c.key == col_key).and_then(|c| c.drop.clone())
            {
                self.drop_on_column(dragged.id, drop_target, cx);
            }
        } else {
            cx.notify();
        }
    }

    pub(super) fn apply_kanban_task_drop(&mut self, dragged: Id, plan: task_rows::TaskDropPlan, cx: &mut Context<Self>) {
        let target_sub = self
            .kanban_drag_rows
            .iter()
            .find_map(|c| c.row_subs.get(&plan.target_id).copied());
        let result = (|| -> anyhow::Result<()> {
            let Some(dragged_task) = self.store.task(dragged)? else { return Ok(()) };
            let mut advanced_repeat = false;
            match target_sub {
                Some(KanbanSub::Completed) => {
                    advanced_repeat = self.complete_task_from_drop(&dragged_task, cx)?;
                }
                Some(KanbanSub::Pinned) => {
                    if dragged_task.is_completed() {
                        self.store.set_completed(dragged, false)?;
                    }
                    if plan.new_parent.is_none() && dragged_task.pinned_at.is_none() {
                        self.store.set_pinned(dragged, true)?;
                    }
                }
                Some(KanbanSub::Unpinned) => {
                    if dragged_task.is_completed() {
                        self.store.set_completed(dragged, false)?;
                    }
                    if plan.new_parent.is_none() && dragged_task.pinned_at.is_some() {
                        self.store.set_pinned(dragged, false)?;
                    }
                }
                None => {}
            }
            if !advanced_repeat {
                self.store.move_task_in_tree(dragged, plan.new_parent, plan.order_target)?;
            }
            Ok(())
        })();
        if self.check(result).is_some()
            && let Some(parent_id) = plan.expand_parent
        {
            self.collapsed_tasks.remove(&parent_id);
        }
        self.reload(cx);
    }
}
