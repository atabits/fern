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
    let groups: Vec<(Section, Vec<&Task>)> = if grouped {
        [Section::Tasks, Section::Notes, Section::Completed]
            .iter()
            .map(|&section| (section, tasks.iter().filter(|t| Section::of(t) == section).collect::<Vec<_>>()))
            .filter(|(_, tasks)| !tasks.is_empty())
            .collect()
    } else {
        vec![(Section::Tasks, tasks.iter().collect())]
    };
    let with_headers = groups.len() > 1;

    let mut rows = Vec::new();
    for (section, tasks) in groups {
        let collapsed = with_headers && collapsed_sections.contains(&section);
        if with_headers {
            rows.push(Row::Section { section, count: tasks.len(), collapsed });
        }
        if collapsed {
            continue;
        }

        let ids: HashSet<Id> = tasks.iter().map(|t| t.id).collect();
        let mut children: HashMap<Id, Vec<&Task>> = HashMap::new();
        let mut roots = Vec::new();
        for task in &tasks {
            match task.parent_id.filter(|p| ids.contains(p)) {
                Some(parent) => children.entry(parent).or_default().push(task),
                None => roots.push(*task),
            }
        }

        fn push(
            task: &Task,
            depth: usize,
            children: &HashMap<Id, Vec<&Task>>,
            collapsed_tasks: &HashSet<Id>,
            rows: &mut Vec<Row>,
        ) {
            let kids = children.get(&task.id);
            let collapsed = collapsed_tasks.contains(&task.id);
            rows.push(Row::Task {
                task: task.clone(),
                depth,
                has_children: kids.is_some(),
                collapsed,
            });
            if !collapsed {
                for kid in kids.into_iter().flatten() {
                    push(kid, depth + 1, children, collapsed_tasks, rows);
                }
            }
        }
        for root in roots {
            push(root, 0, &children, collapsed_tasks, &mut rows);
        }
    }
    rows
}

/// What dropping a task onto a group (a kanban column) changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// A list section, or none.
    Section(Option<Id>),
    Kind(Kind),
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
pub fn grouped_rows(tasks: &[Task], group: GroupBy, today: NaiveDate, overdue_last: bool, strings: &Strings) -> Vec<Row> {
    grouped_rows_in(tasks, group, today, overdue_last, &[], strings)
}

/// `grouped_rows` knowing the lists, for Group by List: their names, in their order.
pub fn grouped_rows_in(
    tasks: &[Task],
    group: GroupBy,
    today: NaiveDate,
    overdue_last: bool,
    lists: &[(fern_core::Id, String)],
    strings: &Strings,
) -> Vec<Row> {
    let (open, done): (Vec<&Task>, Vec<&Task>) = tasks.iter().partition(|t| !t.is_completed());
    // Each group's order, label and what dropping a task into it does.
    let key = |task: &Task| -> (i64, String) {
        match group {
            GroupBy::Date => match task.due_date {
                Some(d) if d < today => (if overdue_last { 6 } else { 0 }, strings.group_overdue.to_owned()),
                Some(d) if d == today => (1, strings.today.to_owned()),
                Some(d) if d == today + chrono::Duration::days(1) => (2, strings.tomorrow.to_owned()),
                Some(d) if d <= today + chrono::Duration::days(7) => (3, strings.next_7_days.to_owned()),
                Some(_) => (4, strings.group_later.to_owned()),
                None => (5, strings.group_no_date.to_owned()),
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
                5 => Some(DropTarget::Due(None)),
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
    let unsectioned: Vec<Task> =
        open.iter().filter(|t| t.section_id.is_none_or(|s| !known.contains(&s))).cloned().collect();
    group(strings.not_sectioned.to_owned(), DropTarget::Section(None), unsectioned, false, &mut rows);
    for section in sections {
        let tasks: Vec<Task> = open.iter().filter(|t| t.section_id == Some(section.id)).cloned().collect();
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

    pub fn contains(self, day: NaiveDate, today: NaiveDate) -> bool {
        let monday = crate::calendar::week_of(today)[0];
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
pub fn short_date(date: NaiveDate, strings: &Strings) -> String {
    format_day(strings.short_date[0], date, strings)
}

/// The local day of a timestamp in milliseconds.
pub fn local_day(ms: i64) -> Option<NaiveDate> {
    use chrono::{Local, TimeZone};
    Local.timestamp_millis_opt(ms).single().map(|time| time.date_naive())
}

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
pub fn day_header(day: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    format_day(strings.day_header[usize::from(day.year() != today.year())], day, strings)
}

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
pub fn next_days_rows(tasks: &[Task], today: NaiveDate, week_start: chrono::Weekday, strings: &Strings) -> Vec<Row> {
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
        label.push_str(&format!(" {}", crate::calendar::format_time(time, strings)));
    }
    (label, days < 0)
}

/// The date in the calendar's task card, as the original's: the weekday and the date,
/// "Sat, Oct 24" ("Sat, Oct 24, 2027" in another year), with the time when there is one.
pub fn card_due_label(date: NaiveDate, time: Option<NaiveTime>, today: NaiveDate, strings: &Strings) -> String {
    let weekday = strings.weekdays_short[date.weekday().num_days_from_monday() as usize];
    let mut label = format!("{weekday}, {}", closed_due_label(date, today, strings));
    if let Some(time) = time {
        label.push_str(&format!(" {}", crate::calendar::format_time(time, strings)));
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
pub fn row_due_label(date: NaiveDate, time: Option<NaiveTime>, today: NaiveDate, strings: &Strings, countdown: bool) -> (String, bool) {
    let days = (date - today).num_days();
    if !countdown || (-1..=1).contains(&days) {
        return due_label(date, time, today, strings);
    }
    let text = if days > 0 { strings.in_days.replace("{n}", &days.to_string()) } else { strings.days_ago.replace("{n}", &(-days).to_string()) };
    (text, days < 0)
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
/// places it as a top-level root task at the end of the last visible group.
pub fn plan_task_drop_at_end(rows: &[Row], dragged_id: Id) -> Option<TaskDropPlan> {
    let last_idx = rows.iter().rposition(|r| matches!(r, Row::Task { .. }))?;
    let dragged_idx = rows.iter().position(|r| row_task(r).is_some_and(|(t, _, _)| t.id == dragged_id))?;
    let (dragged_task, dragged_depth, _) = row_task(&rows[dragged_idx])?;
    let (group_start, group_end, group_drop) = group_bounds(rows, last_idx);
    let (_, dragged_group_end, _) = group_bounds(rows, dragged_idx);
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
mod tests {
    use super::*;
    use crate::i18n::{EN, RU};
    use uuid::Uuid;

    fn task(title: &str) -> Task {
        Task {
            id: Uuid::now_v7(),
            list_id: None,
            parent_id: None,
            kind: Kind::Task,
            title: title.into(),
            notes: String::new(),
            priority: Priority::None,
            due_date: None,
            due_time: None,
            repeat: None,
            tags: Vec::new(),
            checklist: None,
            completed_at: None,
            deleted_at: None,
            wont_do_at: None,
            end_date: None,
            end_time: None,
            modified_at: None,
            pinned_at: None,
            section_id: None,
            reminders: Vec::new(),
            estimate: None,
            created_at: 0,
        }
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Section { section, count, .. } => format!("# {} {count}", section.title(&EN)),
                Row::Day { label, count, .. } => format!("## {label} {count}"),
                Row::Task { task, depth, .. } => format!("{}{}", "  ".repeat(*depth), task.title),
            })
            .collect()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn single_section_has_no_header_and_nests_children() {
        let parent = task("parent");
        let mut child = task("child");
        child.parent_id = Some(parent.id);
        let mut grandchild = task("grandchild");
        grandchild.parent_id = Some(child.id);
        let rows = build_rows(&[grandchild, parent.clone(), child.clone()], &HashSet::new(), &HashSet::new(), true);
        assert_eq!(shape(&rows), ["parent", "  child", "    grandchild"]);

        let rows = build_rows(
            &[parent.clone(), child],
            &HashSet::from([parent.id]),
            &HashSet::new(),
            true,
        );
        assert_eq!(shape(&rows), ["parent"]);
        assert!(matches!(rows[0], Row::Task { has_children: true, collapsed: true, .. }));
    }

    #[test]
    fn sections_split_by_kind_and_completion() {
        let a = task("a task");
        let mut note = task("a note");
        note.kind = Kind::Note;
        let mut done = task("done");
        done.completed_at = Some(1);
        let rows = build_rows(&[a, note, done], &HashSet::new(), &HashSet::from([Section::Completed]), true);
        assert_eq!(shape(&rows), ["# Tasks 1", "a task", "# Notes 1", "a note", "# Completed 1"]);
    }

    #[test]
    fn match_ranges_ignore_case_and_handle_unicode() {
        assert_eq!(match_ranges("Buy Milk and milk", "MILK"), [4..8, 13..17]);
        let text = "Купить хлеб";
        let ranges = match_ranges(text, "ХЛЕБ");
        assert_eq!(&text[ranges[0].clone()], "хлеб");
        assert!(match_ranges("abc", "  ").is_empty());
    }

    #[test]
    fn snippets_cut_around_the_match() {
        assert_eq!(snippet("one two three four", "three", 4).as_deref(), Some("…two three fou…"));
        assert_eq!(snippet("short", "sho", 10).as_deref(), Some("short"));
        assert_eq!(snippet("line one\nline two", "two", 20).as_deref(), Some("line one line two"));
        assert_eq!(snippet("nothing", "zzz", 5), None);
    }

    #[test]
    fn rows_by_list_section() {
        let list = Uuid::now_v7();
        let kitchen = ListSection { id: Uuid::now_v7(), list_id: Some(list), name: "Kitchen".into() };
        let garden = ListSection { id: Uuid::now_v7(), list_id: Some(list), name: "Garden".into() };
        let mut tap = task("fix the tap");
        tap.section_id = Some(kitchen.id);
        let mut done = task("done");
        done.completed_at = Some(1);
        let rows = section_rows(&[task("loose"), tap, done], &[kitchen, garden], &HashSet::new(), &EN);
        assert_eq!(shape(&rows), [
            "## Not Sectioned 1", "loose", "## Kitchen 1", "fix the tap", "## Garden 0", "## Completed 1", "done"
        ]);
    }

    #[test]
    fn kanban_columns_follow_the_groups() {
        let mut child = task("child");
        child.parent_id = Some(Uuid::now_v7());
        let rows = vec![
            Row::Section { section: Section::Tasks, count: 2, collapsed: false },
            Row::Task { task: task("parent"), depth: 0, has_children: true, collapsed: false },
            Row::Task { task: child, depth: 1, has_children: false, collapsed: false },
            Row::Task { task: task("solo"), depth: 0, has_children: false, collapsed: false },
            Row::Section { section: Section::Notes, count: 0, collapsed: true },
        ];
        let columns = kanban_columns(rows, &EN);
        let shape: Vec<(String, Vec<(String, usize)>)> = columns
            .into_iter()
            .map(|(title, _, cards)| (title, cards.into_iter().map(|(t, subs)| (t.title, subs.len())).collect()))
            .collect();
        assert_eq!(shape, [
            ("Tasks".to_owned(), vec![("parent".to_owned(), 1), ("solo".to_owned(), 0)]),
            ("Notes".to_owned(), vec![]),
        ]);
        assert_eq!(kanban_columns(vec![Row::Task { task: task("a"), depth: 0, has_children: false, collapsed: false }], &EN)[0].0, "");
    }

    #[test]
    fn pinned_tasks_go_first() {
        let mut tasks = vec![task("a"), task("b"), task("c"), task("d")];
        tasks[2].pinned_at = Some(1);
        tasks[3].pinned_at = Some(2);
        pinned_first(&mut tasks);
        let titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["d", "c", "a", "b"]);
    }

    #[test]
    fn sorting_and_grouping() {
        let today = d(2026, 10, 18);
        let mut a = task("banana");
        a.due_date = Some(d(2026, 10, 17));
        a.priority = Priority::Low;
        let mut b = task("Apple");
        b.due_date = Some(today);
        b.priority = Priority::High;
        b.tags = vec!["work".into()];
        let mut c = task("cherry");
        c.tags = vec!["home".into()];
        let mut done = task("done");
        done.completed_at = Some(1);
        let tasks = vec![a, b, c, done];

        let titles = |tasks: &[Task]| tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        let mut sorted = tasks.clone();
        order_tasks(&mut sorted, SortBy::Title, true);
        assert!(sorted.windows(2).all(|w| w[0].title.to_lowercase() >= w[1].title.to_lowercase()), "Descending turns the sort around");
        sort_tasks(&mut sorted, SortBy::Title);
        assert_eq!(titles(&sorted), ["Apple", "banana", "cherry", "done"]);
        sort_tasks(&mut sorted, SortBy::Priority);
        assert_eq!(titles(&sorted)[..2], ["Apple", "banana"]);
        sort_tasks(&mut sorted, SortBy::Date);
        assert_eq!(titles(&sorted)[..2], ["banana", "Apple"]);
        sort_tasks(&mut sorted, SortBy::Tag);
        assert_eq!(titles(&sorted)[..2], ["cherry", "Apple"]);
        sorted[3].modified_at = Some(10);
        sort_tasks(&mut sorted, SortBy::Modified);
        assert_eq!(sorted[0].modified_at, Some(10));

        assert_eq!(shape(&grouped_rows(&tasks, GroupBy::Date, today, false, &EN)), [
            "## Overdue 1", "banana", "## Today 1", "Apple", "## No Date 1", "cherry", "## Completed 1", "done"
        ]);
        assert_eq!(shape(&grouped_rows(&tasks, GroupBy::Tag, today, false, &EN)), [
            "## home 1", "cherry", "## work 1", "Apple", "## No Tags 1", "banana", "## Completed 1", "done"
        ]);
        assert_eq!(shape(&grouped_rows(&tasks, GroupBy::None, today, false, &EN)), ["banana", "Apple", "cherry", "done"]);
        let work = fern_core::Id::now_v7();
        let mut in_work = tasks.clone();
        in_work[0].list_id = Some(work);
        let by_list = shape(&grouped_rows_in(&in_work, GroupBy::List, today, false, &[(work, "Work".into())], &EN));
        assert!(by_list[0].starts_with("## Inbox"), "the Inbox first, then the lists in their order");
        assert!(by_list.iter().any(|l| l == "## Work 1"));
        // "Overdue Section shows at: Bottom of List".
        assert_eq!(shape(&grouped_rows(&tasks, GroupBy::Date, today, true, &EN)), [
            "## Today 1", "Apple", "## No Date 1", "cherry", "## Overdue 1", "banana", "## Completed 1", "done"
        ]);
        let drops = |rows: Vec<Row>| -> Vec<Option<DropTarget>> {
            rows.into_iter().filter_map(|r| match r { Row::Day { drop, .. } => Some(drop), _ => None }).collect()
        };
        let targets = drops(grouped_rows_in(&in_work, GroupBy::List, today, false, &[(work, "Work".into())], &EN));
        assert!(targets.contains(&Some(DropTarget::List(Some(work)))) && targets.contains(&Some(DropTarget::List(None))), "a list column takes cards into its list");
        assert_eq!(drops(grouped_rows(&tasks, GroupBy::Date, today, false, &EN)), [
            None,
            Some(DropTarget::Due(Some(today))),
            Some(DropTarget::Due(None)),
            Some(DropTarget::Complete),
        ]);
        assert_eq!(drops(grouped_rows(&tasks, GroupBy::Tag, today, false, &EN))[..2], [
            Some(DropTarget::Tag(Some("home".into()))),
            Some(DropTarget::Tag(Some("work".into()))),
        ]);
        assert_eq!(SortBy::from_key(SortBy::Tag.key()), SortBy::Tag);
        assert_eq!(GroupBy::from_key("nonsense"), GroupBy::Custom);
    }

    #[test]
    fn closed_ranges() {
        let today = d(2026, 10, 18); // a Sunday
        let r = |range: ClosedRange, day| range.contains(day, today);
        assert!(r(ClosedRange::ThisWeek, d(2026, 10, 12)) && !r(ClosedRange::ThisWeek, d(2026, 10, 11)));
        assert!(r(ClosedRange::LastWeek, d(2026, 10, 5)) && r(ClosedRange::LastWeek, d(2026, 10, 11)));
        assert!(!r(ClosedRange::LastWeek, d(2026, 10, 12)) && !r(ClosedRange::LastWeek, d(2026, 10, 4)));
        assert!(r(ClosedRange::ThisMonth, d(2026, 10, 1)) && !r(ClosedRange::ThisMonth, d(2025, 10, 1)));
        assert!(r(ClosedRange::All, d(2000, 1, 1)));
        let custom = ClosedRange::custom(d(2026, 10, 9), d(2026, 9, 28));
        assert_eq!(custom, ClosedRange::Custom(d(2026, 9, 28), d(2026, 10, 9)));
        assert!(r(custom, d(2026, 9, 28)) && r(custom, d(2026, 10, 9)) && !r(custom, d(2026, 10, 10)));
        assert_eq!(custom.label(&EN), "Sep 28 - Oct 9");
    }

    #[test]
    fn day_headers() {
        let today = d(2026, 10, 18);
        assert_eq!(day_header(d(2026, 9, 27), today, &EN), "Sunday, September 27");
        assert_eq!(day_header(d(2025, 9, 26), today, &EN), "Friday, September 26, 2025");
        assert_eq!(day_header(d(2026, 9, 27), today, &RU), "воскресенье, 27 сентября");
    }

    #[test]
    fn next_days_groups() {
        let today = d(2026, 10, 18);
        let mon = chrono::Weekday::Mon;
        assert_eq!(next_days_header(today, today, mon, &EN), "Sunday, Today");
        assert_eq!(next_days_header(d(2026, 10, 19), today, mon, &EN), "Monday, Tomorrow");
        assert_eq!(next_days_header(d(2026, 10, 24), today, mon, &EN), "Next Saturday, October 24");
        let mut late = task("late");
        late.due_date = Some(d(2026, 10, 17));
        let mut sat = task("sat");
        sat.due_date = Some(d(2026, 10, 24));
        assert_eq!(shape(&next_days_rows(&[sat, late], today, mon, &EN)), ["## Overdue 1", "late", "## Next Saturday, October 24 1", "sat"]);
    }

    #[test]
    fn ungrouped_lists_have_no_headers() {
        let a = task("open");
        let mut done = task("done");
        done.completed_at = Some(1);
        let rows = build_rows(&[a, done], &HashSet::new(), &HashSet::new(), false);
        assert_eq!(shape(&rows), ["open", "done"]);
    }

    #[test]
    fn child_in_other_section_goes_top_level() {
        let parent = task("parent");
        let mut child = task("finished child");
        child.parent_id = Some(parent.id);
        child.completed_at = Some(1);
        let rows = build_rows(&[parent, child], &HashSet::new(), &HashSet::new(), true);
        assert_eq!(shape(&rows), ["# Tasks 1", "parent", "# Completed 1", "finished child"]);
    }

    #[test]
    fn due_labels_english() {
        let today = d(2026, 10, 18); // a Sunday
        let label = |date| due_label(date, None, today, &EN);
        assert_eq!(label(today), ("Today".into(), false));
        assert_eq!(label(d(2026, 10, 19)), ("Tomorrow".into(), false));
        assert_eq!(label(d(2026, 10, 17)), ("Yesterday".into(), true));
        assert_eq!(label(d(2026, 10, 24)), ("Sat".into(), false));
        assert_eq!(label(d(2026, 11, 13)), ("Nov 13".into(), false));
        assert_eq!(label(d(2026, 10, 1)), ("Oct 1".into(), true));
        assert_eq!(label(d(2027, 1, 5)), ("Jan 5 2027".into(), false));
        assert_eq!(
            due_label(today, NaiveTime::from_hms_opt(18, 30, 0), today, &EN),
            ("Today 18:30".into(), false)
        );
    }

    #[test]
    fn detail_due_labels() {
        let today = d(2026, 10, 18);
        assert_eq!(detail_due_label("Nov 13".into(), d(2026, 11, 13), today, &EN), "Nov 13, 26d left");
        assert_eq!(detail_due_label("Tomorrow".into(), d(2026, 10, 19), today, &EN), "Tomorrow");
        assert_eq!(detail_due_label("Yesterday".into(), d(2026, 10, 17), today, &EN), "Yesterday");
        assert_eq!(detail_due_label("Oct 13".into(), d(2026, 10, 13), today, &EN), "Oct 13, 5d overdue");
    }

    #[test]
    fn card_due_labels() {
        let today = d(2026, 10, 18);
        assert_eq!(card_due_label(d(2026, 10, 24), None, today, &EN), "Sat, Oct 24");
        assert_eq!(card_due_label(d(2027, 1, 4), None, today, &EN), "Mon, Jan 4, 2027");
    }

    #[test]
    fn due_labels_russian() {
        let today = d(2026, 10, 18);
        assert_eq!(row_due_label(today + chrono::Duration::days(3), None, today, &EN, true).0, "in 3 days");
        assert_eq!(row_due_label(today - chrono::Duration::days(2), None, today, &EN, true), ("2 days ago".to_owned(), true));
        assert_eq!(row_due_label(today, None, today, &EN, true).0, "Today");
        assert_eq!(closed_due_label(today, today, &EN), "Oct 18", "a closed task shows the date itself");
        assert_eq!(closed_due_label(d(2025, 9, 26), today, &EN), "Sep 26, 2025");
        assert_eq!(closed_due_label(d(2025, 9, 24), today, &RU), "24.09.2025", "as the Russian original");
        assert_eq!(closed_day(d(2026, 10, 17), today, &RU), "17 окт Сб");
        let label = |date| due_label(date, None, today, &RU).0;
        assert_eq!(label(today), "Сегодня");
        assert_eq!(label(d(2026, 10, 24)), "Сб");
        assert_eq!(label(d(2026, 11, 13)), "13 ноя");
        assert_eq!(label(d(2027, 1, 5)), "5 янв 2027");
    }

    #[test]
    fn plan_task_drop_unnests_in_two_task_list_by_all_gestures() {
        let parent = task("parent");
        let mut child = task("child");
        child.parent_id = Some(parent.id);
        let rows = build_rows(&[parent.clone(), child.clone()], &HashSet::new(), &HashSet::new(), true);

        // 1) Drag child left on its own row -> top-level right after parent.
        let plan = plan_task_drop(&rows, child.id, child.id, false, -24.0, 60.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);
        assert_eq!(plan.order_target, Some((parent.id, true)));
        assert_eq!(plan.target_id, child.id);
        assert!(plan.after);

        // 2) Drag child slightly left near the left gutter -> snaps to root.
        let plan = plan_task_drop(&rows, child.id, child.id, false, -8.0, 30.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);

        // 3) Drag child above parent -> top-level right before parent regardless of X.
        let plan = plan_task_drop(&rows, child.id, parent.id, false, 0.0, 120.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);
        assert_eq!(plan.order_target, Some((parent.id, false)));
        assert_eq!(plan.target_id, parent.id);
        assert!(!plan.after);

        // 4) Drag child onto parent's bottom half without moving right -> top-level right after parent.
        let plan = plan_task_drop(&rows, child.id, parent.id, true, 0.0, 120.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);
        assert_eq!(plan.order_target, Some((parent.id, true)));

        // 5) Drag child into empty space below the list -> top-level right after parent.
        let plan = plan_task_drop_at_end(&rows, child.id).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);
        assert_eq!(plan.order_target, Some((parent.id, true)));

        // Staying put on own row with no horizontal movement is a no-op.
        assert!(plan_task_drop(&rows, child.id, child.id, false, 0.0, 120.0, 24.0, 20.0, true).is_none());
    }

    #[test]
    fn plan_task_drop_nests_outdents_multi_level_and_guards_cycles_and_notes() {
        let a = task("a");
        let mut b = task("b");
        b.parent_id = Some(a.id);
        let mut c = task("c");
        c.parent_id = Some(b.id);
        let solo = task("solo");
        let rows = build_rows(&[a.clone(), b.clone(), c.clone(), solo.clone()], &HashSet::new(), &HashSet::new(), true);

        // Cannot drop parent onto its own descendant.
        assert!(plan_task_drop(&rows, a.id, b.id, true, 0.0, 100.0, 24.0, 20.0, true).is_none());
        assert!(plan_task_drop(&rows, a.id, c.id, false, 0.0, 100.0, 24.0, 20.0, true).is_none());

        // Drag solo right onto its own row after c (depth 2) -> nests under c (depth 3)?
        // Wait: solo is depth 0; prev is c (depth 2), so on solo's own row base_level = 0, dx = +24 -> depth 1 (under a!).
        let plan = plan_task_drop(&rows, solo.id, solo.id, false, 24.0, 120.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 1);
        assert_eq!(plan.new_parent, Some(a.id));
        assert_eq!(plan.order_target, Some((b.id, true)));

        // Drag solo onto bottom half of c with dx = +24 -> nests under c (depth 3).
        let plan = plan_task_drop(&rows, solo.id, c.id, true, 24.0, 140.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 3);
        assert_eq!(plan.new_parent, Some(c.id));

        // Drag solo onto bottom half of a (expanded, next is b at depth 1) -> forced to depth 1 before b.
        let plan = plan_task_drop(&rows, solo.id, a.id, true, 0.0, 100.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 1);
        assert_eq!(plan.new_parent, Some(a.id));
        assert_eq!(plan.order_target, Some((b.id, false)));

        // Drag c left by 1 step on own row -> becomes sibling of b under a, right after b.
        let plan = plan_task_drop(&rows, c.id, c.id, false, -24.0, 100.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 1);
        assert_eq!(plan.new_parent, Some(a.id));
        assert_eq!(plan.order_target, Some((b.id, true)));

        // Drag c left by 2 steps on own row -> becomes root right before solo.
        let plan = plan_task_drop(&rows, c.id, c.id, false, -48.0, 40.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);
        assert_eq!(plan.order_target, Some((solo.id, false)));

        // Dragging onto a collapsed parent requests auto-expanding it.
        let collapsed_rows = build_rows(&[a.clone(), b.clone(), solo.clone()], &HashSet::from([a.id]), &HashSet::new(), true);
        let plan = plan_task_drop(&collapsed_rows, solo.id, a.id, true, 24.0, 120.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.new_parent, Some(a.id));
        assert_eq!(plan.expand_parent, Some(a.id));

        // Notes cannot nest or be dropped inside an expanded subtask tree.
        let mut note = task("note");
        note.kind = Kind::Note;
        let note_rows = build_rows(&[a.clone(), b, note.clone()], &HashSet::new(), &HashSet::new(), false);
        assert!(plan_task_drop(&note_rows, note.id, a.id, true, 24.0, 120.0, 24.0, 20.0, true).is_none());
        let plan = plan_task_drop(&note_rows, a.id, note.id, true, 24.0, 120.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, 0);
        assert_eq!(plan.new_parent, None);
    }

    #[test]
    fn plan_task_drop_respects_max_subtask_depth() {
        let mut chain: Vec<Task> = Vec::new();
        for i in 0..=fern_core::MAX_SUBTASK_DEPTH {
            let mut t = task(&format!("L{i}"));
            if let Some(prev) = chain.last() {
                t.parent_id = Some(prev.id);
            }
            chain.push(t);
        }
        let extra = task("extra");
        let mut all = chain.clone();
        all.push(extra.clone());
        let rows = build_rows(&all, &HashSet::new(), &HashSet::new(), true);
        let deepest = chain.last().unwrap();
        let parent_of_deepest = &chain[fern_core::MAX_SUBTASK_DEPTH - 1];

        // Even dragging right under a depth-4 task caps at MAX_SUBTASK_DEPTH (4), sibling of deepest.
        let plan = plan_task_drop(&rows, extra.id, deepest.id, true, 48.0, 220.0, 24.0, 20.0, true).unwrap();
        assert_eq!(plan.target_level, fern_core::MAX_SUBTASK_DEPTH);
        assert_eq!(plan.new_parent, Some(parent_of_deepest.id));
    }
}
