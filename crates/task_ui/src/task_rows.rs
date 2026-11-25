//! Turns a task list into rows: groups, the subtask tree and due-date labels.
//! Kept free of GPUI so it can be unit-tested.

use std::collections::{HashMap, HashSet};

use fern_core::{Id, Kind, ListSection, Priority, Task};
use gpui::Hsla;
use theme::Theme;
use chrono::{Datelike, NaiveDate, NaiveTime};

use crate::i18n::Strings;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Tasks,
    Notes,
    Completed,
}

impl Section {
    pub fn title(self, strings: &Strings) -> &'static str {
        match self {
            Section::Tasks => strings.group_tasks,
            Section::Notes => strings.group_notes,
            Section::Completed => strings.group_completed,
        }
    }

    fn of(task: &Task) -> Self {
        if task.is_completed() {
            Section::Completed
        } else if task.kind == Kind::Note {
            Section::Notes
        } else {
            Section::Tasks
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Section { section: Section, count: usize, collapsed: bool },
    /// A group header with a label: a day in the Completed and Won't Do views ("Sunday,
    /// September 27"), a date, tag or priority group, or a list section. `drop` says what moving
    /// a task into the group does, if anything.
    Day { label: String, count: usize, drop: Option<DropTarget> },
    Task { task: Task, depth: usize, has_children: bool, collapsed: bool },
}

/// Rows for the list. Group headers appear only when `grouped` is set and more than one group is
/// non-empty; ungrouped lists (the trash) keep their order and only nest subtasks.
/// A subtask nests under its parent when the parent is in the same group; otherwise it is
/// shown at the top level.
pub fn build_rows(
    tasks: &[Task],
    collapsed_tasks: &HashSet<Id>,
    collapsed_sections: &HashSet<Section>,
    grouped: bool,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let push_tree_rows = |slice: &[Task], collapsed_tasks: &HashSet<Id>, rows: &mut Vec<Row>| {
        let tree = fern_core::TaskTree::build(slice);
        for (task, depth) in tree.flatten_with_depth(|id| !collapsed_tasks.contains(&id)) {
            rows.push(Row::Task {
                task: task.clone(),
                depth,
                has_children: tree.has_children(task.id),
                collapsed: collapsed_tasks.contains(&task.id),
            });
        }
    };
    if !grouped {
        push_tree_rows(tasks, collapsed_tasks, &mut rows);
        return rows;
    }

    let groups: Vec<(Section, Vec<Task>)> = [Section::Tasks, Section::Notes, Section::Completed]
        .iter()
        .map(|&section| (section, tasks.iter().filter(|t| Section::of(t) == section).cloned().collect::<Vec<_>>()))
        .filter(|(_, tasks)| !tasks.is_empty())
        .collect();
    let with_headers = groups.len() > 1;

    for (section, group_tasks) in groups {
        let collapsed = with_headers && collapsed_sections.contains(&section);
        if with_headers {
            rows.push(Row::Section { section, count: group_tasks.len(), collapsed });
        }
        if collapsed {
            continue;
        }
        push_tree_rows(&group_tasks, collapsed_tasks, &mut rows);
    }
    rows
}

/// What dropping a task onto a group (a kanban column) changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// A list section, or none.
    Section(Option<Id>),
    Kind(Kind),
    /// Moves a task back to open/to-do status (clears completed and in-progress).
    Todo,
    /// Marks a task explicitly In Progress.
    InProgress,
    Complete,
    /// A due date, or none.
    Due(Option<NaiveDate>),
    Priority(Priority),
    /// A tag in place of the first one, or no tags.
    Tag(Option<String>),
    /// A list, `None` being the Inbox.
    List(Option<fern_core::Id>),
}

/// The header sort menu's "Sort by".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    /// The manual order.
    #[default]
    Custom,
    Date,
    Created,
    Modified,
    Title,
    Tag,
    Priority,
}

/// The header sort menu's "Group by".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    /// Tasks / Notes / Completed.
    #[default]
    Custom,
    Date,
    Created,
    Tag,
    Priority,
    None,
    /// By list, the Inbox first, as the original groups its smart lists.
    List,
}

impl SortBy {
    pub const ALL: [Self; 7] =
        [Self::Custom, Self::Date, Self::Created, Self::Modified, Self::Title, Self::Tag, Self::Priority];

    pub fn key(self) -> &'static str {
        ["custom", "date", "created", "modified", "title", "tag", "priority"][self as usize]
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|s| s.key() == key).unwrap_or_default()
    }

    pub fn label(self, strings: &Strings) -> &'static str {
        strings.sort_options[self as usize]
    }
}

impl GroupBy {
    pub const ALL: [Self; 7] = [Self::Custom, Self::List, Self::Date, Self::Created, Self::Tag, Self::Priority, Self::None];

    pub fn key(self) -> &'static str {
        ["custom", "date", "created", "tag", "priority", "none", "list"][self as usize]
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|s| s.key() == key).unwrap_or_default()
    }

    pub fn label(self, strings: &Strings) -> &'static str {
        strings.group_options[self as usize]
    }
}

/// Moves pinned tasks to the top, the most recently pinned first; the rest keep their order.
pub fn pinned_first(tasks: &mut [Task]) {
    tasks.sort_by_key(|t| t.pinned_at.map_or(1, |_| 0));
    let pinned = tasks.iter().take_while(|t| t.pinned_at.is_some()).count();
    tasks[..pinned].sort_by_key(|t| std::cmp::Reverse(t.pinned_at));
}

/// Sorts tasks in place; the sort is stable, so ties keep the manual order.
pub fn sort_tasks(tasks: &mut [Task], sort: SortBy) {
    let tag = |t: &Task| t.tags.first().map(|t| t.to_lowercase());
    match sort {
        SortBy::Custom => {}
        // Dated tasks first, earliest first; all-day before timed on the same day.
        SortBy::Date => tasks.sort_by_key(|t| (t.due_date.is_none(), t.due_date, t.due_time.is_some(), t.due_time)),
        SortBy::Created => tasks.sort_by_key(|t| std::cmp::Reverse(t.created_at)),
        // Most recently edited first; a task never edited counts from when it was added.
        SortBy::Modified => tasks.sort_by_key(|t| std::cmp::Reverse(t.modified_at.unwrap_or(t.created_at))),
        SortBy::Title => tasks.sort_by_key(|t| t.title.to_lowercase()),
        SortBy::Tag => tasks.sort_by_key(|t| (tag(t).is_none(), tag(t))),
        SortBy::Priority => tasks.sort_by_key(|t| std::cmp::Reverse(t.priority as u8)),
    }
}

/// Sorts tasks with the list's Order, as the original's sort menu: Descending turns the chosen
/// sort around (Custom keeps the manual order).
pub fn order_tasks(tasks: &mut [Task], sort: SortBy, descending: bool) {
    sort_tasks(tasks, sort);
    if descending && sort != SortBy::Custom {
        tasks.reverse();
    }
}

/// Rows grouped by date, creation day, tag or priority (or not at all), with completed tasks in
/// a last group. Groups follow a fixed order: by date, overdue first (or, with `overdue_last` —
/// Settings → "Overdue Section shows at" — after the undated ones) and undated last.
pub fn grouped_rows(
    tasks: &[Task],
    group: GroupBy,
    today: NaiveDate,
    overdue_last: bool,
    strings: &Strings,
    cx: &gpui::App,
) -> Vec<Row> {
    grouped_rows_in(tasks, group, today, overdue_last, &[], strings, cx)
}

/// Pure variant of [`grouped_rows`] for headless/unit-test callers.
pub fn grouped_rows_with(
    tasks: &[Task],
    group: GroupBy,
    today: NaiveDate,
    overdue_last: bool,
    horizons_enabled: bool,
    strings: &Strings,
) -> Vec<Row> {
    grouped_rows_in_with(tasks, group, today, overdue_last, horizons_enabled, &[], strings)
}

/// `grouped_rows` knowing the lists, for Group by List: their names, in their order.
pub fn grouped_rows_in(
    tasks: &[Task],
    group: GroupBy,
    today: NaiveDate,
    overdue_last: bool,
    lists: &[(fern_core::Id, String)],
    strings: &Strings,
    cx: &gpui::App,
) -> Vec<Row> {
    grouped_rows_in_with(
        tasks,
        group,
        today,
        overdue_last,
        desktop_common::calendar::horizons_enabled(cx),
        lists,
        strings,
    )
}

/// Pure variant of [`grouped_rows_in`] for headless/unit-test callers.
pub fn grouped_rows_in_with(
    tasks: &[Task],
    group: GroupBy,
    today: NaiveDate,
    overdue_last: bool,
    horizons_enabled: bool,
    lists: &[(fern_core::Id, String)],
    strings: &Strings,
) -> Vec<Row> {
    let (open, done): (Vec<&Task>, Vec<&Task>) = tasks.iter().partition(|t| !t.is_completed());
    // Each group's order, label and what dropping a task into it does.
    let key = |task: &Task| -> (i64, String) {
        match group {
            GroupBy::Date => match task.due_date {
                Some(d) if d < today => (if overdue_last { 12 } else { 0 }, strings.group_overdue.to_owned()),
                Some(d) if d == today => (1, strings.today.to_owned()),
                Some(d) if d == today + chrono::Duration::days(1) => (2, strings.tomorrow.to_owned()),
                Some(d) if d <= today + chrono::Duration::days(7) => (3, strings.next_7_days.to_owned()),
                Some(_) => (4, strings.group_later.to_owned()),
                None => match task.horizon.filter(|_| horizons_enabled) {
                    Some(h @ fern_core::Horizon::Week { .. }) if h <= fern_core::Horizon::current_week(today) => {
                        let badge = desktop_common::calendar::format_horizon_badge(
                            fern_core::Horizon::current_week(today),
                            strings,
                        );
                        (5, format!("{} · {badge}", strings.horizon_this_week))
                    }
                    Some(h @ fern_core::Horizon::Month { .. }) if h <= fern_core::Horizon::current_month(today) => {
                        (6, strings.horizon_this_month.to_owned())
                    }
                    Some(h @ fern_core::Horizon::Quarter { .. }) if h <= fern_core::Horizon::current_quarter(today) => {
                        (7, strings.horizon_this_quarter.to_owned())
                    }
                    Some(h @ fern_core::Horizon::Year(_)) if h <= fern_core::Horizon::current_year(today) => {
                        (8, strings.horizon_this_year.to_owned())
                    }
                    Some(fern_core::Horizon::Life) => (10, strings.horizons_life.to_owned()),
                    Some(_) => (9, strings.horizon_future.to_owned()),
                    None => (11, strings.group_no_date.to_owned()),
                },
            },
            GroupBy::Created => {
                let day = local_day(task.created_at).unwrap_or(today);
                (-(day - NaiveDate::MIN).num_days(), day_header(day, today, strings))
            }
            GroupBy::Tag => match task.tags.first() {
                Some(tag) => (0, tag.clone()),
                None => (1, strings.group_no_tags.to_owned()),
            },
            GroupBy::Priority => {
                let order = 5 - task.priority as i64;
                let label = match task.priority {
                    Priority::High => strings.priority_high,
                    Priority::Medium => strings.priority_medium,
                    Priority::Low => strings.priority_low,
                    Priority::None => strings.priority_none,
                };
                (order, label.to_owned())
            }
            GroupBy::List => match task.list_id.and_then(|id| lists.iter().position(|(l, _)| *l == id)) {
                Some(i) => (i as i64 + 1, lists[i].1.clone()),
                None => (0, strings.inbox.to_owned()),
            },
            GroupBy::Custom | GroupBy::None => (0, String::new()),
        }
    };
    let task_row = |task: &Task| Row::Task { task: task.clone(), depth: 0, has_children: false, collapsed: false };
    if group == GroupBy::None {
        return open.into_iter().chain(done).map(task_row).collect();
    }
    let drop_for = |task: &Task, order: i64, label: &str| -> Option<DropTarget> {
        match group {
            GroupBy::Date => match order {
                1 => Some(DropTarget::Due(Some(today))),
                2 => Some(DropTarget::Due(Some(today + chrono::Duration::days(1)))),
                11 => Some(DropTarget::Due(None)),
                _ => None,
            },
            GroupBy::Priority => Some(DropTarget::Priority(task.priority)),
            GroupBy::Tag => Some(DropTarget::Tag(task.tags.first().map(|_| label.to_owned()))),
            GroupBy::List => Some(DropTarget::List(task.list_id)),
            _ => None,
        }
    };
    let mut groups: Vec<((i64, String), Vec<&Task>)> = Vec::new();
    for task in open {
        let k = key(task);
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, items)) => items.push(task),
            None => groups.push((k, vec![task])),
        }
    }
    // Tag groups are alphabetical, the rest by their fixed order.
    groups.sort_by(|((a, la), _), ((b, lb), _)| a.cmp(b).then_with(|| la.to_lowercase().cmp(&lb.to_lowercase())));
    let completed_at = groups.len();
    if !done.is_empty() {
        groups.push(((i64::MAX, strings.group_completed.to_owned()), done));
    }
    let mut rows = Vec::new();
    for (i, ((order, label), items)) in groups.into_iter().enumerate() {
        let drop = if i == completed_at { Some(DropTarget::Complete) } else { drop_for(items[0], order, &label) };
        rows.push(Row::Day { label, count: items.len(), drop });
        rows.extend(items.into_iter().map(task_row));
    }
    rows
}

/// Rows of a list with sections: tasks without a section first, then each section (even an
/// empty one, so it shows as a kanban column), then completed tasks. Subtasks stay nested.
pub fn section_rows(tasks: &[Task], sections: &[ListSection], collapsed_tasks: &HashSet<Id>, strings: &Strings) -> Vec<Row> {
    let (open, done): (Vec<Task>, Vec<Task>) = tasks.iter().cloned().partition(|t| !t.is_completed());
    let mut rows = Vec::new();
    let group = |label: String, drop: DropTarget, tasks: Vec<Task>, always: bool, rows: &mut Vec<Row>| {
        let drop = Some(drop);
        if tasks.is_empty() && !always {
            return;
        }
        rows.push(Row::Day { label, count: tasks.len(), drop });
        rows.extend(build_rows(&tasks, collapsed_tasks, &HashSet::new(), false));
    };
    let known: HashSet<Id> = sections.iter().map(|s| s.id).collect();
    let mut unsectioned: Vec<Task> = Vec::new();
    let mut by_section: HashMap<Id, Vec<Task>> = HashMap::with_capacity(sections.len());
    for task in open {
        match task.section_id.filter(|s| known.contains(s)) {
            Some(sec_id) => by_section.entry(sec_id).or_default().push(task),
            None => unsectioned.push(task),
        }
    }
    group(strings.not_sectioned.to_owned(), DropTarget::Section(None), unsectioned, false, &mut rows);
    for section in sections {
        let tasks = by_section.remove(&section.id).unwrap_or_default();
        group(section.name.clone(), DropTarget::Section(Some(section.id)), tasks, true, &mut rows);
    }
    group(strings.group_completed.to_owned(), DropTarget::Complete, done, false, &mut rows);
    rows
}

/// A kanban column: its title and cards, each a top-level task with its subtasks.
pub type KanbanColumn = (String, Option<DropTarget>, Vec<(Task, Vec<Task>)>);

/// Splits list rows into kanban columns, one per group header; rows before any header form an
/// untitled column. Subtasks go into their top-level task's card.
pub fn kanban_columns(rows: Vec<Row>, strings: &Strings) -> Vec<KanbanColumn> {
    let mut columns: Vec<KanbanColumn> = Vec::new();
    for row in rows {
        match row {
            Row::Section { section, .. } => {
                let drop = match section {
                    Section::Tasks => DropTarget::Kind(Kind::Task),
                    Section::Notes => DropTarget::Kind(Kind::Note),
                    Section::Completed => DropTarget::Complete,
                };
                columns.push((section.title(strings).to_owned(), Some(drop), Vec::new()))
            }
            Row::Day { label, drop, .. } => columns.push((label, drop, Vec::new())),
            Row::Task { task, depth, .. } => {
                if columns.is_empty() {
                    columns.push((String::new(), None, Vec::new()));
                }
                let cards = &mut columns.last_mut().expect("a column was just ensured").2;
                match cards.last_mut() {
                    Some((_, subtasks)) if depth > 0 => subtasks.push(task),
                    _ => cards.push((task, Vec::new())),
                }
            }
        }
    }
    columns
}

/// Byte ranges of case-insensitive occurrences of `query` in `text`, for highlighting.
/// Compares character by character, so non-ASCII text (Cyrillic, accents) works.
pub fn match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let query: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let lower: Vec<char> = chars.iter().map(|(_, c)| c.to_lowercase().next().unwrap_or(*c)).collect();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + query.len() <= lower.len() {
        if lower[i..i + query.len()] == query[..] {
            let start = chars[i].0;
            let end = chars.get(i + query.len()).map_or(text.len(), |(index, _)| *index);
            ranges.push(start..end);
            i += query.len();
        } else {
            i += 1;
        }
    }
    ranges
}

/// A short excerpt of `text` around the first match of `query`, with "…" where it was cut.
pub fn snippet(text: &str, query: &str, context: usize) -> Option<String> {
    let range = match_ranges(text, query).into_iter().next()?;
    let before: Vec<char> = text[..range.start].chars().collect();
    let after: Vec<char> = text[range.end..].chars().collect();
    let start = before.len().saturating_sub(context);
    let end = after.len().min(context);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&before[start..]);
    out.push_str(&text[range]);
    out.extend(&after[..end]);
    if end < after.len() {
        out.push('…');
    }
    Some(out.replace('\n', " "))
}

/// The "All Dates" filter of the Completed and Won't Do views. Weeks start on Monday.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClosedRange {
    #[default]
    All,
    ThisWeek,
    LastWeek,
    ThisMonth,
    /// "Other Date": both days included.
    Custom(NaiveDate, NaiveDate),
}

impl ClosedRange {
    /// The preset ranges of the menu; "Other Date" comes after them.
    pub const PRESETS: [Self; 4] = [Self::All, Self::ThisWeek, Self::LastWeek, Self::ThisMonth];

    /// A custom range in either order.
    pub fn custom(a: NaiveDate, b: NaiveDate) -> Self {
        Self::Custom(a.min(b), a.max(b))
    }

    pub fn label(self, strings: &Strings) -> String {
        match self {
            Self::All => strings.all_dates.to_owned(),
            Self::ThisWeek => strings.this_week.to_owned(),
            Self::LastWeek => strings.last_week.to_owned(),
            Self::ThisMonth => strings.this_month.to_owned(),
            Self::Custom(start, end) => format!("{} - {}", short_date(start, strings), short_date(end, strings)),
        }
    }

    pub fn contains(self, day: NaiveDate, today: NaiveDate, week_start: chrono::Weekday) -> bool {
        let monday = crate::calendar::week_from(today, week_start)[0];
        match self {
            Self::All => true,
            Self::ThisWeek => day >= monday && day < monday + chrono::Duration::weeks(1),
            Self::LastWeek => day >= monday - chrono::Duration::weeks(1) && day < monday,
            Self::ThisMonth => day.year() == today.year() && day.month() == today.month(),
            Self::Custom(start, end) => day >= start && day <= end,
        }
    }
}

/// "Oct 18" / "18 окт".
pub use crate::calendar::{day_header, local_day, short_date};

/// Rows for closed tasks grouped by the day they were closed (newest first), as in the original's
/// Completed and Won't Do views. `closed_at` picks the timestamp (completion or "Won't Do").
pub fn rows_by_day(
    tasks: &[Task],
    closed_at: impl Fn(&Task) -> Option<i64>,
    today: NaiveDate,
    strings: &Strings,
) -> Vec<Row> {
    let mut groups: Vec<(NaiveDate, Vec<&Task>)> = Vec::new();
    let mut sorted: Vec<&Task> = tasks.iter().collect();
    sorted.sort_by_key(|t| std::cmp::Reverse(closed_at(t).unwrap_or(0)));
    for task in sorted {
        let day = closed_at(task).and_then(local_day).unwrap_or(today);
        match groups.last_mut() {
            Some((last, items)) if *last == day => items.push(task),
            _ => groups.push((day, vec![task])),
        }
    }
    let mut rows = Vec::new();
    for (day, items) in groups {
        // As the original: "Sunday, Today" and "Saturday, Yesterday" for the last two days.
        let label = match (today - day).num_days() {
            0 => relative_day(day, strings.today, strings),
            1 => relative_day(day, strings.yesterday, strings),
            _ => closed_day(day, today, strings),
        };
        rows.push(Row::Day { label, count: items.len(), drop: None });
        rows.extend(items.into_iter().map(|task| Row::Task {
            task: task.clone(),
            depth: 0,
            has_children: false,
            collapsed: false,
        }));
    }
    rows
}

/// Leaves out the tasks of the day and section groups whose titles are in `collapsed`; their
/// headers stay, as the original folds a group by its arrow.
pub fn fold_days(rows: Vec<Row>, collapsed: &HashSet<String>) -> Vec<Row> {
    if collapsed.is_empty() {
        return rows;
    }
    let mut folded = false;
    rows.into_iter()
        .filter(|row| match row {
            Row::Day { label, .. } => {
                folded = collapsed.contains(label);
                true
            }
            Row::Section { .. } => {
                folded = false;
                true
            }
            Row::Task { .. } => !folded,
        })
        .collect()
}

pub use desktop_common::calendar::format_day;

/// A closed task's day group, as the original: "Sunday, September 27" / "17 окт Сб".
pub fn closed_day(day: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    format_day(strings.closed_day[usize::from(day.year() != today.year())], day, strings)
}

/// A day named after today, with its weekday: "Sunday, Today" / "Пн, Сегодня".
pub fn relative_day(day: NaiveDate, name: &str, strings: &Strings) -> String {
    format_day(strings.relative_day, day, strings).replace("{rel}", name)
}

/// "Sunday, September 27", with the year when it is not the current one.
/// A day's header in Next 7 Days, as the original's: "Sunday, Today", "Monday, Tomorrow",
/// "Wednesday, October 21", and "Next Saturday, October 24" for a day in the next week.
pub fn next_days_header(day: NaiveDate, today: NaiveDate, week_start: chrono::Weekday, strings: &Strings) -> String {
    let offset = (day - today).num_days();
    if offset == 0 || offset == 1 {
        return relative_day(day, if offset == 0 { strings.today } else { strings.tomorrow }, strings);
    }
    let week = |d: NaiveDate| (d - chrono::Duration::days(((7 + d.weekday().num_days_from_monday() - week_start.num_days_from_monday()) % 7) as i64)).num_days_from_ce();
    let label = day_header(day, today, strings);
    if week(day) != week(today) { strings.next_weekday.replace("{day}", &label) } else { label }
}

/// Next 7 Days' rows, as the original groups them: the overdue tasks, then a group for each day
/// with tasks, its header from `next_days_header`; completed tasks last.
pub fn next_days_rows(tasks: &[Task], today: NaiveDate, strings: &Strings, cx: &gpui::App) -> Vec<Row> {
    next_days_rows_with(
        tasks,
        today,
        crate::calendar::week_start(cx),
        desktop_common::calendar::horizons_enabled(cx),
        strings,
    )
}

/// Pure variant of [`next_days_rows`] for headless/unit-test callers.
pub fn next_days_rows_with(
    tasks: &[Task],
    today: NaiveDate,
    week_start: chrono::Weekday,
    horizons_enabled: bool,
    strings: &Strings,
) -> Vec<Row> {
    let (open, done): (Vec<&Task>, Vec<&Task>) = tasks.iter().partition(|t| !t.is_completed());
    let task_row = |task: &Task| Row::Task { task: task.clone(), depth: 0, has_children: false, collapsed: false };
    let mut rows = Vec::new();
    let overdue: Vec<&&Task> = open.iter().filter(|t| t.due_date.is_some_and(|d| d < today)).collect();
    if !overdue.is_empty() {
        rows.push(Row::Day { label: strings.group_overdue.to_owned(), count: overdue.len(), drop: None });
        rows.extend(overdue.into_iter().map(|t| task_row(t)));
    }
    for offset in 0..7 {
        let day = today + chrono::Duration::days(offset);
        let on_day: Vec<&&Task> = open.iter().filter(|t| t.due_date == Some(day)).collect();
        if on_day.is_empty() {
            continue;
        }
        rows.push(Row::Day { label: next_days_header(day, today, week_start, strings), count: on_day.len(), drop: Some(DropTarget::Due(Some(day))) });
        rows.extend(on_day.into_iter().map(|t| task_row(t)));
    }
    if horizons_enabled {
        let cur_week = fern_core::Horizon::current_week(today);
        let week_goals: Vec<&&Task> = open
            .iter()
            .filter(|t| {
                t.due_date.is_none()
                    && matches!(t.horizon, Some(h @ fern_core::Horizon::Week { .. }) if h <= cur_week)
            })
            .collect();
        if !week_goals.is_empty() {
            let badge = desktop_common::calendar::format_horizon_badge(cur_week, strings);
            rows.push(Row::Day {
                label: format!("{} · {badge}", strings.horizon_this_week),
                count: week_goals.len(),
                drop: None,
            });
            rows.extend(week_goals.into_iter().map(|t| task_row(t)));
        }
    }
    if !done.is_empty() {
        rows.push(Row::Section { section: Section::Completed, count: done.len(), collapsed: false });
        rows.extend(done.into_iter().map(task_row));
    }
    rows
}

/// The checkbox and flag tint for a priority; `None` for no priority.
pub fn priority_color(priority: Priority, theme: &Theme) -> Option<Hsla> {
    match priority {
        Priority::High => Some(theme.status().error),
        Priority::Medium => Some(theme.status().warning),
        Priority::Low => Some(theme.status().info),
        Priority::None => None,
    }
}

/// The due-date label for a row and whether it is overdue:
/// "Today", "Tomorrow", "Sat", "Nov 13", "Nov 13 2027" (or the localized equivalents).
pub fn due_label(
    date: NaiveDate,
    time: Option<NaiveTime>,
    today: NaiveDate,
    strings: &Strings,
    cx: &gpui::App,
) -> (String, bool) {
    due_label_with(date, time, today, strings, crate::calendar::hour12(cx))
}

/// Pure variant of [`due_label`] for headless/print callers.
pub fn due_label_with(
    date: NaiveDate,
    time: Option<NaiveTime>,
    today: NaiveDate,
    strings: &Strings,
    hour12: bool,
) -> (String, bool) {
    let days = (date - today).num_days();
    let mut label = match days {
        0 => strings.today.to_owned(),
        1 => strings.tomorrow.to_owned(),
        -1 => strings.yesterday.to_owned(),
        2..=6 => strings.weekdays_short[date.weekday().num_days_from_monday() as usize].to_owned(),
        _ => format_day(strings.short_date[usize::from(date.year() != today.year())], date, strings),
    };
    if let Some(time) = time {
        label.push_str(&format!(" {}", crate::calendar::format_time_with(time, hour12, strings)));
    }
    (label, days < 0)
}

/// The date in the calendar's task card, as the original's: the weekday and the date,
/// "Sat, Oct 24" ("Sat, Oct 24, 2027" in another year), with the time when there is one.
pub fn card_due_label(
    date: NaiveDate,
    time: Option<NaiveTime>,
    today: NaiveDate,
    strings: &Strings,
    cx: &gpui::App,
) -> String {
    card_due_label_with(date, time, today, strings, crate::calendar::hour12(cx))
}

/// Pure variant of [`card_due_label`] for headless/unit-test callers.
pub fn card_due_label_with(
    date: NaiveDate,
    time: Option<NaiveTime>,
    today: NaiveDate,
    strings: &Strings,
    hour12: bool,
) -> String {
    let weekday = strings.weekdays_short[date.weekday().num_days_from_monday() as usize];
    let mut label = format!("{weekday}, {}", closed_due_label(date, today, strings));
    if let Some(time) = time {
        label.push_str(&format!(" {}", crate::calendar::format_time_with(time, hour12, strings)));
    }
    label
}

/// The details' date as the original's: a date two days off or more says how many are left,
/// "Nov 13, 26d left", or how long it is overdue, "Oct 21, 5d overdue".
pub fn detail_due_label(label: String, date: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    let days = (date - today).num_days();
    match days {
        2.. => format!("{label}, {}", strings.days_left.replace("{n}", &days.to_string())),
        ..=-2 => format!("{label}, {}", strings.days_overdue.replace("{n}", &(-days).to_string())),
        _ => label,
    }
}

/// A closed task's date, as the original's Completed and Won't Do show it: the date itself,
/// "Oct 15" ("Oct 15, 2025" in another year), never "Today" or "Tomorrow".
pub fn closed_due_label(date: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    format_day(strings.closed_date[usize::from(date.year() != today.year())], date, strings)
}

/// A task row's date with the View Options' "Show Date by": Task Time is `due_label`; Countdown
/// says how far away the date is ("in 3 days", "2 days ago"), today, tomorrow and yesterday
/// keeping their names.
pub fn row_due_label(
    date: NaiveDate,
    time: Option<NaiveTime>,
    today: NaiveDate,
    strings: &Strings,
    countdown: bool,
    cx: &gpui::App,
) -> (String, bool) {
    row_due_label_with(date, time, today, strings, countdown, crate::calendar::hour12(cx))
}

/// Pure variant of [`row_due_label`] for headless/unit-test callers.
pub fn row_due_label_with(
    date: NaiveDate,
    time: Option<NaiveTime>,
    today: NaiveDate,
    strings: &Strings,
    countdown: bool,
    hour12: bool,
) -> (String, bool) {
    let days = (date - today).num_days();
    if !countdown || (-1..=1).contains(&days) {
        return due_label_with(date, time, today, strings, hour12);
    }
    let text = if days > 0 { strings.in_days.replace("{n}", &days.to_string()) } else { strings.days_ago.replace("{n}", &(-days).to_string()) };
    (text, days < 0)
}

/// Returns the subtle horizon badge label when a task has no exact due date and belongs
/// to a time horizon bucket, so horizon tasks stay clearly labeled across classic task views.
pub fn horizon_badge_label(task: &Task, strings: &Strings) -> Option<String> {
    if task.due_date.is_some() {
        return None;
    }
    task.horizon.map(|horizon| desktop_common::calendar::format_horizon_badge(horizon, strings))
}

/// Returns `(label, is_past)` when a task has no exact due date and belongs to a time horizon
/// bucket, mirroring [`due_label`] so views can gently highlight past-period carryover tasks.
pub fn horizon_badge(task: &Task, today: NaiveDate, strings: &Strings) -> Option<(String, bool)> {
    if task.due_date.is_some() {
        return None;
    }
    task.horizon.map(|horizon| {
        (
            desktop_common::calendar::format_horizon_badge(horizon, strings),
            horizon.is_past_relative_to(today),
        )
    })
}

/// Returns `(label, is_past)` for a task row rendered under `group`.
/// In [`GroupBy::Date`], past-period horizons (`h.is_past_relative_to(today)`) are always kept
/// visible even though they roll into the current period bucket (e.g. an October task grouped
/// under "This month" still displays `Oct`), while redundant current-period badges can be omitted.
pub fn horizon_badge_for_group(
    task: &Task,
    group: GroupBy,
    today: NaiveDate,
    strings: &Strings,
    cx: &gpui::App,
) -> Option<(String, bool)> {
    horizon_badge_for_group_with(
        task,
        group,
        today,
        desktop_common::calendar::horizons_enabled(cx),
        strings,
    )
}

/// Pure variant of [`horizon_badge_for_group`] for headless/unit-test callers.
pub fn horizon_badge_for_group_with(
    task: &Task,
    group: GroupBy,
    today: NaiveDate,
    horizons_enabled: bool,
    strings: &Strings,
) -> Option<(String, bool)> {
    if task.due_date.is_some() {
        return None;
    }
    let horizon = task.horizon?;
    let is_past = horizon.is_past_relative_to(today);
    if group == GroupBy::Date && horizons_enabled && !is_past {
        let redundant_with_bucket = match horizon {
            fern_core::Horizon::Week { .. } => horizon == fern_core::Horizon::current_week(today),
            fern_core::Horizon::Month { .. } => horizon == fern_core::Horizon::current_month(today),
            fern_core::Horizon::Quarter { .. } => horizon == fern_core::Horizon::current_quarter(today),
            fern_core::Horizon::Year(_) => horizon == fern_core::Horizon::current_year(today),
            fern_core::Horizon::Life => true,
            fern_core::Horizon::Decade(_) => false,
        };
        if redundant_with_bucket {
            return None;
        }
    }
    Some((
        desktop_common::calendar::format_horizon_badge(horizon, strings),
        is_past,
    ))
}

/// Foreground text/icon color, fill background, and subtle border color for a horizon badge.
/// Open tasks with a past horizon receive a warm warning tint calmer than hard due-date overdue red.
pub fn horizon_badge_style(past: bool, closed: bool, theme: &Theme) -> (Hsla, Hsla, Hsla) {
    if past && !closed {
        let warning = theme.status().warning;
        (warning, warning.opacity(0.12), warning.opacity(0.32))
    } else {
        let colors = theme.colors();
        (colors.text_muted, colors.element_hover.opacity(0.65), colors.border_variant)
    }
}

/// Planned result of dropping a dragged task in the list view:
/// vertical half chooses before/after and horizontal movement chooses the nesting depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDropPlan {
    /// Task row on which to render the horizontal drop indicator line.
    pub target_id: Id,
    /// `false` draws the indicator at the top edge of `target_id`; `true` at the bottom edge.
    pub after: bool,
    /// Indentation level (`0..=MAX_SUBTASK_DEPTH`) of the drop indicator and resulting task.
    pub target_level: usize,
    /// New `parent_id` for the dragged task (`None` = top-level root).
    pub new_parent: Option<Id>,
    /// Where to move `dragged_id` in manual `sort_order`: `Some((anchor_id, after))`.
    pub order_target: Option<(Id, bool)>,
    /// Collapsed parent that should expand after dropping a child inside it.
    pub expand_parent: Option<Id>,
    /// Group header drop semantics when dropping into a section, Completed, or Notes group.
    pub group_drop: Option<DropTarget>,
}

fn row_task(row: &Row) -> Option<(&Task, usize, bool)> {
    match row {
        Row::Task { task, depth, collapsed, .. } => Some((task, *depth, *collapsed)),
        Row::Section { .. } | Row::Day { .. } => None,
    }
}

fn group_bounds(rows: &[Row], idx: usize) -> (usize, usize, Option<DropTarget>) {
    let mut start = idx;
    while start > 0 && matches!(rows[start - 1], Row::Task { .. }) {
        start -= 1;
    }
    let group_drop = start.checked_sub(1).and_then(|h| match &rows[h] {
        Row::Day { drop, .. } => drop.clone(),
        Row::Section { section, .. } => Some(match section {
            Section::Tasks => DropTarget::Kind(Kind::Task),
            Section::Notes => DropTarget::Kind(Kind::Note),
            Section::Completed => DropTarget::Complete,
        }),
        Row::Task { .. } => None,
    });
    let mut end = idx + 1;
    while end < rows.len() && matches!(rows[end], Row::Task { .. }) {
        end += 1;
    }
    (start, end, group_drop)
}

fn visible_subtree_end(rows: &[Row], idx: usize, depth: usize, group_end: usize) -> usize {
    let mut k = idx + 1;
    while k < group_end {
        match row_task(&rows[k]) {
            Some((_, d, _)) if d > depth => k += 1,
            _ => break,
        }
    }
    k
}

/// Plans a list-view task drag over `hovered_id`.
///
/// - `after`: whether the pointer is in the bottom half of `hovered_id`.
/// - `dx`: horizontal pointer displacement since drag start (`pointer_x - start_x`).
/// - `rel_x`: horizontal pointer position relative to the row's left edge.
/// - `indent`: horizontal pixels per nesting level (24 px).
/// - `gutter`: left gutter before the root checkbox (20 px).
/// - `allow_nesting`: whether the active grouping displays nested trees.
#[allow(clippy::too_many_arguments)]
pub fn plan_task_drop(
    rows: &[Row],
    dragged_id: Id,
    hovered_id: Id,
    after: bool,
    dx: f32,
    rel_x: f32,
    indent: f32,
    gutter: f32,
    allow_nesting: bool,
) -> Option<TaskDropPlan> {
    let hovered_idx = rows.iter().position(|r| row_task(r).is_some_and(|(t, _, _)| t.id == hovered_id))?;
    let dragged_idx = rows.iter().position(|r| row_task(r).is_some_and(|(t, _, _)| t.id == dragged_id))?;
    let (dragged_task, dragged_depth, _) = row_task(&rows[dragged_idx])?;
    let (group_start, group_end, group_drop) = group_bounds(rows, hovered_idx);
    let (_, dragged_group_end, _) = group_bounds(rows, dragged_idx);
    let dragged_subtree_end = visible_subtree_end(rows, dragged_idx, dragged_depth, dragged_group_end);

    // Reject dropping a task onto one of its own visible descendants.
    if hovered_idx > dragged_idx && hovered_idx < dragged_subtree_end {
        return None;
    }

    let same_group = dragged_idx >= group_start && dragged_idx < group_end;
    let in_dragged = |i: usize| same_group && i >= dragged_idx && i < dragged_subtree_end;

    let (prev_idx, next_idx) = if hovered_idx == dragged_idx {
        let prev = (dragged_idx > group_start).then(|| dragged_idx - 1);
        let next = (dragged_subtree_end < group_end).then_some(dragged_subtree_end);
        (prev, next)
    } else if !after {
        let prev = (group_start..hovered_idx).rev().find(|&i| !in_dragged(i));
        (prev, Some(hovered_idx))
    } else {
        let next = (hovered_idx + 1..group_end).find(|&i| !in_dragged(i));
        (Some(hovered_idx), next)
    };

    let dragged_is_note = dragged_task.kind == Kind::Note;
    let target_is_notes = matches!(group_drop, Some(DropTarget::Kind(Kind::Note)));
    let target_is_completed = matches!(group_drop, Some(DropTarget::Complete));
    // Notes and completed sections cannot be nested or hold subtasks, nor be dropped inside an expanded subtask block.
    let can_nest = allow_nesting && !dragged_is_note && !target_is_notes && !target_is_completed;

    let min_level = if allow_nesting {
        next_idx.and_then(|i| row_task(&rows[i]).map(|(_, d, _)| d)).unwrap_or(0)
    } else {
        0
    };
    let max_level = if !can_nest {
        0
    } else if let Some(p) = prev_idx {
        let (prev_task, prev_depth, _) = row_task(&rows[p])?;
        if prev_task.kind == Kind::Note {
            0
        } else {
            (prev_depth + 1).min(fern_core::MAX_SUBTASK_DEPTH)
        }
    } else {
        0
    };
    if min_level > max_level {
        return None;
    }

    let target_level = if min_level == max_level {
        min_level
    } else {
        let base_level = if hovered_idx == dragged_idx {
            dragged_depth
        } else {
            prev_idx.and_then(|p| row_task(&rows[p]).map(|(_, d, _)| d)).unwrap_or(0)
        }
        .clamp(min_level, max_level);

        const STEP_THRESHOLD: f32 = 18.0;
        let step_indent = indent.max(1.0);
        let delta: isize = if dx >= STEP_THRESHOLD {
            ((dx - STEP_THRESHOLD) / step_indent).floor() as isize + 1
        } else if dx <= -STEP_THRESHOLD {
            -(((dx.abs() - STEP_THRESHOLD) / step_indent).floor() as isize + 1)
        } else {
            0
        };
        let mut desired = (base_level as isize + delta).clamp(min_level as isize, max_level as isize) as usize;
        // Ergonomic aid when dragging from near the left margin: moving into the root gutter
        // area snaps cleanly down toward `min_level` even on shorter leftward drags.
        if dx < -4.0 && rel_x < gutter + (desired as f32) * step_indent - 4.0 {
            let abs_level = ((rel_x - gutter).max(0.0) / step_indent).floor() as usize;
            desired = desired.min(abs_level.clamp(min_level, max_level));
        }
        desired
    };

    let new_parent = if target_level == 0 {
        None
    } else {
        let p = prev_idx?;
        let parent_idx = (group_start..=p)
            .rev()
            .filter(|&i| !in_dragged(i))
            .find(|&i| row_task(&rows[i]).is_some_and(|(_, d, _)| d == target_level - 1))?;
        let (parent_task, _, _) = row_task(&rows[parent_idx])?;
        if parent_task.kind == Kind::Note {
            return None;
        }
        Some(parent_task.id)
    };

    // Suppress no-op drops that leave the task in its exact current slot and parent.
    let current_prev = (same_group && dragged_idx > group_start).then(|| dragged_idx - 1);
    let current_next = (same_group && dragged_subtree_end < group_end).then_some(dragged_subtree_end);
    if same_group && new_parent == dragged_task.parent_id && prev_idx == current_prev && next_idx == current_next {
        return None;
    }

    let expand_parent = prev_idx.and_then(|p| {
        let (prev_task, _, collapsed) = row_task(&rows[p])?;
        (new_parent == Some(prev_task.id) && collapsed).then_some(prev_task.id)
    });

    // Order target relative to siblings under `new_parent`:
    // 1) If `next` is a sibling at `target_level`, place immediately before `next`.
    // 2) Else if nesting directly under `prev`, place immediately after `prev`.
    // 3) Else place immediately after the nearest preceding sibling at `target_level`.
    let order_target = if let Some(n) = next_idx
        && let Some((next_task, next_depth, _)) = row_task(&rows[n])
        && next_depth == target_level
    {
        Some((next_task.id, false))
    } else if let Some(p) = prev_idx {
        let (prev_task, prev_depth, _) = row_task(&rows[p])?;
        if prev_depth < target_level {
            Some((prev_task.id, true))
        } else {
            let anchor_idx = (group_start..=p)
                .rev()
                .filter(|&i| !in_dragged(i))
                .find(|&i| row_task(&rows[i]).is_some_and(|(_, d, _)| d == target_level))?;
            Some((row_task(&rows[anchor_idx])?.0.id, true))
        }
    } else {
        None
    };

    let (target_id, indicator_after) = if hovered_idx == dragged_idx {
        if target_level < dragged_depth {
            let last_in_sub = row_task(&rows[dragged_subtree_end - 1])?.0.id;
            (last_in_sub, true)
        } else {
            (dragged_id, false)
        }
    } else {
        (hovered_id, after)
    };

    Some(TaskDropPlan {
        target_id,
        after: indicator_after,
        target_level,
        new_parent,
        order_target,
        expand_parent,
        group_drop,
    })
}

/// Plans dropping a dragged task into empty space below the last task in the list:
/// places it as a top-level root task at the end of the last compatible visible group
/// (avoiding accidentally completing an open task or converting a task into a note).
pub fn plan_task_drop_at_end(rows: &[Row], dragged_id: Id) -> Option<TaskDropPlan> {
    let dragged_idx = rows.iter().position(|r| row_task(r).is_some_and(|(t, _, _)| t.id == dragged_id))?;
    let (dragged_task, dragged_depth, _) = row_task(&rows[dragged_idx])?;
    let (_, dragged_group_end, dragged_group_drop) = group_bounds(rows, dragged_idx);
    let last_idx = rows.iter().enumerate().rev().find_map(|(i, r)| {
        if !matches!(r, Row::Task { .. }) {
            return None;
        }
        let (_, _, candidate_drop) = group_bounds(rows, i);
        // Never silently complete an open task or convert a task to a note on an empty-space drop.
        if !dragged_task.is_completed() && candidate_drop == Some(DropTarget::Complete) {
            return None;
        }
        if dragged_task.kind != Kind::Note
            && candidate_drop == Some(DropTarget::Kind(Kind::Note))
            && dragged_group_drop != Some(DropTarget::Kind(Kind::Note))
        {
            return None;
        }
        Some(i)
    })?;
    let (group_start, group_end, group_drop) = group_bounds(rows, last_idx);
    let dragged_subtree_end = visible_subtree_end(rows, dragged_idx, dragged_depth, dragged_group_end);

    let same_group = dragged_idx >= group_start && dragged_idx < group_end;
    if same_group && dragged_task.parent_id.is_none() && dragged_subtree_end == group_end {
        return None;
    }

    let in_dragged = |i: usize| same_group && i >= dragged_idx && i < dragged_subtree_end;
    let last_root = (group_start..group_end)
        .rev()
        .filter(|&i| !in_dragged(i))
        .find(|&i| row_task(&rows[i]).is_some_and(|(_, d, _)| d == 0))
        .and_then(|i| row_task(&rows[i]).map(|(t, _, _)| t.id));

    let (last_task, _, _) = row_task(&rows[last_idx])?;
    Some(TaskDropPlan {
        target_id: last_task.id,
        after: true,
        target_level: 0,
        new_parent: None,
        order_target: last_root.map(|id| (id, true)),
        expand_parent: None,
        group_drop,
    })
}

#[cfg(test)]
mod tests;
