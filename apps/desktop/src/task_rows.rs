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
    let month = strings.months_short[date.month0() as usize];
    if strings.day_before_month { format!("{} {month}", date.day()) } else { format!("{month} {}", date.day()) }
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
        let weekday = strings.weekdays_full[day.weekday().num_days_from_monday() as usize];
        let label = match (today - day).num_days() {
            0 => format!("{weekday}, {}", strings.today),
            1 => format!("{weekday}, {}", strings.yesterday),
            _ => day_header(day, today, strings),
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

/// "Sunday, September 27", with the year when it is not the current one.
pub fn day_header(day: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    let weekday = strings.weekdays_full[day.weekday().num_days_from_monday() as usize];
    let month = strings.months_genitive[day.month0() as usize];
    let mut label = if strings.day_before_month {
        format!("{weekday}, {} {month}", day.day())
    } else {
        format!("{weekday}, {month} {}", day.day())
    };
    if day.year() != today.year() {
        label.push_str(&format!(", {}", day.year()));
    }
    label
}

/// A day's header in Next 7 Days, as the original's: "Sunday, Today", "Monday, Tomorrow",
/// "Wednesday, October 21", and "Next Saturday, October 24" for a day in the next week.
pub fn next_days_header(day: NaiveDate, today: NaiveDate, week_start: chrono::Weekday, strings: &Strings) -> String {
    let weekday = strings.weekdays_full[day.weekday().num_days_from_monday() as usize];
    let offset = (day - today).num_days();
    if offset == 0 || offset == 1 {
        return format!("{weekday}, {}", if offset == 0 { strings.today } else { strings.tomorrow });
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
    let day_month = || {
        let month = strings.months_short[date.month0() as usize];
        if strings.day_before_month {
            format!("{} {month}", date.day())
        } else {
            format!("{month} {}", date.day())
        }
    };
    let mut label = match days {
        0 => strings.today.to_owned(),
        1 => strings.tomorrow.to_owned(),
        -1 => strings.yesterday.to_owned(),
        2..=6 => strings.weekdays_short[date.weekday().num_days_from_monday() as usize].to_owned(),
        _ if date.year() == today.year() => day_month(),
        _ => format!("{} {}", day_month(), date.year()),
    };
    if let Some(time) = time {
        label.push_str(&format!(" {}", crate::calendar::format_time(time)));
    }
    (label, days < 0)
}

/// A closed task's date, as the original's Completed and Won't Do show it: the date itself,
/// "Oct 15" ("Oct 15, 2025" in another year), never "Today" or "Tomorrow".
pub fn closed_due_label(date: NaiveDate, today: NaiveDate, strings: &Strings) -> String {
    let month = strings.months_short[date.month0() as usize];
    let day = if strings.day_before_month { format!("{} {month}", date.day()) } else { format!("{month} {}", date.day()) };
    if date.year() == today.year() { day } else { format!("{day}, {}", date.year()) }
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
            &[parent.clone(), child.clone()],
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
    fn due_labels_russian() {
        let today = d(2026, 10, 18);
        assert_eq!(row_due_label(today + chrono::Duration::days(3), None, today, &EN, true).0, "in 3 days");
        assert_eq!(row_due_label(today - chrono::Duration::days(2), None, today, &EN, true), ("2 days ago".to_owned(), true));
        assert_eq!(row_due_label(today, None, today, &EN, true).0, "Today");
        assert_eq!(closed_due_label(today, today, &EN), "Oct 18", "a closed task shows the date itself");
        assert_eq!(closed_due_label(d(2025, 9, 26), today, &EN), "Sep 26, 2025");
        let label = |date| due_label(date, None, today, &RU).0;
        assert_eq!(label(today), "Сегодня");
        assert_eq!(label(d(2026, 10, 24)), "Сб");
        assert_eq!(label(d(2026, 11, 13)), "13 нояб.");
        assert_eq!(label(d(2027, 1, 5)), "5 янв. 2027");
    }
}
