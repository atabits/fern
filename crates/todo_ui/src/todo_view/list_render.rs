use super::*;

use gpui::{ListAlignment, ListState, list};
use ui::VerticalScrollbar;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RowsCacheKey {
    pub(super) version: u64,
    pub(super) today: NaiveDate,
    pub(super) strings_ptr: usize,
    pub(super) sort: SortBy,
    pub(super) group: GroupBy,
    pub(super) show_completed: bool,
    pub(super) descending: bool,
    pub(super) closed_range: ClosedRange,
    pub(super) closed_lists: Option<Vec<Option<Id>>>,
    pub(super) collapsed_tasks_len: usize,
    pub(super) collapsed_sections_len: usize,
    pub(super) collapsed_days_len: usize,
    pub(super) completing_len: usize,
    pub(super) overdue_last: bool,
    pub(super) horizons_enabled: bool,
    pub(super) week_start: chrono::Weekday,
}

/// An entry of the task list: a row, or what the list shows around the rows.
#[derive(Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub(super) enum ListEntry {
    /// The field for a new section's name.
    NewSection,
    /// Today's habits.
    Habits,
    /// The countdowns shown in Today.
    Countdowns,
    Row(Row),
}

/// A round color option; `None` is "no color", drawn as an outlined circle.
pub(super) fn chevron(collapsed: bool) -> Icon {
    Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).size(IconSize::XSmall)
}

impl TodoView {
    /// The rows of the current view: closed tasks by day, the trash flat, everything else grouped.
    /// Tells the list which entries changed since it last drew: the ones between the unchanged
    /// start and end get measured again, and the scroll position stays put. Habits and
    /// countdowns are read elsewhere, so they are always measured again.
    pub(super) fn new_list_state() -> ListState {
        ListState::new(0, ListAlignment::Top, px(400.)).measure_all()
    }

    pub(super) fn sync_list(&mut self, entries: Vec<ListEntry>, rows: Rc<Vec<Row>>) {
        let old = &self.list_entries;
        let start = old.iter().zip(&entries).take_while(|(a, b)| a == b).count();
        let end = old[start..].iter().rev().zip(entries[start..].iter().rev()).take_while(|(a, b)| a == b).count();
        if start < old.len() || old.len() != entries.len() {
            let inserted = entries.len() - start - end;
            self.list_state.splice(start..old.len() - end, inserted);
            if inserted > 0 {
                self.list_state.remeasure_items(start..start + inserted);
            }
        }
        for (ix, entry) in entries.iter().enumerate() {
            if !matches!(entry, ListEntry::Row(_)) {
                self.list_state.remeasure_items(ix..ix + 1);
            }
        }
        self.list_rows = rows;
        self.list_entries = entries;
    }

    pub(super) fn sync_list_entries(&mut self, strings: &Strings) -> bool {
        let rows = self.rows_rc(strings);
        let rows_empty = rows.is_empty();
        let list_key = (self.rows_cache_key(strings), self.new_section.is_some(), self.view == View::Next7Days);
        if self.synced_list_key.as_ref() == Some(&list_key) && !self.list_entries.is_empty() {
            for (ix, entry) in self.list_entries.iter().enumerate() {
                if !matches!(entry, ListEntry::Row(_)) {
                    self.list_state.remeasure_items(ix..ix + 1);
                }
            }
        } else {
            let mut entries = Vec::with_capacity(rows.len() + 3);
            if self.new_section.is_some() {
                entries.push(ListEntry::NewSection);
            }
            // In Next 7 Days today's habits come first, as the original's.
            if self.view == View::Next7Days {
                entries.push(ListEntry::Habits);
            }
            entries.extend(rows.iter().cloned().map(ListEntry::Row));
            if self.view != View::Next7Days {
                entries.push(ListEntry::Habits);
            }
            entries.push(ListEntry::Countdowns);
            self.sync_list(entries, rows);
            self.synced_list_key = Some(list_key);
        }
        rows_empty
    }

    pub(super) fn render_entry(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        // Each entry spans the list's width, as in the column the list replaced.
        let entry_bounds = self.list_entry_bounds.clone();
        div()
            .relative()
            .w_full()
            .px(px(14.))
            .child(
                gpui::canvas(move |area, _, _| {
                    entry_bounds.borrow_mut().insert(ix, area);
                }, |_, _, _, _| {})
                .absolute()
                .size_full(),
            )
            .child(self.render_entry_content(ix, cx))
            .into_any_element()
    }

    pub(super) fn render_entry_content(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let empty = || div().into_any_element();
        match self.list_entries.get(ix).cloned() {
            Some(ListEntry::NewSection) => match self.new_section.clone() {
                Some(input) => h_flex()
                    .h(px(32.))
                    .mx(px(GUTTER))
                    .px_2()
                    .rounded(px(6.))
                    .bg(cx.theme().colors().surface_background)
                    .child(input)
                    .into_any_element(),
                None => empty(),
            },
            Some(ListEntry::Habits) => self.render_habits(strings, cx).unwrap_or_else(empty),
            Some(ListEntry::Countdowns) => self.render_countdowns(strings, cx).unwrap_or_else(empty),
            Some(ListEntry::Row(Row::Section { section, count, collapsed })) => {
                self.render_section(section, count, collapsed, strings, cx)
            }
            // As the original's, a group folds by its header, the arrow in the gutter.
            Some(ListEntry::Row(Row::Day { label, count, drop })) => {
                let collapsed = self.collapsed_days.contains(&label);
                let key = label.clone();
                let accent = cx.theme().colors().accent;
                let drop_on_header = drop.clone();
                let header_id: SharedString = match &drop {
                    Some(DropTarget::Section(Some(id))) => format!("day-sec-{id}").into(),
                    _ => format!("day-{ix}-{label}").into(),
                };
                h_flex()
                    .id(header_id)
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when_some(drop_on_header, |el, target| {
                        el.drag_over::<DraggedTask>(move |style, _, _, _| style.bg(accent.opacity(0.10)))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                                cx.stop_propagation();
                                this.task_drop = None;
                                this.task_drag_origin = None;
                                this.drop_on_column(dragged.id, target.clone(), cx);
                            }))
                    })
                    .child(div().w(px(GUTTER)).flex_none().pl(px(4.)).child(chevron(collapsed)))
                    .child(div().flex_1().min_w_0().child(self.render_group_header(ix, label, count, drop.as_ref(), 36., cx)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.section_rename.is_none() {
                            this.toggle_day(key.clone(), cx);
                        }
                    }))
                    .into_any_element()
            }
            Some(ListEntry::Row(Row::Task { task, depth, has_children, collapsed })) => {
                let tag_colors = Rc::clone(&self.tag_colors);
                self.render_task(&task, depth, has_children, collapsed, &tag_colors, Self::today(), strings, cx)
            }
            None => empty(),
        }
    }

    pub(super) fn invalidate_rows(&mut self) {
        self.rows_version = self.rows_version.wrapping_add(1);
        self.cached_rows.borrow_mut().take();
        self.kanban_cache.borrow_mut().take();
        self.synced_list_key = None;
    }

    pub(super) fn rows_cache_key(&self, strings: &Strings) -> RowsCacheKey {
        RowsCacheKey {
            version: self.rows_version,
            today: Self::today(),
            strings_ptr: strings as *const Strings as usize,
            sort: self.sort_by,
            group: self.group_by,
            show_completed: self.show_completed,
            descending: self.sort_descending,
            closed_range: self.closed_range,
            closed_lists: self.closed_lists.clone(),
            collapsed_tasks_len: self.collapsed_tasks.len(),
            collapsed_sections_len: self.collapsed_sections.len(),
            collapsed_days_len: self.collapsed_days.len(),
            completing_len: self.completing.len(),
            overdue_last: self.preferences.task_defaults.overdue_last,
            horizons_enabled: crate::calendar::horizons_enabled(),
            week_start: crate::calendar::week_start(),
        }
    }

    pub(super) fn rows_rc(&self, strings: &Strings) -> Rc<Vec<Row>> {
        let key = self.rows_cache_key(strings);
        if let Some((cached_key, rows)) = self.cached_rows.borrow().as_ref()
            && *cached_key == key
        {
            return Rc::clone(rows);
        }
        let built = Rc::new(task_rows::fold_days(
            self.rows_with(strings, &self.collapsed_tasks, &self.collapsed_sections),
            &self.collapsed_days,
        ));
        *self.cached_rows.borrow_mut() = Some((key, Rc::clone(&built)));
        built
    }

    pub(super) fn rows(&self, strings: &Strings) -> Vec<Row> {
        (*self.rows_rc(strings)).clone()
    }

    pub(super) fn tasks_for_rows(&self) -> Vec<Task> {
        let mut tasks = self.tasks.clone();
        let mut pending: Vec<_> = self.completing.values().collect();
        pending.sort_by_key(|pending| pending.index);
        for pending in pending {
            if let crate::undo::UndoAction::ReopenRepeat { copy, .. } = &pending.undo {
                tasks.retain(|task| task.id != *copy);
            }
            if let Some(task) = tasks.iter_mut().find(|task| task.id == pending.before.id) {
                *task = pending.before.clone();
            } else {
                tasks.insert(pending.index.min(tasks.len()), pending.before.clone());
            }
        }
        tasks
    }

    /// Rows with the given collapsed tasks and groups; the kanban board shows everything.
    pub(super) fn rows_with(&self, strings: &Strings, collapsed_tasks: &HashSet<Id>, collapsed_sections: &HashSet<Section>) -> Vec<Row> {
        let tasks = self.tasks_for_rows();
        let closed = |closed_at: fn(&Task) -> Option<i64>| {
            let today = Self::today();
            let tasks: Vec<Task> = tasks
                .iter()
                .filter(|t| {
                    let day = closed_at(t).and_then(task_rows::local_day).unwrap_or(today);
                    self.closed_range.contains(day, today)
                        && self.closed_lists.as_ref().is_none_or(|lists| lists.contains(&t.list_id))
                })
                .cloned()
                .collect();
            task_rows::rows_by_day(&tasks, closed_at, today, strings)
        };
        match self.view {
            View::Completed => closed(|t| t.completed_at),
            View::WontDo => closed(|t| t.wont_do_at),
            View::Trash | View::Search(_) => task_rows::build_rows(
                &tasks,
                collapsed_tasks,
                collapsed_sections,
                self.view != View::Trash,
            ),
            _ => {
                let (sort, group) = self.sort_settings();
                let mut tasks = tasks;
                if !self.view_flag("show_completed", true) {
                    tasks.retain(|t| !t.is_completed());
                }
                task_rows::order_tasks(&mut tasks, sort, self.view_flag("descending", false));
                task_rows::pinned_first(&mut tasks);
                match group {
                    // As the original's Next 7 Days: a group for each day.
                    GroupBy::Custom if self.view == View::Next7Days => {
                        task_rows::next_days_rows(&tasks, Self::today(), crate::calendar::week_start(), strings)
                    }
                    GroupBy::Custom if !self.sections.is_empty() => {
                        task_rows::section_rows(&tasks, &self.sections, collapsed_tasks, strings)
                    }
                    GroupBy::Custom => {
                        task_rows::build_rows(&tasks, collapsed_tasks, collapsed_sections, true)
                    }
                    group => {
                        let lists: Vec<(Id, String)> = self.lists.iter().map(|l| (l.id, l.name.clone())).collect();
                        task_rows::grouped_rows_in(&tasks, group, Self::today(), self.preferences.task_defaults.overdue_last, &lists, strings)
                    }
                }
            }
        }
    }

    pub(super) fn clear_task_drag_state(&mut self) -> bool {
        self.task_drop.take().is_some()
            | self.task_drag_origin.take().is_some()
            | self.kanban_hover_col.take().is_some()
    }

    pub(super) fn cancel_task_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let had_state = self.clear_task_drag_state();
        if had_state || cx.has_active_drag() {
            cx.stop_active_drag(window);
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn render_drop_indicator(
        plan: &task_rows::TaskDropPlan,
        gutter: f32,
        accent: Hsla,
        dot_bg: Hsla,
    ) -> AnyElement {
        let left = px(plan.target_level as f32 * INDENT + gutter);
        div()
            .absolute()
            .left(left)
            .right_0()
            .when(!plan.after, |s| s.top(px(-1.)))
            .when(plan.after, |s| s.bottom(px(-1.)))
            .h(px(2.))
            .bg(accent)
            .child(
                div()
                    .absolute()
                    .left(px(-3.))
                    .top(px(-2.))
                    .size(px(6.))
                    .rounded_full()
                    .border_2()
                    .border_color(accent)
                    .bg(dot_bg),
            )
            .into_any_element()
    }

    /// Whether the active list view renders subtasks nested under their parents.
    pub(super) fn allows_tree_nesting(&self) -> bool {
        !matches!(self.view, View::Completed | View::WontDo | View::Trash | View::Next7Days)
            && self.sort_settings().1 == GroupBy::Custom
    }

    pub(super) fn move_task_drag(&mut self, event: &gpui::DragMoveEvent<DraggedTask>, _: &mut Window, cx: &mut Context<Self>) {
        if self.view == View::Trash || self.is_current_view_archived_list() || self.kanban || self.timeline {
            return;
        }
        let pos = event.event.position;
        if !event.bounds.contains(&pos) {
            if self.task_drop.take().is_some() {
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
        let allow_nesting = self.allows_tree_nesting();
        let bounds_map = self.list_entry_bounds.borrow();

        let mut hit: Option<(usize, gpui::Bounds<Pixels>)> = None;
        let mut bottommost_row_y: Option<Pixels> = None;
        for (&ix, &bounds) in bounds_map.iter() {
            if !matches!(self.list_entries.get(ix), Some(ListEntry::Row(_))) {
                continue;
            }
            bottommost_row_y = Some(bottommost_row_y.map_or(bounds.bottom(), |b| b.max(bounds.bottom())));
            if pos.y >= bounds.top() && pos.y <= bounds.bottom() {
                hit = Some((ix, bounds));
            }
        }
        drop(bounds_map);

        let plan = if let Some((ix, bounds)) = hit {
            match self.list_entries.get(ix) {
                Some(ListEntry::Row(Row::Task { task, .. })) => {
                    let mid_y = bounds.top() + bounds.size.height / 2.;
                    let after = pos.y >= mid_y;
                    let dx = f32::from(pos.x - start_x);
                    // Subtract `render_entry`'s 14 px horizontal padding so `rel_x = 0` is the row's left edge.
                    let rel_x = f32::from(pos.x - bounds.left()) - 14.0;
                    task_rows::plan_task_drop(
                        &self.list_rows,
                        dragged_id,
                        task.id,
                        after,
                        dx,
                        rel_x,
                        INDENT,
                        GUTTER,
                        allow_nesting,
                    )
                }
                _ => None,
            }
        } else if bottommost_row_y.is_some_and(|bottom| pos.y > bottom) {
            task_rows::plan_task_drop_at_end(&self.list_rows, dragged_id)
        } else {
            None
        };

        if self.task_drop != plan {
            self.task_drop = plan;
            cx.notify();
        }
    }

    pub(super) fn drop_task_in_list(&mut self, dragged: &DraggedTask, _: &mut Window, cx: &mut Context<Self>) {
        let plan = self.task_drop.take();
        self.task_drag_origin = None;
        if let Some(plan) = plan {
            cx.stop_propagation();
            self.apply_task_drop(dragged.id, plan, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn apply_task_drop(&mut self, dragged: Id, plan: task_rows::TaskDropPlan, cx: &mut Context<Self>) {
        if self.is_current_view_archived_list() || self.is_task_id_read_only(dragged) {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let Some(dragged_task) = self.store.task(dragged)? else { return Ok(()) };
            let mut advanced_repeat = false;
            if let Some(group_drop) = plan.group_drop {
                match group_drop {
                    DropTarget::Section(sec) => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if dragged_task.section_id != sec {
                            self.store.set_section(dragged, sec)?;
                        }
                    }
                    DropTarget::Todo => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if dragged_task.is_in_progress() {
                            self.store.set_in_progress(dragged, false)?;
                        }
                    }
                    DropTarget::InProgress => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if !dragged_task.is_in_progress() {
                            self.store.set_in_progress(dragged, true)?;
                        }
                    }
                    DropTarget::Complete => {
                        advanced_repeat = self.complete_task_from_drop(&dragged_task, cx)?;
                    }
                    DropTarget::Kind(kind) => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        if dragged_task.kind != kind {
                            self.store.set_kind(dragged, kind)?;
                        }
                    }
                    other => {
                        if dragged_task.is_completed() {
                            self.store.set_completed(dragged, false)?;
                        }
                        self.apply_column_value(dragged, other)?;
                    }
                }
            } else if dragged_task.is_completed()
                && self.store.task(plan.target_id)?.is_some_and(|t| !t.is_completed())
            {
                self.store.set_completed(dragged, false)?;
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

    pub(super) fn start_title_edit(&mut self, id: Id, click: Option<gpui::Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_title_edit(true, cx);
        let Some(task) = self.tasks.iter().find(|task| task.id == id && !self.is_task_read_only(task)) else { return };
        let title = task.title.clone();
        if !self.uses_board_layout() {
            self.select(id, cx);
        }
        let input = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, "", cx);
            input.set_text(title, cx);
            if let Some(position) = click {
                input.place_cursor_at(position, cx);
            }
            input
        });
        cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| {
            this.finish_title_edit(true, cx);
            window.focus(&this.list_focus, cx);
        }).detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.finish_title_edit(false, cx);
            window.focus(&this.list_focus, cx);
        }).detach();
        let focus = input.focus_handle(cx);
        let blur = cx.on_blur(&focus, window, |this, _, cx| this.finish_title_edit(true, cx));
        if click.is_some() {
            input.update(cx, |input, cx| input.open_touch_keyboard(window, cx));
        }
        self.title_edit = Some((id, input, blur));
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn finish_title_edit(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some((id, input, _blur)) = self.title_edit.take() else { return };
        let title = input.read(cx).text().trim();
        if save && !title.is_empty() && self.tasks.iter().any(|task| task.id == id && task.title != title) {
            let result = self.store.set_title(id, title);
            if self.check(result).is_some() {
                self.reload(cx);
            }
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_task(
        &self,
        task: &Task,
        depth: usize,
        has_children: bool,
        collapsed: bool,
        tag_colors: &HashMap<String, Hsla>,
        today: NaiveDate,
        strings: &Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let id = task.id;
        let title_input = self.title_edit.as_ref().filter(|(editing, _, _)| *editing == id).map(|(_, input, _)| input.clone());
        let done = task.is_completed() || self.completing.contains_key(&id);
        let details = self.show_details && !task.notes.trim().is_empty();
        let wont_do = task.is_wont_do();
        let read_only = self.is_task_read_only(task);
        let closed_view = matches!(self.view, View::Completed | View::WontDo);
        let selected = self.is_task_selected(id);
        let group = format!("task-{id}");

        let marker = if task.kind == Kind::Note {
            Icon::new(IconName::FileText).into_any_element()
        } else {
            Checkbox::new("marker")
                .toggle_state(done)
                .animate_completion(crate::completion_mark::started(id, cx))
                .in_progress(task.is_in_progress())
                .dismissed(wont_do)
                .lines(task.checklist.is_some())
                .tint(task_rows::priority_color(task.priority, &theme))
                .when(!read_only, |cb| {
                    cb.on_click(cx.listener(move |this, _, _, cx| {
                        if cx.has_active_drag() {
                            return;
                        }
                        cx.stop_propagation();
                        this.toggle(id, !done && !wont_do, cx);
                    }))
                })
                .into_any_element()
        };
        // A finger gets a wider target around the box, which does the same.
        let marker = if crate::touch::active() && task.kind != Kind::Note && !read_only {
            div()
                .id("marker-target")
                .p(px(8.))
                .m(px(-8.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if cx.has_active_drag() {
                        return;
                    }
                    cx.stop_propagation();
                    this.toggle(id, !done && !wont_do, cx);
                }))
                .child(marker)
                .into_any_element()
        } else {
            marker
        };

        let due = task.due_date.map(|date| {
            if matches!(self.view, View::Completed | View::WontDo) {
                (task_rows::closed_due_label(date, today, strings), false)
            } else {
                task_rows::row_due_label(date, task.due_time, today, strings, self.preferences.countdown_dates)
            }
        });
        let badge_group = if closed_view
            || done
            || wont_do
            || matches!(self.view, View::Trash | View::Search(_))
        {
            GroupBy::None
        } else if self.view == View::Next7Days && self.group_by == GroupBy::Custom {
            GroupBy::Date
        } else {
            self.group_by
        };
        let horizon_badge = task_rows::horizon_badge_for_group(task, badge_group, today, strings);

        let draggable = self.view != View::Trash && !read_only && title_input.is_none();
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let accent = theme.colors().accent;
        let dragging_active = cx.has_active_drag();
        let is_being_dragged = dragging_active && self.task_drag_origin.is_some_and(|(d, _)| d == id);
        let drop_indicator = dragging_active
            .then(|| self.task_drop.as_ref().filter(|plan| plan.target_id == id).cloned())
            .flatten();

        // The separator starts at the checkbox; the gutter to its left holds the chevron.
        div()
            .id(ElementId::Uuid(id))
            .debug_selector(move || format!("task-row-{id}"))
            .group(group.clone())
            .relative()
            .when(draggable, |el| {
                let view = cx.entity().downgrade();
                el.on_drag(dragged, move |dragged, cursor_offset, window, cx| {
                    let dragged_id = dragged.id;
                    let fallback_x = window.mouse_position().x;
                    view.update(cx, |this, cx| {
                        let start_x = this
                            .list_entries
                            .iter()
                            .position(|e| matches!(e, ListEntry::Row(Row::Task { task, .. }) if task.id == dragged_id))
                            .and_then(|ix| this.list_entry_bounds.borrow().get(&ix).copied())
                            .map_or(fallback_x, |area| area.left() + px(14.) + cursor_offset.x);
                        this.task_drag_origin = Some((dragged_id, start_x));
                        this.task_drop = None;
                        cx.notify();
                    })
                    .ok();
                    cx.new(|_| dragged.clone())
                })
            })
            .when(is_being_dragged, |el| el.opacity(0.45))
            .when_some(drop_indicator, |el, plan| {
                el.child(Self::render_drop_indicator(&plan, GUTTER, accent, theme.colors().background))
            })
            .h(px(if details { ROW_HEIGHT + 14. } else { ROW_HEIGHT }))
            .flex()
            .pl(px(depth as f32 * INDENT))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |el| el.bg(theme.colors().ghost_element_selected))
            .when(!selected, |el| el.hover(|s| s.bg(theme.colors().ghost_element_hover)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                if cx.has_active_drag()
                    || this.title_edit.as_ref().is_some_and(|(editing, _, _)| *editing == id)
                {
                    return;
                }
                this.handle_row_click(id, event.modifiers(), window, cx);
            }))
            // The original's right-click menu, at the pointer; the task is selected too.
            // Archived-list tasks have no context menu; Trash has Restore / Delete Forever.
            .when(!read_only || task.is_deleted(), |el| {
                el.on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.open_context_menu(id, event.position, true, window, cx);
                }))
            })
            .child(
                div()
                    .id("toggle-children")
                    .w(px(GUTTER))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(has_children, |el| {
                        el.child(chevron(collapsed)).on_click(cx.listener(move |this, _, _, cx| {
                            if cx.has_active_drag() {
                                return;
                            }
                            cx.stop_propagation();
                            this.toggle_task_collapsed(id, cx);
                        }))
                    }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(px(10.))
                    // Reserve the menu button's width and gap even when it is hidden.
                    .pr(px(36.))
                    .border_b_1()
                    .border_color(theme.colors().border_variant)
                    .child(marker)
                    .when(task.pinned_at.is_some() && !closed_view, |el| {
                        el.child(Icon::new(IconName::Pin).size(IconSize::XSmall).color(Color::Accent))
                    })
                    .child(
                        v_flex().flex_1().min_w_0().debug_selector(|| format!("task-title-field-{id}")).child(if let Some(input) = title_input {
                            input.into_any_element()
                        } else {
                            div()
                                .id("task-title")
                                .max_w_full()
                                .min_w_0()
                                .self_start()
                                .child(Label::new(if task.title.is_empty() {
                                    SharedString::new_static(strings.untitled)
                                } else {
                                    task.title.clone().into()
                                })
                                // As the original: closed tasks are muted, not struck through.
                                .color(if done || wont_do || task.title.is_empty() { Color::Muted } else { Color::Default })
                                .truncate())
                                .debug_selector(|| format!("task-title-{id}"))
                                .when(!read_only, |el| {
                                    el.cursor(gpui::CursorStyle::IBeam).on_click(cx.listener(
                                        move |this, event: &gpui::ClickEvent, window, cx| {
                                            if cx.has_active_drag() {
                                                return;
                                            }
                                            cx.stop_propagation();
                                            if event.modifiers().shift || event.modifiers().secondary() {
                                                this.handle_row_click(id, event.modifiers(), window, cx);
                                                return;
                                            }
                                            this.start_title_edit(id, Some(event.position()), window, cx);
                                        },
                                    ))
                                })
                                .into_any_element()
                        })
                        // "Show Details": the first line of the notes under the title.
                        .when_some(details.then(|| task.notes.lines().find(|l| !l.trim().is_empty())).flatten(), |el, line| {
                            el.child(Label::new(line.trim().to_owned()).size(LabelSize::XSmall).color(Color::Muted).truncate())
                        }),
                    )
                    // A closed task's tags are muted with it, as the original's.
                    .children(task.tags.iter().map(|tag| {
                        let color = tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted);
                        div().flex_none().when(closed_view, |el| el.opacity(0.45)).child(Chip::new(tag.clone(), color))
                    }))
                    // As the original's, a view across lists names each task's list after its tags.
                    .when(self.view.shows_list_names(), |el| {
                        let list = task
                            .list_id
                            .and_then(|id| self.lists.iter().find(|l| l.id == id))
                            .map_or(SharedString::new_static(strings.inbox), |l| l.name.clone().into());
                        el.child(div().flex_none().child(Label::new(list).size(LabelSize::Small).color(Color::Muted)))
                    })
                    .when_some(task.location.as_deref().filter(|s| !s.trim().is_empty()), |el, loc| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .max_w(px(120.))
                                .gap_1()
                                .child(Icon::new(IconName::MapPin).size(IconSize::XSmall).color(Color::Muted))
                                .child(Label::new(loc.trim().to_owned()).size(LabelSize::XSmall).color(Color::Muted).truncate()),
                        )
                    })
                    .when_some(task.checklist, |el, (done_items, total)| {
                        el.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .child(Icon::new(IconName::ListChecks).size(IconSize::XSmall))
                                .child(
                                    Label::new(format!("{done_items}/{total}"))
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .when_some(due, |el, (label, overdue)| {
                        el.when(!task.reminders.is_empty() && !closed_view, |el| {
                            el.child(
                                Icon::new(IconName::AlarmClock)
                                    .size(IconSize::XSmall)
                                    .color(if task.constant_reminder { Color::Accent } else { Color::Muted }),
                            )
                        })
                        .when(task.repeat.is_some(), |el| {
                            el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                        })
                        .when(!closed_view, |el| {
                            el.when_some(
                                crate::due_picker::short_timezone_label(task.timezone.as_deref(), task.is_floating, strings),
                                |el, tz| el.child(Label::new(tz).size(LabelSize::XSmall).color(Color::Muted)),
                            )
                        })
                        .child({
                            let due_label = Label::new(label)
                                .size(LabelSize::Small)
                                .color(if closed_view {
                                    Color::Muted
                                } else if overdue && !done {
                                    Color::Error
                                } else {
                                    Color::Accent
                                });
                            if closed_view {
                                due_label.into_any_element()
                            } else {
                                let due_tip = match (self.preferences.countdown_dates, task.due_date) {
                                    (true, Some(d)) => format!("{} · {}", crate::calendar::format_date(d, strings), strings.show_countdown_mode),
                                    _ => strings.show_countdown_mode.to_owned(),
                                };
                                div()
                                    .id(SharedString::from(format!("due-toggle-{id}")))
                                    .px_1()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(theme.colors().element_hover))
                                    .tooltip(Tooltip::text(due_tip))
                                    .child(due_label)
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.save_preferences(|p| p.countdown_dates = !p.countdown_dates, cx);
                                    }))
                                    .into_any_element()
                            }
                        })
                    })
                    .when_some(horizon_badge, |el, (badge, past)| {
                        let (fg, bg, border) =
                            task_rows::horizon_badge_style(past, done || wont_do || closed_view, &theme);
                        el.when(task.repeat.is_some(), |el| {
                            el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Muted))
                        })
                        .child(
                            div()
                                .flex_none()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .bg(bg)
                                .border_1()
                                .border_color(border)
                                .child(Label::new(badge).size(LabelSize::XSmall).color(Color::Custom(fg))),
                        )
                    })
                    // As the original's, a note with text shows a small page at the row's end.
                    .when(task.kind == fern_core::Kind::Note && !task.notes.trim().is_empty(), |el| {
                        el.child(Icon::new(IconName::FileText).size(IconSize::XSmall).color(Color::Muted))
                    })
                    // And a task with subtasks a small tree + progress count, open or folded.
                    .when(has_children, |el| {
                        let (done_subs, total_subs) = self.subtask_progress.get(&id).copied().unwrap_or((0, 0));
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(IconName::ListTree).size(IconSize::XSmall).color(Color::Muted))
                                .when(total_subs > 0, |row| {
                                    row.child(
                                        Label::new(format!("{done_subs}/{total_subs}"))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                }),
                        )
                    })
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            h_flex()
                                .id(SharedString::from(format!("trash-actions-{id}")))
                                .flex_none()
                                .gap_1()
                                .opacity(if crate::touch::active() { 1. } else { 0. })
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .child(
                                    IconButton::new(SharedString::from(format!("restore-button-{id}")), IconName::RotateCcw)
                                        .size(px(20.))
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted)
                                        .hover_icon_color(Color::Icon)
                                        .tooltip(Tooltip::text(strings.restore))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.restore(id, cx);
                                        })),
                                )
                                .child(
                                    IconButton::new(SharedString::from(format!("purge-button-{id}")), IconName::Trash)
                                        .size(px(20.))
                                        .icon_size(IconSize::Small)
                                        .icon_color(Color::Muted)
                                        .hover_icon_color(theme.status().error)
                                        .tooltip(Tooltip::text(strings.delete_forever))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.request_purge_task(id, cx);
                                        })),
                                ),
                        )
                    }),
            )
            // Keep the menu button inside the row, including in Trash and while editing,
            // but omit it for read-only archived-list tasks (matching the original).
            .when(!read_only || task.is_deleted(), |el| {
                let more_group = SharedString::from(format!("row-more-{id}"));
                el.child(
                    div()
                        .id("row-more")
                        .group(more_group.clone())
                        .absolute()
                        .right(px(6.))
                        .top_0()
                        .w(px(20.))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .child(
                            Icon::new(IconName::Ellipsis)
                                .size(IconSize::XSmall)
                                .color(Color::Muted)
                                .group_hover_color(more_group, Color::Icon),
                        )
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_context_menu(id, event.position, true, window, cx);
                        })),
                )
            })
            // And the drag handle left of a hovered row, as the original's.
            .when(draggable, |el| {
                let drag_group = SharedString::from(format!("row-drag-{id}"));
                el.child(
                    div()
                        .group(drag_group.clone())
                        .absolute()
                        .left(px(-13.))
                        .top_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(
                            Icon::new(IconName::Menu)
                                .size(IconSize::XSmall)
                                .color(Color::Muted)
                                .group_hover_color(drag_group, Color::Icon),
                        ),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_list_pane(
        &self,
        board: Option<AnyElement>,
        kanban_view: bool,
        kanban_only: bool,
        closed_header: bool,
        add_focused: bool,
        input_focus: FocusHandle,
        rows_empty: bool,
        strings: &'static Strings,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .mt(crate::title_bar::header_inset())
                    .h(px(60.))
                    // The title sits 4 px above the row's middle, as the original's.
                    .pb(px(8.))
                    .flex_none()
                    .gap_2()
                    .pl(px(24.))
                    .pr(px(16.))
                    .child(
                        IconButton::new("toggle-sidebar", IconName::PanelLeft)
                            .tooltip(Tooltip::text(strings.toggle_sidebar))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_lists(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .map(|el| {
                                if matches!(self.view, View::Search(_)) {
                                    el.child(
                                        h_flex()
                                            .gap_2()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::NORMAL)
                                            .child(Icon::new(IconName::Search).size(IconSize::Large))
                                            .child(self.search_input.clone()),
                                    )
                                } else {
                                    el.truncate().child(self.view_title(strings))
                                }
                            }),
                    )
                    .when(self.view == View::Trash, |el| {
                        el.child(
                            div().debug_selector(|| "empty-trash-button".to_owned()).child(
                                IconButton::new("empty-trash-button", IconName::BrushCleaning)
                                    .tooltip(Tooltip::text(strings.empty_trash))
                                    .on_click(cx.listener(|this, _, _, cx| this.request_empty_trash(cx))),
                            ),
                        )
                    })
                    // The original's light bulb in Today: Suggested Tasks.
                    .when(self.view == View::Today, |el| {
                        el.child(
                            IconButton::new("suggestions", IconName::Lightbulb)
                                .toggle_state(self.suggestions_open.is_some())
                                .tooltip(Tooltip::text(strings.suggested_tasks))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.suggestions_open.is_some() {
                                        this.suggestions_open = None;
                                    } else {
                                        this.refresh_suggestions();
                                        this.suggestions_open = Some(false);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    // The Completed and Won't Do views have only Print, as the original's.
                    .when(closed_header, |el| el.child(IconButton::new("print-closed", IconName::Printer).tooltip(Tooltip::text(strings.print)).on_click(cx.listener(|this, _, _, cx| this.print_list(cx)))))
                    .when(kanban_only, |el| {
                        let workflow_on = self.kanban_workflow;
                        el.child(
                            h_flex()
                                .id("kanban-workflow-toggle")
                                .h(px(26.))
                                .px_2()
                                .gap_1()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(workflow_on, |el| el.bg(theme.colors().element_selected))
                                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                .child(
                                    Icon::new(IconName::Kanban)
                                        .size(IconSize::XSmall)
                                        .color(if workflow_on { Color::Accent } else { Color::Muted }),
                                )
                                .child(
                                    Label::new(strings.kanban_workflow)
                                        .size(LabelSize::Small)
                                        .color(if workflow_on { Color::Accent } else { Color::Muted }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_kanban_workflow(cx))),
                        )
                    })
                    .when(self.view != View::Trash && !closed_header, |el| {
                        el.child(
                            div()
                                .relative()
                                .debug_selector(|| "task-sort-menu-trigger".to_owned())
                                .child(IconButton::new("sort", IconName::ArrowDownUp).tip(strings.group_and_sort, ui::TipSide::Below).on_click(cx.listener(|this, _, window, cx| this.open_sort_menu(window, cx))))
                                .when_some(self.sort_menu, |el, expanded| {
                                    el.child(PopoverAttachment::new(Placement::BelowRight, self.render_sort_menu(expanded, strings, cx)))
                                }),
                        )
                    })
                    .when(self.view != View::Trash && !closed_header, |el| el.child(
                        div()
                            .relative()
                            .child(IconButton::new("more", IconName::Ellipsis).tip(strings.more, ui::TipSide::Below).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.header_menu_open = true;
                                    cx.notify();
                                },
                            )))
                            .when(self.header_menu_open, |el| {
                                el.child(PopoverAttachment::new(
                                    Placement::BelowRight,
                                    self.render_header_menu(strings, cx),
                                ))
                            }),
                    )),
            )
            .when(matches!(self.view, View::Completed | View::WontDo), |el| {
                el.child(self.render_closed_filters(strings, cx))
            })
            .when(!self.is_read_only_view() && !kanban_view && self.preferences.detailed_input, |el| el.child(self.render_detailed_add(strings, cx)))
            .when(!self.is_read_only_view() && !kanban_view && !self.preferences.detailed_input, |el| el.child(
                // As in the original: 20 px in from the column's edge, lined up with the rows,
                // and outlined in the accent while focused.
                h_flex()
                    .mx(px(20.))
                    .mb_2()
                    .h(px(38.))
                    .flex_none()
                    .gap_2()
                    .px_3()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if add_focused { theme.colors().accent } else { gpui::transparent_black() })
                    .bg(theme.colors().surface_background)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&input_focus, cx);
                    })
                    .child(Icon::new(IconName::Plus))
                    .child(div().flex_1().min_w_0().debug_selector(|| "quick-add-input".to_owned()).child(self.input.clone()))
                    .children(self.shown_add_due().map(|(date, time)| {
                        let (label, _) = task_rows::due_label(date, time, Self::today(), strings);
                        // A picked duration shows its span, as the original's "Today, 22:00 - 23:00".
                        let label = match self.add_draft.end.filter(|_| self.quick_add_due.is_none()) {
                            Some((end, end_time)) => crate::due_picker::range_label(date, time, end, end_time, strings),
                            None => label,
                        };
                        let has_repeat = self.quick_add_repeat || self.add_draft.repeat.is_some();
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(Icon::new(IconName::CalendarDays).size(IconSize::Small).color(Color::Accent))
                            .when(has_repeat, |el| {
                                el.child(Icon::new(IconName::Repeat).size(IconSize::XSmall).color(Color::Accent))
                            })
                            .child(Label::new(label).size(LabelSize::Small).color(Color::Accent))
                    }))
                    .when(self.shown_add_due().is_none(), |el| {
                        el.when_some(self.shown_add_horizon(), |el, h| {
                            el.child(
                                div()
                                    .flex_none()
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(theme.colors().accent.opacity(0.12))
                                    .child(
                                        Label::new(crate::calendar::format_horizon_badge(h, strings))
                                            .size(LabelSize::XSmall)
                                            .color(Color::Accent),
                                    ),
                            )
                        })
                    })
                    .when(self.shown_add_due().is_none() && (self.quick_add_repeat || self.add_draft.repeat.is_some()), |el| {
                        el.child(Icon::new(IconName::Repeat).size(IconSize::Small).color(Color::Accent))
                    })
                    .child(
                        div()
                            .relative()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                h_flex().child(self.add_date_button(Placement::BelowCenter, strings, cx)).child(
                                // The original's "▾": priority, list, template and note for the
                                // next task; its flag takes the chosen priority's color.
                                IconButton::new("add-options", IconName::ChevronDown)
                                    .size(px(24.))
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(strings.more))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_menu_open = !this.add_menu_open;
                                        this.add_list_side = false;
                                        cx.notify();
                                    })),
                            ))
                            .map(|el| self.add_field_popovers(el, strings, cx)),
                    ),
            ))
            .children(board)
            .when(!kanban_view, |el| el.child(
                self.bind_task_list_actions(
                    div().id("tasks").track_focus(&self.list_focus),
                    cx,
                )
                    .on_drag_move(cx.listener(Self::move_task_drag))
                    .on_drop(cx.listener(Self::drop_task_in_list))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child({
                        let bounds = self.list_entry_bounds.clone();
                        gpui::canvas(move |_, _, _| bounds.borrow_mut().clear(), |_, _, _, _| {})
                            .absolute()
                            .size_0()
                    })
                    .when(rows_empty, |el| {
                        el.child(
                            div()
                                .w_full()
                                .pt(px(80.))
                                .flex_none()
                                .child(self.render_empty_state(strings, theme, px(12.))),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .child(
                                list(self.list_state.clone(), cx.processor(|this, ix, _, cx| this.render_entry(ix, cx)))
                                    .size_full()
                                    .pb_4(),
                            )
                            .child(VerticalScrollbar::for_list("task-list-scroll", self.list_state.clone())),
                    ),
            ))
            .when_some(self.error.clone().or_else(|| self.add_attachment_error.clone()), |el, error| {
                el.child(
                    div()
                        .px_6()
                        .py_2()
                        .border_t_1()
                        .border_color(theme.colors().border)
                        .child(Label::new(error).color(Color::Error)),
                )
            })
    }
}
