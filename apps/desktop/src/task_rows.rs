//! Turns a task list into rows: groups, the subtask tree and due-date labels.
//! Kept free of GPUI so it can be unit-tested.

use std::collections::{HashMap, HashSet};

use fern_core::{Id, Kind, Priority, Task};
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
    Task { task: Task, depth: usize, has_children: bool, collapsed: bool },
}

/// Rows for the list. Group headers appear only when more than one group is non-empty.
/// A subtask nests under its parent when the parent is in the same group; otherwise it is
/// shown at the top level.
pub fn build_rows(
    tasks: &[Task],
    collapsed_tasks: &HashSet<Id>,
    collapsed_sections: &HashSet<Section>,
) -> Vec<Row> {
    let sections = [Section::Tasks, Section::Notes, Section::Completed];
    let grouped: Vec<(Section, Vec<&Task>)> = sections
        .iter()
        .map(|&section| (section, tasks.iter().filter(|t| Section::of(t) == section).collect::<Vec<_>>()))
        .filter(|(_, tasks)| !tasks.is_empty())
        .collect();
    let with_headers = grouped.len() > 1;

    let mut rows = Vec::new();
    for (section, tasks) in grouped {
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
        label.push_str(&time.format(" %H:%M").to_string());
    }
    (label, days < 0)
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
            tags: Vec::new(),
            checklist: None,
            completed_at: None,
            deleted_at: None,
            created_at: 0,
        }
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Section { section, count, .. } => format!("# {} {count}", section.title(&EN)),
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
        let rows = build_rows(&[grandchild, parent.clone(), child.clone()], &HashSet::new(), &HashSet::new());
        assert_eq!(shape(&rows), ["parent", "  child", "    grandchild"]);

        let rows = build_rows(
            &[parent.clone(), child.clone()],
            &HashSet::from([parent.id]),
            &HashSet::new(),
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
        let rows = build_rows(&[a, note, done], &HashSet::new(), &HashSet::from([Section::Completed]));
        assert_eq!(shape(&rows), ["# Tasks 1", "a task", "# Notes 1", "a note", "# Completed 1"]);
    }

    #[test]
    fn child_in_other_section_goes_top_level() {
        let parent = task("parent");
        let mut child = task("finished child");
        child.parent_id = Some(parent.id);
        child.completed_at = Some(1);
        let rows = build_rows(&[parent, child], &HashSet::new(), &HashSet::new());
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
        let label = |date| due_label(date, None, today, &RU).0;
        assert_eq!(label(today), "Сегодня");
        assert_eq!(label(d(2026, 10, 24)), "Сб");
        assert_eq!(label(d(2026, 11, 13)), "13 нояб.");
        assert_eq!(label(d(2027, 1, 5)), "5 янв. 2027");
    }
}
