//! Раскладка списка задач в строки: секции, дерево подзадач, подписи сроков.
//! Без GPUI, чтобы проверять тестами.

use std::collections::{HashMap, HashSet};

use amin_core::{Id, Kind, Task};
use chrono::{Datelike, NaiveDate, NaiveTime, Weekday};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Tasks,
    Notes,
    Completed,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Tasks => "Задачи",
            Section::Notes => "Заметки",
            Section::Completed => "Выполненные",
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

/// Строки для списка. Заголовки секций показываются, только если непустых секций больше одной.
/// Подзадача вкладывается в родителя, если родитель в той же секции; иначе стоит на верхнем уровне.
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

/// Подпись срока в строке и признак просрочки: «Сегодня», «Завтра», «Сб», «13 нояб.», «13 нояб. 2027».
pub fn due_label(date: NaiveDate, time: Option<NaiveTime>, today: NaiveDate) -> (String, bool) {
    let days = (date - today).num_days();
    let mut label = match days {
        0 => "Сегодня".to_owned(),
        1 => "Завтра".to_owned(),
        -1 => "Вчера".to_owned(),
        2..=6 => weekday_short(date.weekday()).to_owned(),
        _ if date.year() == today.year() => format!("{} {}", date.day(), month_short(date.month())),
        _ => format!("{} {} {}", date.day(), month_short(date.month()), date.year()),
    };
    if let Some(time) = time {
        label.push_str(&time.format(" %H:%M").to_string());
    }
    (label, days < 0)
}

fn weekday_short(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Пн",
        Weekday::Tue => "Вт",
        Weekday::Wed => "Ср",
        Weekday::Thu => "Чт",
        Weekday::Fri => "Пт",
        Weekday::Sat => "Сб",
        Weekday::Sun => "Вс",
    }
}

fn month_short(month: u32) -> &'static str {
    [
        "янв.", "февр.", "мар.", "апр.", "мая", "июн.", "июл.", "авг.", "сент.", "окт.", "нояб.",
        "дек.",
    ][(month as usize - 1).min(11)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use amin_core::Priority;
    use uuid::Uuid;

    fn task(title: &str) -> Task {
        Task {
            id: Uuid::now_v7(),
            parent_id: None,
            kind: Kind::Task,
            title: title.into(),
            notes: String::new(),
            priority: Priority::None,
            due_date: None,
            due_time: None,
            tags: Vec::new(),
            completed_at: None,
            created_at: 0,
        }
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Section { section, count, .. } => format!("# {} {count}", section.title()),
                Row::Task { task, depth, .. } => format!("{}{}", "  ".repeat(*depth), task.title),
            })
            .collect()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn single_section_has_no_header_and_nests_children() {
        let parent = task("родитель");
        let mut child = task("ребёнок");
        child.parent_id = Some(parent.id);
        let mut grandchild = task("внук");
        grandchild.parent_id = Some(child.id);
        let rows = build_rows(&[grandchild, parent.clone(), child.clone()], &HashSet::new(), &HashSet::new());
        assert_eq!(shape(&rows), ["родитель", "  ребёнок", "    внук"]);

        let rows = build_rows(
            &[parent.clone(), child.clone()],
            &HashSet::from([parent.id]),
            &HashSet::new(),
        );
        assert_eq!(shape(&rows), ["родитель"]);
        assert!(matches!(rows[0], Row::Task { has_children: true, collapsed: true, .. }));
    }

    #[test]
    fn sections_split_by_kind_and_completion() {
        let a = task("задача");
        let mut note = task("заметка");
        note.kind = Kind::Note;
        let mut done = task("готово");
        done.completed_at = Some(1);
        let rows = build_rows(&[a, note, done], &HashSet::new(), &HashSet::from([Section::Completed]));
        assert_eq!(shape(&rows), ["# Задачи 1", "задача", "# Заметки 1", "заметка", "# Выполненные 1"]);
    }

    #[test]
    fn child_in_other_section_goes_top_level() {
        let parent = task("родитель");
        let mut child = task("готовый ребёнок");
        child.parent_id = Some(parent.id);
        child.completed_at = Some(1);
        let rows = build_rows(&[parent, child], &HashSet::new(), &HashSet::new());
        assert_eq!(shape(&rows), ["# Задачи 1", "родитель", "# Выполненные 1", "готовый ребёнок"]);
    }

    #[test]
    fn due_labels() {
        let today = d(2026, 10, 18); // воскресенье
        assert_eq!(due_label(today, None, today), ("Сегодня".into(), false));
        assert_eq!(due_label(d(2026, 10, 19), None, today), ("Завтра".into(), false));
        assert_eq!(due_label(d(2026, 10, 17), None, today), ("Вчера".into(), true));
        assert_eq!(due_label(d(2026, 10, 24), None, today), ("Сб".into(), false));
        assert_eq!(due_label(d(2026, 11, 13), None, today), ("13 нояб.".into(), false));
        assert_eq!(due_label(d(2026, 10, 1), None, today), ("1 окт.".into(), true));
        assert_eq!(due_label(d(2027, 1, 5), None, today), ("5 янв. 2027".into(), false));
        assert_eq!(
            due_label(today, NaiveTime::from_hms_opt(18, 30, 0), today),
            ("Сегодня 18:30".into(), false)
        );
    }
}
