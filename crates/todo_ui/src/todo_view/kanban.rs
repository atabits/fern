use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KanbanSub {
    Pinned,
    Unpinned,
    Completed,
}

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

pub(super) struct KanbanColRows {
    pub(super) key: String,
    pub(super) drop: Option<DropTarget>,
    pub(super) rows: Vec<Row>,
    pub(super) row_subs: HashMap<Id, KanbanSub>,
}

pub(super) type BoardCard = (Task, Vec<Task>);

pub(super) struct BoardCol {
    pub(super) key: String,
    pub(super) title: String,
    pub(super) drop: Option<DropTarget>,
    pub(super) pinned_cards: Vec<BoardCard>,
    pub(super) open_cards: Vec<BoardCard>,
    pub(super) completed_cards: Vec<BoardCard>,
}

impl BoardCol {
    pub(super) fn open_count(&self) -> usize {
        self.pinned_cards.len() + self.open_cards.len()
    }

    pub(super) fn total_count(&self) -> usize {
        self.open_count() + self.completed_cards.len()
    }
}


impl TodoView {
    pub(super) fn ensure_kanban_sections(&mut self, strings: &Strings) {
        if !self.kanban
            || self.timeline
            || matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_))
        {
            return;
        }
        let Some(owner) = self.section_owner() else { return };
        if self.sections.is_empty()
            && let Some(section) = self.check(self.store.add_list_section(owner, strings.not_sectioned))
        {
            self.sections.push(section);
        }
        let Some(first_id) = self.sections.first().map(|s| s.id) else { return };
        if self.sort_settings().1 != GroupBy::Custom {
            return;
        }
        let known: HashSet<Id> = self.sections.iter().map(|s| s.id).collect();
        let unsectioned_roots: Vec<Id> = self
            .tasks
            .iter()
            .filter(|t| t.list_id == owner && t.parent_id.is_none() && t.section_id.is_none_or(|s| !known.contains(&s)))
            .map(|t| t.id)
            .collect();
        if unsectioned_roots.is_empty() {
            return;
        }
        for root_id in &unsectioned_roots {
            let result = self.store.set_section(*root_id, Some(first_id));
            self.check(result);
        }
        let mut moved: HashSet<Id> = unsectioned_roots.into_iter().collect();
        loop {
            let mut added = false;
            for t in &self.tasks {
                if let Some(p) = t.parent_id
                    && moved.contains(&p)
                    && moved.insert(t.id)
                {
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        for task in &mut self.tasks {
            if moved.contains(&task.id) {
                task.section_id = Some(first_id);
            }
        }
    }

    pub(super) fn board_columns(&self, strings: &Strings) -> Vec<BoardCol> {
        let (sort, group) = self.sort_settings();
        let show_completed = self.view_flag("show_completed", true);
        let descending = self.view_flag("descending", false);
        let today = Self::today();

        let mut tasks = self.tasks_for_rows();
        if !show_completed {
            tasks.retain(|t| !t.is_completed());
        }
        task_rows::order_tasks(&mut tasks, sort, descending);

        let ids: HashSet<Id> = tasks.iter().map(|t| t.id).collect();
        let mut children_map: HashMap<Id, Vec<&Task>> = HashMap::new();
        let mut roots: Vec<&Task> = Vec::new();
        for task in &tasks {
            match task.parent_id.filter(|p| ids.contains(p)) {
                Some(parent) => children_map.entry(parent).or_default().push(task),
                None => roots.push(task),
            }
        }
        fn collect_descendants(
            parent: Id,
            map: &HashMap<Id, Vec<&Task>>,
            visited: &mut HashSet<Id>,
            out: &mut Vec<Task>,
        ) {
            if let Some(kids) = map.get(&parent) {
                for kid in kids {
                    if !visited.insert(kid.id) {
                        continue;
                    }
                    out.push((*kid).clone());
                    collect_descendants(kid.id, map, visited, out);
                }
            }
        }
        let all_cards: Vec<(Task, Vec<Task>)> = roots
            .into_iter()
            .map(|root| {
                let mut subs = Vec::new();
                let mut visited = HashSet::from([root.id]);
                collect_descendants(root.id, &children_map, &mut visited, &mut subs);
                (root.clone(), subs)
            })
            .collect();

        let make_col = |key: String, title: String, drop: Option<DropTarget>, cards: Vec<(Task, Vec<Task>)>| -> BoardCol {
            let mut pinned_cards = Vec::new();
            let mut open_cards = Vec::new();
            let mut completed_cards = Vec::new();
            for card in cards {
                if card.0.is_completed() {
                    completed_cards.push(card);
                } else if card.0.pinned_at.is_some() {
                    pinned_cards.push(card);
                } else {
                    open_cards.push(card);
                }
            }
            if sort == SortBy::Custom {
                pinned_cards.sort_by_key(|(t, _)| std::cmp::Reverse(t.pinned_at));
            }
            completed_cards.sort_by_key(|(t, _)| std::cmp::Reverse(t.completed_at.unwrap_or(0)));
            BoardCol { key, title, drop, pinned_cards, open_cards, completed_cards }
        };

        if self.kanban_workflow {
            let mut todo_cards = Vec::new();
            let mut in_progress_cards = Vec::new();
            let mut completed_cards = Vec::new();
            for card in all_cards {
                if card.0.is_completed() {
                    completed_cards.push(card);
                } else if card.0.is_in_progress() {
                    in_progress_cards.push(card);
                } else {
                    todo_cards.push(card);
                }
            }
            return vec![
                make_col("workflow:todo".into(), strings.col_todo.to_owned(), Some(DropTarget::Todo), todo_cards),
                make_col("workflow:in_progress".into(), strings.in_progress.to_owned(), Some(DropTarget::InProgress), in_progress_cards),
                make_col("workflow:completed".into(), strings.completed.to_owned(), Some(DropTarget::Complete), completed_cards),
            ];
        }

        match group {
            GroupBy::Custom if self.view == View::Next7Days => {
                let week_start = crate::calendar::week_start();
                let mut cols = Vec::new();
                let mut overdue = Vec::new();
                let mut by_day: Vec<Vec<(Task, Vec<Task>)>> = (0..7).map(|_| Vec::new()).collect();
                for card in all_cards {
                    match card.0.due_date {
                        Some(d) if d < today => overdue.push(card),
                        Some(d) if (d - today).num_days() < 7 => by_day[(d - today).num_days().max(0) as usize].push(card),
                        _ => by_day[0].push(card),
                    }
                }
                if !overdue.is_empty() {
                    cols.push(make_col("next7:overdue".into(), strings.group_overdue.to_owned(), None, overdue));
                }
                for (offset, cards) in by_day.into_iter().enumerate() {
                    if !cards.is_empty() {
                        let day = today + Duration::days(offset as i64);
                        let label = task_rows::next_days_header(day, today, week_start, strings);
                        cols.push(make_col(format!("next7:{day}"), label, Some(DropTarget::Due(Some(day))), cards));
                    }
                }
                cols
            }
            GroupBy::Custom if self.section_owner().is_some() => {
                if self.sections.is_empty() {
                    return vec![make_col(
                        "sec:none".into(),
                        strings.not_sectioned.to_owned(),
                        Some(DropTarget::Section(None)),
                        all_cards,
                    )];
                }
                let known: HashSet<Id> = self.sections.iter().map(|s| s.id).collect();
                let first_id = self.sections[0].id;
                let mut buckets: HashMap<Id, Vec<(Task, Vec<Task>)>> =
                    self.sections.iter().map(|s| (s.id, Vec::new())).collect();
                for card in all_cards {
                    let target_sec = card.0.section_id.filter(|s| known.contains(s)).unwrap_or(first_id);
                    if let Some(bucket) = buckets.get_mut(&target_sec) {
                        bucket.push(card);
                    }
                }
                self.sections
                    .iter()
                    .map(|sec| {
                        let cards = buckets.remove(&sec.id).unwrap_or_default();
                        make_col(format!("sec:{}", sec.id), sec.name.clone(), Some(DropTarget::Section(Some(sec.id))), cards)
                    })
                    .collect()
            }
            GroupBy::Custom => {
                let mut task_cards = Vec::new();
                let mut note_cards = Vec::new();
                for card in all_cards {
                    if card.0.kind == Kind::Note {
                        note_cards.push(card);
                    } else {
                        task_cards.push(card);
                    }
                }
                let mut cols = Vec::new();
                if !task_cards.is_empty() {
                    cols.push(make_col("kind:task".into(), strings.group_tasks.to_owned(), Some(DropTarget::Kind(Kind::Task)), task_cards));
                }
                if !note_cards.is_empty() {
                    cols.push(make_col("kind:note".into(), strings.group_notes.to_owned(), Some(DropTarget::Kind(Kind::Note)), note_cards));
                }
                cols
            }
            GroupBy::List => {
                let known: HashSet<Id> = self.lists.iter().map(|l| l.id).collect();
                let mut inbox_cards = Vec::new();
                let mut list_buckets: HashMap<Id, Vec<(Task, Vec<Task>)>> = HashMap::new();
                for card in all_cards {
                    match card.0.list_id.filter(|id| known.contains(id)) {
                        Some(id) => list_buckets.entry(id).or_default().push(card),
                        None => inbox_cards.push(card),
                    }
                }
                let mut cols = Vec::new();
                if !inbox_cards.is_empty() {
                    cols.push(make_col("list:inbox".into(), strings.inbox.to_owned(), Some(DropTarget::List(None)), inbox_cards));
                }
                for list in &self.lists {
                    if let Some(cards) = list_buckets.remove(&list.id)
                        && !cards.is_empty()
                    {
                        cols.push(make_col(format!("list:{}", list.id), list.name.clone(), Some(DropTarget::List(Some(list.id))), cards));
                    }
                }
                cols
            }
            GroupBy::Date => {
                let overdue_last = self.preferences.task_defaults.overdue_last;
                let mut buckets: [Vec<(Task, Vec<Task>)>; 6] = Default::default();
                for card in all_cards {
                    let slot = match card.0.due_date {
                        Some(d) if d < today => 0,
                        Some(d) if d == today => 1,
                        Some(d) if d == today + Duration::days(1) => 2,
                        Some(d) if d <= today + Duration::days(7) => 3,
                        Some(_) => 4,
                        None => 5,
                    };
                    buckets[slot].push(card);
                }
                let mut specs = vec![
                    (if overdue_last { 6 } else { 0 }, 0usize, "date:overdue", strings.group_overdue, None),
                    (1, 1, "date:today", strings.today, Some(DropTarget::Due(Some(today)))),
                    (2, 2, "date:tomorrow", strings.tomorrow, Some(DropTarget::Due(Some(today + Duration::days(1))))),
                    (3, 3, "date:next7", strings.next_7_days, Some(DropTarget::Due(Some(today + Duration::days(7))))),
                    (4, 4, "date:later", strings.group_later, None),
                    (5, 5, "date:none", strings.group_no_date, Some(DropTarget::Due(None))),
                ];
                specs.sort_by_key(|(ord, _, _, _, _)| *ord);
                let mut cols = Vec::new();
                for (_, slot, key, label, drop) in specs {
                    let cards = std::mem::take(&mut buckets[slot]);
                    if !cards.is_empty() {
                        cols.push(make_col(key.into(), label.to_owned(), drop, cards));
                    }
                }
                cols
            }
            GroupBy::Priority => {
                let specs = [
                    (Priority::High, "prio:high", strings.priority_high),
                    (Priority::Medium, "prio:medium", strings.priority_medium),
                    (Priority::Low, "prio:low", strings.priority_low),
                    (Priority::None, "prio:none", strings.priority_none),
                ];
                let mut buckets: [Vec<(Task, Vec<Task>)>; 4] = Default::default();
                for card in all_cards {
                    let idx = match card.0.priority {
                        Priority::High => 0,
                        Priority::Medium => 1,
                        Priority::Low => 2,
                        Priority::None => 3,
                    };
                    buckets[idx].push(card);
                }
                specs
                    .into_iter()
                    .zip(buckets)
                    .filter(|(_, cards)| !cards.is_empty())
                    .map(|((prio, key, label), cards)| {
                        make_col(key.into(), label.to_owned(), Some(DropTarget::Priority(prio)), cards)
                    })
                    .collect()
            }
            GroupBy::Tag => {
                let mut tag_groups: Vec<(String, Vec<BoardCard>)> = Vec::new();
                let mut untagged = Vec::new();
                for card in all_cards {
                    match card.0.tags.first().cloned() {
                        Some(tag) => match tag_groups.iter_mut().find(|(t, _)| *t == tag) {
                            Some((_, items)) => items.push(card),
                            None => tag_groups.push((tag, vec![card])),
                        },
                        None => untagged.push(card),
                    }
                }
                tag_groups.sort_by_key(|(a, _)| a.to_lowercase());
                let mut cols: Vec<BoardCol> = tag_groups
                    .into_iter()
                    .map(|(tag, cards)| make_col(format!("tag:{tag}"), tag.clone(), Some(DropTarget::Tag(Some(tag))), cards))
                    .collect();
                if !untagged.is_empty() {
                    cols.push(make_col("tag:none".into(), strings.group_no_tags.to_owned(), Some(DropTarget::Tag(None)), untagged));
                }
                cols
            }
            GroupBy::Created | GroupBy::None => {
                let mut groups: Vec<(NaiveDate, Vec<BoardCard>)> = Vec::new();
                for card in all_cards {
                    let day = task_rows::local_day(card.0.created_at).unwrap_or(today);
                    match groups.iter_mut().find(|(d, _)| *d == day) {
                        Some((_, items)) => items.push(card),
                        None => groups.push((day, vec![card])),
                    }
                }
                groups.sort_by_key(|(d, _)| std::cmp::Reverse(*d));
                groups
                    .into_iter()
                    .map(|(day, cards)| make_col(format!("created:{day}"), task_rows::day_header(day, today, strings), None, cards))
                    .collect()
            }
        }
    }

    /// Flattens a kanban subsection's cards into visible rows preserving real multi-level
    /// nesting depth, per-parent collapse state, and `KanbanItemUi` card-slice role.
    pub(super) fn kanban_visible_rows(&self, cards: &[BoardCard]) -> Vec<KanbanVisibleRow> {
        fn count_descendants(
            id: Id,
            children: &HashMap<Id, Vec<&Task>>,
            visited: &mut HashSet<Id>,
        ) -> (usize, usize) {
            let mut done = 0;
            let mut total = 0;
            if let Some(kids) = children.get(&id) {
                for kid in kids {
                    if !visited.insert(kid.id) {
                        continue;
                    }
                    total += 1;
                    if kid.is_completed() {
                        done += 1;
                    }
                    let (d, t) = count_descendants(kid.id, children, visited);
                    done += d;
                    total += t;
                }
            }
            (done, total)
        }
        fn push_tree(
            task: &Task,
            depth: usize,
            children: &HashMap<Id, Vec<&Task>>,
            collapsed_tasks: &HashSet<Id>,
            visited: &mut HashSet<Id>,
            out: &mut Vec<KanbanVisibleRow>,
        ) {
            let kids = children.get(&task.id);
            let has_children = kids.is_some_and(|k| !k.is_empty());
            let collapsed = has_children && collapsed_tasks.contains(&task.id);
            let (descendant_done, descendant_total) = if collapsed {
                let mut count_visited = HashSet::from([task.id]);
                count_descendants(task.id, children, &mut count_visited)
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
            if !collapsed {
                for kid in kids.into_iter().flatten() {
                    if visited.insert(kid.id) {
                        push_tree(kid, depth + 1, children, collapsed_tasks, visited, out);
                    }
                }
            }
        }

        let mut out = Vec::new();
        for (root, subs) in cards {
            let mut card_ids: HashSet<Id> = HashSet::with_capacity(subs.len() + 1);
            card_ids.insert(root.id);
            for sub in subs {
                card_ids.insert(sub.id);
            }
            let mut children: HashMap<Id, Vec<&Task>> = HashMap::new();
            for sub in subs {
                let parent = sub.parent_id.filter(|p| card_ids.contains(p)).unwrap_or(root.id);
                children.entry(parent).or_default().push(sub);
            }
            let mut visited = HashSet::from([root.id]);
            push_tree(root, 0, &children, &self.collapsed_tasks, &mut visited, &mut out);
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
        let due = task.due_date.map(|date| task_rows::row_due_label(date, task.due_time, today, strings, self.preferences.countdown_dates));
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
        let has_tips_row = due.is_some()
            || has_note_icon
            || has_location
            || task.repeat.is_some()
            || has_reminders
            || checklist.is_some()
            || (collapsed && has_children)
            || list_info.is_some();
        let has_meta = details_line.is_some() || !task.tags.is_empty() || has_tips_row;

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
        let draggable = self.view != View::Trash && title_input.is_none();
        let dragged = DraggedTask { id, title: task.title.clone().into() };
        let dragging_active = cx.has_active_drag();
        let is_being_dragged = dragging_active && self.task_drag_origin.is_some_and(|(d, _)| d == id);
        let drop_indicator = dragging_active
            .then(|| self.task_drop.as_ref().filter(|plan| plan.target_id == id).cloned())
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
                base_surface.blend(gpui::white().opacity(0.06))
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
        let row_bounds = self.kanban_row_bounds.clone();
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if cx.has_active_drag() {
                                return;
                            }
                            cx.stop_propagation();
                            this.toggle(id, !done, cx);
                        }));
                    // A finger gets a wider target around the box, which does the same.
                    el.child(
                        div()
                            .id(SharedString::from(format!("card-box-{id}")))
                            .pt(px(2.))
                            .debug_selector(move || format!("kanban-checkbox-{id}"))
                            .when(crate::touch::active(), |el| {
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            if event.click_count > 1 {
                                cx.stop_propagation();
                                this.close_card(cx);
                                this.start_title_edit(id, Some(event.position), window, cx);
                            }
                        }),
                    )
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
                        .when(!task.tags.is_empty(), |el| {
                            el.child(h_flex().gap_1().flex_wrap().children(task.tags.iter().map(|tag| {
                                Chip::new(tag.clone(), tag_colors.get(tag).copied().unwrap_or(theme.colors().text_muted))
                            })))
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
                            .kanban_row_bounds
                            .borrow()
                            .get(&dragged_id)
                            .copied()
                            .map_or(fallback_x, |area| area.left() + cursor_offset.x);
                        this.task_drag_origin = Some((dragged_id, start_x));
                        this.task_drop = None;
                        this.kanban_hover_col = None;
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_context_menu(id, event.position, false, window, cx);
                }),
            )
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


    /// The original's column widths: Small 242 px, Medium 282 px, Large 342 px.
    pub(super) fn kanban_column_width(&self) -> f32 {
        [242., 282., 342.][self.kanban_column_size.min(2) as usize]
    }

    pub(super) fn set_kanban_column_size(&mut self, size: u8, cx: &mut Context<Self>) {
        let size = size.min(2);
        let key = ["s", "m", "l"][size as usize];
        let result = self.store.set_setting("kanban:column_size", key);
        self.check(result);
        self.kanban_column_size = size;
        cx.notify();
    }

    pub(super) fn toggle_kanban_column_color(&mut self, cx: &mut Context<Self>) {
        let next = !self.kanban_column_color;
        let result = self.store.set_setting("kanban:column_color", if next { "1" } else { "0" });
        self.check(result);
        self.kanban_column_color = next;
        cx.notify();
    }

    pub(super) fn toggle_kanban_quick_add(&mut self, cx: &mut Context<Self>) {
        let next = !self.kanban_quick_add;
        let result = self.store.set_setting("kanban:quick_add", if next { "1" } else { "0" });
        self.check(result);
        self.kanban_quick_add = next;
        if next {
            self.column_add = None;
        } else {
            self.kanban_quick_adds.clear();
        }
        cx.notify();
    }

    pub(super) fn toggle_kanban_workflow(&mut self, cx: &mut Context<Self>) {
        let next = !self.kanban_workflow;
        let key = format!("kanban_workflow:{}", self.view_key());
        let result = self.store.set_setting(&key, if next { "1" } else { "0" });
        self.check(result);
        self.kanban_workflow = next;
        cx.notify();
    }

    pub(super) fn set_section_color(&mut self, section: Id, color: Option<&str>, cx: &mut Context<Self>) {
        let value = color.unwrap_or("");
        let result = self.store.set_setting(&format!("kanban:section_color:{section}"), value);
        self.check(result);
        if value.is_empty() {
            self.kanban_section_colors.remove(&section);
        } else {
            self.kanban_section_colors.insert(section, value.to_owned());
        }
        self.column_menu = None;
        cx.notify();
    }

    /// TickTick's column tint palette (used when Column Color is enabled).
    const KANBAN_COLUMN_COLORS: [&'static str; 8] = [
        "#6387F7", "#2FBFBC", "#52C41A", "#FAAD14", "#FF5C62", "#B069EE", "#F06292", "#8F8F8F",
    ];

    pub(super) fn kanban_column_tint(
        &self,
        index: usize,
        target: Option<&DropTarget>,
        title: &str,
        strings: &Strings,
        tag_colors: &HashMap<String, Hsla>,
        accent: Hsla,
    ) -> Option<Hsla> {
        if !self.kanban_column_color {
            return None;
        }
        let hex = |s: &str| lists_panel::parse_color(Some(s));
        match target {
            Some(DropTarget::Todo) => hex("#6387F7"),
            Some(DropTarget::InProgress) => hex("#FAAD14"),
            Some(DropTarget::Complete) => hex("#A5A5A5"),
            Some(DropTarget::Section(Some(id))) => self
                .kanban_section_colors
                .get(id)
                .and_then(|c| hex(c))
                .or_else(|| hex(Self::KANBAN_COLUMN_COLORS[index % Self::KANBAN_COLUMN_COLORS.len()])),
            Some(DropTarget::Section(None)) => hex(Self::KANBAN_COLUMN_COLORS[index % Self::KANBAN_COLUMN_COLORS.len()]),
            Some(DropTarget::List(Some(id))) => self
                .lists
                .iter()
                .find(|l| l.id == *id)
                .and_then(|l| lists_panel::parse_color(l.color.as_deref()))
                .or(Some(accent)),
            Some(DropTarget::List(None)) => Some(accent),
            Some(DropTarget::Priority(p)) => match p {
                Priority::High => hex("#F6252E"),
                Priority::Medium => hex("#FFA300"),
                Priority::Low => hex("#4772FA"),
                Priority::None => hex("#8F8F8F"),
            },
            Some(DropTarget::Tag(Some(name))) => tag_colors.get(name).copied().or(Some(accent)),
            Some(DropTarget::Tag(None)) => hex("#8F8F8F"),
            Some(DropTarget::Due(Some(date))) => {
                let today = Self::today();
                let days = (*date - today).num_days();
                if days < 0 {
                    hex("#FF5C62")
                } else if days == 0 {
                    hex("#6387F7")
                } else if days == 1 {
                    hex("#FAAD14")
                } else if days <= 7 {
                    hex("#2FBFBC")
                } else {
                    hex("#52C41A")
                }
            }
            Some(DropTarget::Due(None)) => hex("#8F8F8F"),
            Some(DropTarget::Kind(_)) => hex("#A5A5A5"),
            None => {
                if title == strings.group_overdue {
                    hex("#FF5C62")
                } else if title == strings.group_completed {
                    hex("#A5A5A5")
                } else {
                    hex(Self::KANBAN_COLUMN_COLORS[index % Self::KANBAN_COLUMN_COLORS.len()])
                }
            }
        }
    }

    pub(super) fn sync_kanban_quick_adds(&mut self, columns: &[BoardCol], strings: &'static Strings, cx: &mut Context<Self>) {
        if !self.kanban_quick_add {
            if !self.kanban_quick_adds.is_empty() {
                self.kanban_quick_adds.clear();
            }
            return;
        }
        let mut active_keys = HashSet::new();
        for col in columns {
            let Some(target) = col.drop.as_ref().filter(|t| !matches!(t, DropTarget::Complete)) else { continue };
            let key = col.key.clone();
            active_keys.insert(key.clone());
            let entry = self.kanban_quick_adds.entry(key.clone()).or_insert_with(|| {
                let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
                let key_for_sub = key;
                cx.subscribe(&input, |this, input, _: &Changed, cx| {
                    let text = input.read(cx).text().to_owned();
                    let list_names: Vec<String> = this.lists.iter().map(|l| l.name.clone()).collect();
                    let parsed = fern_core::parse_quick_add_with(&text, Self::today(), &list_names, this.preferences.recognition);
                    input.update(cx, |input, cx| input.set_highlights(parsed.spans, cx));
                })
                .detach();
                cx.subscribe(&input, move |this, input, Submitted(text), cx| {
                    if !text.trim().is_empty() {
                        input.update(cx, |input, cx| input.set_text("", cx));
                        if let Some(task) = this.add_task(text, cx) {
                            if let Some((target, _)) = this.kanban_quick_adds.get(&key_for_sub).cloned() {
                                this.drop_on_column(task.id, target, cx);
                                return;
                            }
                            this.reload(cx);
                        }
                    }
                })
                .detach();
                (target.clone(), input)
            });
            entry.0 = target.clone();
        }
        self.kanban_quick_adds.retain(|k, _| active_keys.contains(k));
    }

    /// A kanban column's "+": a field at the column's top; the task it adds takes the column's
    /// value, as one dropped there.
    pub(super) fn start_column_add(&mut self, column: usize, target: DropTarget, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
        cx.subscribe(&input, |this, input, _: &Changed, cx| {
            let text = input.read(cx).text().to_owned();
            let list_names: Vec<String> = this.lists.iter().map(|l| l.name.clone()).collect();
            let parsed = fern_core::parse_quick_add_with(&text, Self::today(), &list_names, this.preferences.recognition);
            input.update(cx, |input, cx| input.set_highlights(parsed.spans, cx));
        })
        .detach();
        cx.subscribe_in(&input, window, move |this, input, Submitted(text), window, cx| {
            if !text.trim().is_empty() {
                input.update(cx, |input, cx| input.clear(window, cx));
                if let Some(task) = this.add_task(text, cx) {
                    if let Some((_, target, _)) = this.column_add.clone() {
                        this.drop_on_column(task.id, target, cx);
                        return;
                    }
                    this.reload(cx);
                }
            }
        })
        .detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.column_add = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.column_add = Some((column, target, input));
        cx.notify();
    }

    pub(super) fn start_kanban_section_rename(&mut self, section: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.start_rename_section(section, window, cx);
    }

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
                Some(input) => el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(2.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_section_rename(cx)))
                        .child(input),
                ),
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
            .when_some(target.filter(|t| !matches!(t, DropTarget::Complete) && !self.kanban_quick_add), |el, target| {
                el.child(
                    IconButton::new(SharedString::from(format!("kanban-add-{column}")), IconName::Plus)
                        .size(px(22.))
                        .icon_size(IconSize::Small)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.add_task))
                        .on_click(cx.listener(move |this, _, window, cx| this.start_column_add(column, target.clone(), window, cx))),
                )
            })
            .when_some(section, |el, id| {
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
            DropTarget::Due(date) => store.set_due(id, date, date.and(task.due_time)),
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
            if let Some(col_target) = column_drop {
                self.apply_column_value(id, col_target)?;
            }
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
                    if !task.is_completed() {
                        self.store.set_completed(id, true)?;
                    }
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
            match target_sub {
                Some(KanbanSub::Completed) => {
                    if !dragged_task.is_completed() {
                        self.store.set_completed(dragged, true)?;
                    }
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
            self.store.move_task_in_tree(dragged, plan.new_parent, plan.order_target)?;
            Ok(())
        })();
        if self.check(result).is_some()
            && let Some(parent_id) = plan.expand_parent
        {
            self.collapsed_tasks.remove(&parent_id);
        }
        self.reload(cx);
    }


    /// The kanban's Habit column in Today and Next 7 Days, as the original's: a card per habit
    /// due today with its icon and name and "Today 0 / 2 unit" (or "0 / 1" without a goal); a
    /// click checks it in or adds a step.
    pub(super) fn render_habit_column(&self, strings: &'static Strings, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let today = Self::today();
        let habits: Vec<(fern_core::Habit, bool, f64)> = self.today_habits.clone();
        if habits.is_empty() {
            return None;
        }
        let count = habits.len();
        let dark = theme.is_dark();
        let base_surface = theme.colors().elevated_surface_background;
        let habit_border: Hsla = if dark { gpui::rgb(0x363636).into() } else { theme.colors().border_variant };
        let habit_hover = base_surface.blend(theme.colors().ghost_element_hover);
        let cards: Vec<AnyElement> = habits
            .into_iter()
            .map(|(habit, checked, amount)| {
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
                    .when(!dark && !self.kanban_column_color, |el| el.shadow_xs())
                    .hover(move |s| s.bg(habit_hover))
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .gap(px(7.))
                            .child(crate::habit_view::avatar(&habit, px(20.), cx))
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
                        this.reload_today_habits();
                        cx.notify();
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.habit_menu = Some((id, ev.position));
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            })
            .collect();
        let col_width = self.kanban_column_width();
        let dark = theme.is_dark();
        let tint = self.kanban_column_color.then(|| lists_panel::parse_color(Some("#A5A5A5"))).flatten();
        let card_px = if self.kanban_column_color { px(10.) } else { px(12.) };
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


    pub(super) fn render_kanban_board(
        &mut self,
        tag_colors: &HashMap<String, Hsla>,
        today: NaiveDate,
        strings: &'static Strings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let columns = self.board_columns(strings);
        let habit_col = self.render_habit_column(strings, cx);
        if columns.is_empty() && habit_col.is_none() && self.section_owner().is_none() {
            return v_flex()
                .id("kanban-empty")
                .track_focus(&self.list_focus)
                .flex_1()
                .child(self.render_empty_state(strings, &theme, px(24.)))
                .into_any_element();
        }
        self.sync_kanban_quick_adds(&columns, strings, cx);
        let dark = theme.is_dark();
        let col_width = self.kanban_column_width();
        let colored = self.kanban_column_color;
        let card_px = if colored { px(10.) } else { px(12.) };
        let owner = self.section_owner();
        let show_list_name = self.view.shows_list_names() && self.sort_settings().1 != GroupBy::List;
        let hide_new_section = self.view_flag("kanban:hide_new_section", false);
        // Position where an inline "Add Section to Left / Right" input should appear among columns.
        let inline_new_col_pos: Option<usize> = self.new_section.as_ref().and(self.new_section_index).map(|sec_idx| {
            let mut seen_sections = 0usize;
            for (col_i, col) in columns.iter().enumerate() {
                if matches!(col.drop, Some(DropTarget::Section(Some(_)))) {
                    if seen_sections == sec_idx {
                        return col_i;
                    }
                    seen_sections += 1;
                }
            }
            columns.len()
        });
        let render_new_sec_input = |input: Entity<TextInput>, cx: &mut Context<Self>| {
            v_flex()
                .w(px(col_width))
                .flex_none()
                .mt(px(6.))
                .px(px(10.))
                .child(
                    h_flex()
                        .h(px(34.))
                        .px_2p5()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(theme.colors().accent)
                        .bg(theme.colors().elevated_surface_background)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_new_section(cx)))
                        .child(input),
                )
                .into_any_element()
        };
        let accent = theme.colors().accent;
        let render_sub_header = |sub_id: String,
                                 fold_key: String,
                                 folded: bool,
                                 pin_icon: bool,
                                 label: &'static str,
                                 sub_count: usize,
                                 sub_kind: KanbanSub,
                                 top_margin: Pixels,
                                 col_drop: Option<DropTarget>,
                                 cx: &mut Context<Self>| {
            h_flex()
                .id(SharedString::from(sub_id))
                .mt(top_margin)
                .h(px(28.))
                .px(px(4.))
                .gap(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .hover(|s| s.bg(theme.colors().ghost_element_hover))
                .drag_over::<DraggedTask>(move |style, _, _, _| {
                    style.bg(accent.opacity(0.08)).border_color(accent)
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedTask, _, cx| {
                    cx.stop_propagation();
                    this.task_drop = None;
                    this.task_drag_origin = None;
                    this.kanban_hover_col = None;
                    this.drop_on_kanban_subsection(dragged.id, col_drop.clone(), sub_kind, cx);
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if !this.kanban_folded_sub.remove(&fold_key) {
                        this.kanban_folded_sub.insert(fold_key.clone());
                    }
                    cx.notify();
                }))
                .child(chevron(folded).color(Color::Muted))
                .when(pin_icon, |el| {
                    el.child(
                        Icon::new(IconName::Pin)
                            .size(IconSize::XSmall)
                            .color(Color::Custom(Hsla::from(gpui::rgb(0xFAA80C)))),
                    )
                })
                .child(Label::new(label).size(LabelSize::XSmall).weight(FontWeight::SEMIBOLD).color(Color::Muted))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.colors().text_muted.opacity(0.75))
                        .child(sub_count.to_string()),
                )
                .into_any_element()
        };
        let mut rendered_cols: Vec<AnyElement> = Vec::with_capacity(columns.len() + 1);
        let mut drag_cols: Vec<KanbanColRows> = Vec::with_capacity(columns.len());
        for (i, col) in columns.into_iter().enumerate() {
            if inline_new_col_pos == Some(i)
                && let Some(input) = self.new_section.clone()
            {
                rendered_cols.push(render_new_sec_input(input, cx));
            }
            let open_count = col.open_count();
            let has_cards = col.total_count() > 0;
            let BoardCol { key: col_key, title, drop, pinned_cards, open_cards, completed_cards } = col;
            let drop_for_header = drop.clone();
            let drop_for_cards = drop.clone();
            let section_id = match drop {
                Some(DropTarget::Section(Some(id))) => Some(id),
                _ => None,
            };
            let display_title = if title.is_empty() { strings.group_tasks.to_owned() } else { title };
            let tint = self.kanban_column_tint(i, drop.as_ref(), &display_title, strings, tag_colors, accent);
            let persistent_add = if self.kanban_quick_add {
                self.kanban_quick_adds.get(&col_key).map(|(_, input)| input.clone())
            } else {
                None
            };
            let is_temp_add = persistent_add.is_none() && self.column_add.as_ref().is_some_and(|(at, _, _)| *at == i);
            let add_input = persistent_add.or_else(|| {
                self.column_add.as_ref().filter(|(at, _, _)| *at == i).map(|(_, _, input)| input.clone())
            });
            let mut col_items: Vec<AnyElement> = Vec::new();
            let mut col_plan_rows: Vec<Row> = Vec::new();
            let mut col_row_subs: HashMap<Id, KanbanSub> = HashMap::new();
            let push_sub_rows = |rows: &[KanbanVisibleRow],
                                     sub: KanbanSub,
                                     sub_label: &str,
                                     sub_drop: Option<DropTarget>,
                                     col_items: &mut Vec<AnyElement>,
                                     col_plan_rows: &mut Vec<Row>,
                                     col_row_subs: &mut HashMap<Id, KanbanSub>,
                                     cx: &mut Context<Self>| {
                if rows.is_empty() {
                    return;
                }
                col_plan_rows.push(Row::Day {
                    label: sub_label.to_owned(),
                    count: rows.len(),
                    drop: sub_drop,
                });
                for vrow in rows {
                    col_row_subs.insert(vrow.task.id, sub);
                    col_plan_rows.push(Row::Task {
                        task: vrow.task.clone(),
                        depth: vrow.depth,
                        has_children: vrow.has_children,
                        collapsed: vrow.collapsed,
                    });
                    col_items.push(self.render_kanban_row(
                        i,
                        vrow,
                        tag_colors,
                        tint,
                        show_list_name,
                        today,
                        strings,
                        cx,
                    ));
                }
            };
            if has_cards {
                if !pinned_cards.is_empty() {
                    let pinned_key = format!("{col_key}:pinned");
                    let pinned_folded = self.kanban_folded_sub.contains(&pinned_key);
                    col_items.push(render_sub_header(
                        format!("kanban-sub-pinned-{i}"),
                        pinned_key,
                        pinned_folded,
                        true,
                        strings.group_pinned,
                        pinned_cards.len(),
                        KanbanSub::Pinned,
                        px(0.),
                        drop_for_cards.clone(),
                        cx,
                    ));
                    if !pinned_folded {
                        let rows = self.kanban_visible_rows(&pinned_cards);
                        push_sub_rows(
                            &rows,
                            KanbanSub::Pinned,
                            strings.group_pinned,
                            drop_for_cards.clone(),
                            &mut col_items,
                            &mut col_plan_rows,
                            &mut col_row_subs,
                            cx,
                        );
                    }
                    if !open_cards.is_empty() {
                        let unpinned_key = format!("{col_key}:unpinned");
                        let unpinned_folded = self.kanban_folded_sub.contains(&unpinned_key);
                        col_items.push(render_sub_header(
                            format!("kanban-sub-unpinned-{i}"),
                            unpinned_key,
                            unpinned_folded,
                            false,
                            strings.group_unpinned,
                            open_cards.len(),
                            KanbanSub::Unpinned,
                            px(4.),
                            drop_for_cards.clone(),
                            cx,
                        ));
                        if !unpinned_folded {
                            let rows = self.kanban_visible_rows(&open_cards);
                            push_sub_rows(
                                &rows,
                                KanbanSub::Unpinned,
                                strings.group_unpinned,
                                drop_for_cards.clone(),
                                &mut col_items,
                                &mut col_plan_rows,
                                &mut col_row_subs,
                                cx,
                            );
                        }
                    }
                } else {
                    let rows = self.kanban_visible_rows(&open_cards);
                    push_sub_rows(
                        &rows,
                        KanbanSub::Unpinned,
                        strings.group_unpinned,
                        drop_for_cards.clone(),
                        &mut col_items,
                        &mut col_plan_rows,
                        &mut col_row_subs,
                        cx,
                    );
                }
                if !completed_cards.is_empty() {
                    let completed_key = format!("{col_key}:completed");
                    let completed_folded = self.kanban_folded_sub.contains(&completed_key);
                    col_items.push(render_sub_header(
                        format!("kanban-sub-completed-{i}"),
                        completed_key,
                        completed_folded,
                        false,
                        strings.group_completed,
                        completed_cards.len(),
                        KanbanSub::Completed,
                        if open_count > 0 { px(6.) } else { px(0.) },
                        drop_for_cards.clone(),
                        cx,
                    ));
                    if !completed_folded {
                        let rows = self.kanban_visible_rows(&completed_cards);
                        push_sub_rows(
                            &rows,
                            KanbanSub::Completed,
                            strings.group_completed,
                            Some(DropTarget::Complete),
                            &mut col_items,
                            &mut col_plan_rows,
                            &mut col_row_subs,
                            cx,
                        );
                    }
                }
            }
            let is_hover_target =
                cx.has_active_drag() && self.kanban_hover_col.as_deref() == Some(col_key.as_str());
            let col_bounds = self.kanban_col_bounds.clone();
            let col_key_for_bounds = col_key.clone();
            drag_cols.push(KanbanColRows {
                key: col_key,
                drop,
                rows: col_plan_rows,
                row_subs: col_row_subs,
            });
            rendered_cols.push(
                v_flex()
                    .id(SharedString::from(format!("kanban-column-{i}")))
                    .debug_selector(move || format!("kanban-column-{i}"))
                    .relative()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(if is_hover_target { accent } else { gpui::transparent_black() })
                    .map(|el| {
                        if is_hover_target {
                            el.bg(accent.opacity(0.08))
                        } else {
                            match (tint, !has_cards) {
                                (Some(c), true) => el.bg(gpui::linear_gradient(
                                    180.,
                                    gpui::linear_color_stop(c.opacity(if dark { 0.08 } else { 0.10 }), 0.),
                                    gpui::linear_color_stop(gpui::transparent_black(), 1.),
                                )),
                                (Some(c), false) => el.bg(c.opacity(if dark { 0.08 } else { 0.10 })),
                                (None, _) => el,
                            }
                        }
                    })
                    .when_some(section_id.zip(owner), |el, (target_section, list)| {
                        el.drag_over::<DraggedColumn>(move |style, _, _, _| {
                            style.bg(accent.opacity(0.08)).border_color(accent)
                        })
                        .on_drop(cx.listener(move |this, dragged: &DraggedColumn, _, cx| {
                            if dragged.id != target_section {
                                let result = this.store.reorder_list_sections(list, dragged.id, target_section);
                                this.check(result);
                                this.reload(cx);
                            }
                        }))
                    })
                    .w(px(col_width))
                    .flex_none()
                    .max_h_full()
                    .child(
                        gpui::canvas(
                            move |area, _, _| {
                                col_bounds.borrow_mut().insert(col_key_for_bounds.clone(), area);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(self.render_column_header(
                        i,
                        display_title,
                        open_count,
                        drop_for_header,
                        strings,
                        cx,
                    ))
                    .children(add_input.map(|input| {
                        let focused = input.focus_handle(cx).is_focused(window);
                        let input_focus = input.focus_handle(cx);
                        h_flex()
                            .mx(card_px)
                            .mt(px(6.))
                            .mb(px(4.))
                            .h(px(38.))
                            .px_2p5()
                            .gap_1p5()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(if focused { theme.colors().accent } else { gpui::transparent_black() })
                            .bg(if focused {
                                theme.colors().elevated_surface_background
                            } else {
                                theme.colors().ghost_element_hover
                            })
                            .when(!focused, |el| el.hover(|s| s.border_color(theme.colors().border_variant)))
                            .when(is_temp_add, |el| {
                                el.on_mouse_down_out(cx.listener(|this, _, _, cx| this.commit_column_add(cx)))
                            })
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                cx.stop_propagation();
                                window.focus(&input_focus, cx);
                            })
                            .child(Icon::new(IconName::Plus).size(IconSize::Small).color(if focused { Color::Accent } else { Color::Muted }))
                            .child(div().flex_1().min_w_0().child(input))
                    }))
                    .when(has_cards, |el| {
                        el.child(
                            v_scroll(
                                SharedString::from(format!("kanban-cards-scroll-{i}")),
                                v_flex()
                                    .id(SharedString::from(format!("kanban-cards-{i}")))
                                    .px(card_px)
                                    .pt(px(2.))
                                    .pb(px(12.))
                                    .children(col_items),
                            )
                            .mt(px(2.))
                            .min_h_0(),
                        )
                    })
                    .when(!has_cards, |el| {
                        el.h_full().when(tint.is_none(), |el| {
                            let top = if dark {
                                gpui::white().opacity(0.025)
                            } else {
                                Hsla::from(gpui::rgb(0x191919)).opacity(0.03)
                            };
                            el.child(
                                div()
                                    .flex_1()
                                    .mx(card_px)
                                    .mt(px(6.))
                                    .mb(px(24.))
                                    .rounded(px(8.))
                                    .bg(gpui::linear_gradient(
                                        180.,
                                        gpui::linear_color_stop(top, 0.),
                                        gpui::linear_color_stop(gpui::transparent_black(), 1.),
                                    )),
                            )
                        })
                    })
                    .into_any_element(),
            );
        }
        self.kanban_drag_rows = drag_cols;
        if inline_new_col_pos.is_some_and(|pos| pos >= rendered_cols.len())
            && let Some(input) = self.new_section.clone()
        {
            rendered_cols.push(render_new_sec_input(input, cx));
        }
        let row_bounds_clear = self.kanban_row_bounds.clone();
        let col_bounds_clear = self.kanban_col_bounds.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .mt(px(-8.))
            .child(
                gpui::canvas(
                    move |_, _, _| {
                        row_bounds_clear.borrow_mut().clear();
                        col_bounds_clear.borrow_mut().clear();
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
            .child(
                self.bind_task_list_actions(
                    h_flex()
                        .id("kanban")
                        .track_focus(&self.list_focus)
                        .track_scroll(&self.kanban_scroll)
                        .on_drag_move(cx.listener(Self::move_kanban_task_drag))
                        .on_drop(cx.listener(Self::drop_task_in_kanban)),
                    cx,
                )
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.list_focus, cx);
                    this.close_card(cx);
                    if this.selected.take().is_some() {
                        this.detail.update(cx, |detail, cx| detail.show(None, cx));
                        cx.notify();
                    }
                    let offset = this.kanban_scroll.offset();
                    this.kanban_pan = Some((event.position.x, offset.x));
                }))
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    let Some((start_x, start_offset_x)) = this.kanban_pan else { return };
                    if cx.has_active_drag() || event.pressed_button != Some(MouseButton::Left) {
                        this.kanban_pan = None;
                        return;
                    }
                    let delta = event.position.x - start_x;
                    let offset = this.kanban_scroll.offset();
                    this.kanban_scroll.set_offset(gpui::point((start_offset_x + delta).min(px(0.)), offset.y));
                    cx.notify();
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| {
                    this.kanban_pan = None;
                }))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| {
                    this.kanban_pan = None;
                }))
                .size_full()
                    .items_start()
                    .when(colored, |el| el.gap(px(12.)))
                    .px(px(12.))
                    .pb(px(12.))
                    .overflow_x_scroll()
                    .children(rendered_cols)
                    .children(habit_col)
                    .when(owner.is_some() && inline_new_col_pos.is_none(), |el| {
                        let group_id = SharedString::from("kanban-new-section-group");
                        el.map(|el| match self.new_section.clone() {
                            Some(input) => el.child(render_new_sec_input(input, cx)),
                            None if !hide_new_section => el.child(
                                v_flex().w(px(col_width)).flex_none().mt(px(6.)).px(px(10.)).child(
                                    h_flex()
                                        .id("kanban-new-section")
                                        .group(group_id.clone())
                                        .h(px(32.))
                                        .px(px(6.))
                                        .rounded(px(6.))
                                        .gap_1()
                                        .hover(|s| s.bg(theme.colors().ghost_element_hover))
                                        .cursor_pointer()
                                        .child(Icon::new(IconName::Plus).size(IconSize::Small).color(Color::Muted))
                                        .child(div().flex_1().child(Label::new(strings.new_section).weight(FontWeight::SEMIBOLD).color(Color::Muted)))
                                        .child(
                                            div()
                                                .id("kanban-hide-new-section")
                                                .opacity(0.)
                                                .group_hover(group_id, |s| s.opacity(1.))
                                                .child(
                                                    IconButton::new("kanban-hide-new-section-btn", IconName::X)
                                                        .size(px(20.))
                                                        .icon_size(IconSize::XSmall)
                                                        .icon_color(Color::Muted)
                                                        .tooltip(Tooltip::text(strings.close))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            let _ = this.store.set_setting(&format!("kanban:hide_new_section:{}", this.view_key()), "1");
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| this.start_new_section(window, cx))),
                                ),
                            ),
                            None => el,
                        })
                    }),
            )
            .child(HorizontalScrollbar::for_handle("kanban-h-scroll", self.kanban_scroll.clone()))
            .into_any_element()
    }
}
