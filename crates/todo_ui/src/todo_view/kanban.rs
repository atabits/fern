use super::*;
use super::kanban_cards::KanbanVisibleRow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KanbanSub {
    Pinned,
    Unpinned,
    Completed,
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


#[derive(Clone, PartialEq, Eq)]
pub(super) struct KanbanCacheKey {
    version: u64,
    sort: SortBy,
    group: GroupBy,
    show_completed: bool,
    descending: bool,
    kanban_workflow: bool,
    completing_len: usize,
    overdue_last: bool,
    week_start: chrono::Weekday,
    today: NaiveDate,
    strings_ptr: usize,
}

impl TodoView {
    pub(super) fn ensure_kanban_sections(&mut self, strings: &Strings, cx: &mut Context<Self>) {
        if !self.kanban
            || self.timeline
            || matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_))
        {
            return;
        }
        let Some(owner) = self.section_owner() else { return };
        let mut mutated = false;
        if self.sections.is_empty()
            && let Some(section) = self.check(self.store.add_list_section(owner, strings.not_sectioned))
        {
            self.sections.push(section);
            mutated = true;
        }
        let Some(first_id) = self.sections.first().map(|s| s.id) else {
            if mutated {
                crate::data_version::data_changed(cx.entity_id(), cx);
            }
            return;
        };
        if self.sort_settings().1 != GroupBy::Custom {
            if mutated {
                crate::data_version::data_changed(cx.entity_id(), cx);
            }
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
            if mutated {
                crate::data_version::data_changed(cx.entity_id(), cx);
            }
            return;
        }
        for root_id in &unsectioned_roots {
            let result = self.store.set_section(*root_id, Some(first_id));
            if self.check(result).is_some() {
                mutated = true;
            }
        }
        let tree = fern_core::TaskTree::build(&self.tasks);
        let mut moved: HashSet<Id> = HashSet::new();
        for root_id in unsectioned_roots {
            moved.insert(root_id);
            for desc in tree.descendants(root_id) {
                moved.insert(desc.id);
            }
        }
        drop(tree);
        for task in &mut self.tasks {
            if moved.contains(&task.id) {
                task.section_id = Some(first_id);
            }
        }
        if mutated {
            crate::data_version::data_changed(cx.entity_id(), cx);
        }
    }

    pub(super) fn board_columns(&self, strings: &Strings) -> Rc<Vec<BoardCol>> {
        let (sort, group) = self.sort_settings();
        let show_completed = self.view_flag("show_completed", true);
        let descending = self.view_flag("descending", false);
        let today = Self::today();
        let key = KanbanCacheKey {
            version: self.rows_version,
            sort,
            group,
            show_completed,
            descending,
            kanban_workflow: self.kanban_workflow,
            completing_len: self.completing.len(),
            overdue_last: self.preferences.task_defaults.overdue_last,
            week_start: crate::calendar::week_start(),
            today,
            strings_ptr: strings as *const Strings as usize,
        };
        if let Some((cached_key, cols)) = self.kanban_cache.borrow().as_ref()
            && *cached_key == key
        {
            return Rc::clone(cols);
        }

        let mut tasks = self.tasks_for_rows();
        if !show_completed {
            tasks.retain(|t| !t.is_completed());
        }
        task_rows::order_tasks(&mut tasks, sort, descending);

        let tree = fern_core::TaskTree::build(&tasks);
        let all_cards: Vec<(Task, Vec<Task>)> = tree
            .roots()
            .map(|root| {
                let subs = tree.descendants(root.id).into_iter().cloned().collect();
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

        let built = if self.kanban_workflow {
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
            vec![
                make_col("workflow:todo".into(), strings.col_todo.to_owned(), Some(DropTarget::Todo), todo_cards),
                make_col("workflow:in_progress".into(), strings.in_progress.to_owned(), Some(DropTarget::InProgress), in_progress_cards),
                make_col("workflow:completed".into(), strings.completed.to_owned(), Some(DropTarget::Complete), completed_cards),
            ]
        } else {
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
                        vec![make_col(
                            "sec:none".into(),
                            strings.not_sectioned.to_owned(),
                            Some(DropTarget::Section(None)),
                            all_cards,
                        )]
                    } else {
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
        };
        let built = Rc::new(built);
        *self.kanban_cache.borrow_mut() = Some((key, Rc::clone(&built)));
        built
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
            self.clear_kanban_quick_adds();
        }
        self.sync_kanban_column_state(cx);
        cx.notify();
    }

    pub(super) fn toggle_kanban_workflow(&mut self, cx: &mut Context<Self>) {
        let next = !self.kanban_workflow;
        let key = format!("kanban_workflow:{}", self.view_key());
        let result = self.store.set_setting(&key, if next { "1" } else { "0" });
        self.check(result);
        self.kanban_workflow = next;
        self.sync_kanban_column_state(cx);
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
    pub(super) const KANBAN_COLUMN_COLORS: [&'static str; 8] = [
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
            Some(DropTarget::List(Some(id))) => self.list_colors.get(id).copied().or(Some(accent)),
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

    pub(super) fn clear_kanban_quick_adds(&mut self) {
        self.kanban_quick_adds.clear();
        self.kanban_quick_add_subs.clear();
    }

    fn cancel_kanban_quick_add(
        &mut self,
        input: &Entity<TextInput>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dismiss_transient(window, cx) {
            return;
        }
        if !input.read(cx).text().is_empty() {
            input.update(cx, |input, cx| input.clear(window, cx));
            cx.notify();
            return;
        }
        self.column_add = None;
        window.focus(&self.list_focus, cx);
        cx.notify();
    }

    /// Keeps per-column scroll handles and quick-add `TextInput` entities synchronized with the
    /// active board columns outside of `render()` (called from `reload_inner`, layout/sort/option
    /// toggles, and section mutations).
    pub(super) fn sync_kanban_column_state(&mut self, cx: &mut Context<Self>) {
        if !self.kanban
            || self.timeline
            || matches!(self.view, View::Trash | View::Completed | View::WontDo | View::Search(_))
        {
            self.clear_kanban_quick_adds();
            self.kanban_col_scrolls.clear();
            return;
        }
        let strings = i18n::t(cx);
        let columns = self.board_columns(strings);
        let mut active_col_keys = HashSet::with_capacity(columns.len());
        for col in columns.iter() {
            active_col_keys.insert(col.key.clone());
            self.kanban_col_scrolls.entry(col.key.clone()).or_default();
        }
        self.kanban_col_scrolls.retain(|k, _| active_col_keys.contains(k));
        self.sync_kanban_quick_adds(&columns, strings, cx);
    }

    pub(super) fn sync_kanban_quick_adds(&mut self, columns: &[BoardCol], strings: &'static Strings, cx: &mut Context<Self>) {
        if !self.kanban_quick_add {
            if !self.kanban_quick_adds.is_empty() {
                self.clear_kanban_quick_adds();
            }
            return;
        }
        let mut active_keys = HashSet::new();
        let window_handle = self.window_handle;
        let view = cx.weak_entity();
        for col in columns {
            let Some(target) = col.drop.as_ref().filter(|t| !matches!(t, DropTarget::Complete)) else { continue };
            let key = col.key.clone();
            active_keys.insert(key.clone());
            if let Some(entry) = self.kanban_quick_adds.get_mut(&key) {
                entry.0 = target.clone();
                continue;
            }
            let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
            let key_for_sub = key.clone();
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
            let view = view.clone();
            let cancel_sub = gpui::App::subscribe(cx, &input, move |input, _: &Cancelled, cx| {
                let _ = window_handle.update(cx, |_, window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.cancel_kanban_quick_add(&input, window, cx);
                    });
                });
            });
            self.kanban_quick_adds.insert(key.clone(), (target.clone(), input));
            self.kanban_quick_add_subs.insert(key, cancel_sub);
        }
        self.kanban_quick_adds.retain(|k, _| active_keys.contains(k));
        self.kanban_quick_add_subs.retain(|k, _| active_keys.contains(k));
    }

    /// A kanban column's "+": a field at the column's top; the task it adds takes the column's
    /// value, as one dropped there.
    pub(super) fn start_column_add(&mut self, column: usize, target: DropTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_current_view_archived_list() {
            return;
        }
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
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.column_add = None;
            window.focus(&this.list_focus, cx);
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
        let dark = theme.is_dark();
        let col_width = self.kanban_column_width();
        let colored = self.kanban_column_color;
        let card_px = if colored { px(10.) } else { px(12.) };
        let owner = self.section_owner();
        let show_list_name = self.view.shows_list_names() && self.sort_settings().1 != GroupBy::List;
        let hide_new_section = self.hide_new_section;
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
            let input_focus = input.focus_handle(cx);
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
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            window.focus(&input_focus, cx);
                        })
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
                            .color(Color::Warning),
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
        for (i, col) in columns.iter().enumerate() {
            if inline_new_col_pos == Some(i)
                && let Some(input) = self.new_section.clone()
            {
                rendered_cols.push(render_new_sec_input(input, cx));
            }
            let open_count = col.open_count();
            let has_cards = col.total_count() > 0;
            let col_key = &col.key;
            let drop_for_header = col.drop.clone();
            let drop_for_cards = col.drop.clone();
            let section_id = match col.drop {
                Some(DropTarget::Section(Some(id))) => Some(id),
                _ => None,
            };
            let display_title = if col.title.is_empty() { strings.group_tasks.to_owned() } else { col.title.clone() };
            let tint = self.kanban_column_tint(i, col.drop.as_ref(), &display_title, strings, tag_colors, accent);
            let pinned_cards = &col.pinned_cards;
            let open_cards = &col.open_cards;
            let completed_cards = &col.completed_cards;
            let persistent_add = if self.kanban_quick_add {
                self.kanban_quick_adds.get(col_key).map(|(_, input)| input.clone())
            } else {
                None
            };
            let is_temp_add = persistent_add.is_none() && self.column_add.as_ref().is_some_and(|(at, _, _)| *at == i);
            let add_input = if self.is_current_view_archived_list() {
                None
            } else {
                persistent_add.or_else(|| {
                    self.column_add.as_ref().filter(|(at, _, _)| *at == i).map(|(_, _, input)| input.clone())
                })
            };
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
                        let rows = self.kanban_visible_rows(pinned_cards);
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
                            let rows = self.kanban_visible_rows(open_cards);
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
                    let rows = self.kanban_visible_rows(open_cards);
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
                        let rows = self.kanban_visible_rows(completed_cards);
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
            let col_key_for_ids = col_key.clone();
            let col_scroll_handle = self.kanban_col_scrolls.get(col_key).cloned();
            drag_cols.push(KanbanColRows {
                key: col_key.clone(),
                drop: drop_for_cards,
                rows: col_plan_rows,
                row_subs: col_row_subs,
            });
            rendered_cols.push(
                v_flex()
                    .id(SharedString::from(format!("kanban-column-{col_key_for_ids}")))
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
                    .when_some(section_id.zip(owner).filter(|_| !self.is_current_view_archived_list()), |el, (target_section, list)| {
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
                        let scroll = v_scroll(
                            SharedString::from(format!("kanban-cards-scroll-{col_key_for_ids}")),
                            v_flex()
                                .id(SharedString::from(format!("kanban-cards-{col_key_for_ids}")))
                                .px(card_px)
                                .pt(px(2.))
                                .pb(px(12.))
                                .children(col_items),
                        )
                        .mt(px(2.))
                        .min_h_0();
                        el.child(match col_scroll_handle {
                            Some(handle) => scroll.track_scroll(&handle),
                            None => scroll,
                        })
                    })
                    .when(!has_cards, |el| {
                        el.h_full().when(tint.is_none(), |el| {
                            let top = theme.colors().text.opacity(0.03);
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
                    let max_x = this.kanban_scroll.max_offset().x.max(px(0.));
                    this.kanban_scroll
                        .set_offset(gpui::point((start_offset_x + delta).clamp(-max_x, px(0.)), offset.y));
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
                    .when(owner.is_some() && inline_new_col_pos.is_none() && !self.is_current_view_archived_list(), |el| {
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
                                                            this.hide_new_section = true;
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
