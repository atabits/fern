//! Things 3-style task queries, counts, project progress, and schedule mutations.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::params;

use crate::model::{Id, List, ScheduleBucket, Task};
use crate::store::{DATE_FORMAT, Store, parse_uuid};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThingsViewKind {
    Inbox,
    Today,
    Upcoming,
    Anytime,
    Someday,
    Logbook,
    Project(Id),
    Area(Id),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThingsCounts {
    pub inbox: usize,
    pub today: usize,
    /// Subset of `today` marked `ScheduleBucket::Evening`.
    pub today_evening: usize,
    pub upcoming: usize,
    pub anytime: usize,
    pub someday: usize,
    pub logbook: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectProgress {
    pub list_id: Id,
    pub completed: usize,
    pub total: usize,
}

impl ProjectProgress {
    pub fn ratio(self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f32 / self.total as f32
        }
    }
}

fn hidden_project_ids(lists: &[List]) -> HashSet<Id> {
    lists
        .iter()
        .filter(|l| l.archived || l.completed_at.is_some() || !l.in_smart_lists)
        .map(|l| l.id)
        .collect()
}

fn matches_things_view(
    task: &Task,
    view: &ThingsViewKind,
    today: NaiveDate,
    hidden_lists: &HashSet<Id>,
    area_lists: Option<&HashSet<Id>>,
) -> bool {
    if task.is_deleted() {
        return false;
    }
    let in_hidden_list = task.list_id.is_some_and(|id| hidden_lists.contains(&id));
    match view {
        ThingsViewKind::Inbox => {
            !task.is_closed()
                && task.list_id.is_none()
                && task.start_date.is_none()
                && task.due_date.is_none()
                && task.schedule_bucket == ScheduleBucket::None
        }
        ThingsViewKind::Today => {
            !task.is_closed()
                && !in_hidden_list
                && task.schedule_bucket != ScheduleBucket::Someday
                && (task.start_date.is_some_and(|d| d <= today)
                    || (task.start_date.is_none() && task.due_date.is_some_and(|d| d <= today)))
        }
        ThingsViewKind::Upcoming => {
            !task.is_closed()
                && !in_hidden_list
                && task.schedule_bucket != ScheduleBucket::Someday
                && (task.start_date.is_some_and(|d| d > today)
                    || (task.start_date.is_none() && task.due_date.is_some_and(|d| d > today)))
        }
        ThingsViewKind::Anytime => {
            !task.is_closed()
                && !in_hidden_list
                && task.schedule_bucket != ScheduleBucket::Someday
                && task.start_date.is_none_or(|d| d <= today)
                && (task.list_id.is_some() || task.start_date.is_some() || task.due_date.is_some())
        }
        ThingsViewKind::Someday => {
            !task.is_closed() && !in_hidden_list && task.schedule_bucket == ScheduleBucket::Someday
        }
        ThingsViewKind::Logbook => task.is_closed(),
        ThingsViewKind::Project(list_id) => task.list_id == Some(*list_id),
        ThingsViewKind::Area(_) => {
            !task.is_closed()
                && task
                    .list_id
                    .is_some_and(|id| area_lists.is_some_and(|set| set.contains(&id)))
        }
    }
}

impl Store {
    pub fn things_tasks(&self, view: &ThingsViewKind, today: NaiveDate) -> Result<Vec<Task>> {
        let tasks = self.tasks()?;
        let lists = self.lists()?;
        Ok(self.things_tasks_from(tasks, &lists, view, today))
    }

    pub fn things_tasks_from(
        &self,
        tasks: Vec<Task>,
        lists: &[List],
        view: &ThingsViewKind,
        today: NaiveDate,
    ) -> Vec<Task> {
        let hidden = hidden_project_ids(lists);
        let area_lists: Option<HashSet<Id>> = match view {
            ThingsViewKind::Area(folder_id) => Some(
                lists
                    .iter()
                    .filter(|l| l.folder == Some(*folder_id) && !l.archived && l.completed_at.is_none())
                    .map(|l| l.id)
                    .collect(),
            ),
            _ => None,
        };
        let mut filtered: Vec<Task> = tasks
            .into_iter()
            .filter(|task| matches_things_view(task, view, today, &hidden, area_lists.as_ref()))
            .collect();
        if *view == ThingsViewKind::Logbook {
            filtered.sort_by(|a, b| {
                let a_at = a.completed_at.or(a.wont_do_at).unwrap_or(0);
                let b_at = b.completed_at.or(b.wont_do_at).unwrap_or(0);
                b_at.cmp(&a_at).then_with(|| b.created_at.cmp(&a.created_at))
            });
        }
        filtered
    }

    pub fn things_counts(&self, today: NaiveDate) -> Result<ThingsCounts> {
        let tasks = self.tasks()?;
        let lists = self.lists()?;
        Ok(self.things_counts_from(&tasks, &lists, today))
    }

    pub fn things_counts_from(
        &self,
        tasks: &[Task],
        lists: &[List],
        today: NaiveDate,
    ) -> ThingsCounts {
        let hidden = hidden_project_ids(lists);
        let mut counts = ThingsCounts::default();
        for task in tasks {
            if matches_things_view(task, &ThingsViewKind::Inbox, today, &hidden, None) {
                counts.inbox += 1;
            }
            if matches_things_view(task, &ThingsViewKind::Today, today, &hidden, None) {
                counts.today += 1;
                if task.schedule_bucket == ScheduleBucket::Evening {
                    counts.today_evening += 1;
                }
            }
            if matches_things_view(task, &ThingsViewKind::Upcoming, today, &hidden, None) {
                counts.upcoming += 1;
            }
            if matches_things_view(task, &ThingsViewKind::Anytime, today, &hidden, None) {
                counts.anytime += 1;
            }
            if matches_things_view(task, &ThingsViewKind::Someday, today, &hidden, None) {
                counts.someday += 1;
            }
            if matches_things_view(task, &ThingsViewKind::Logbook, today, &hidden, None) {
                counts.logbook += 1;
            }
        }
        counts
    }

    pub fn project_progress(&self, list_id: Id) -> Result<ProjectProgress> {
        let (total, completed): (i64, i64) = self.conn.prepare_cached(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN completed_at IS NOT NULL OR wont_do_at IS NOT NULL THEN 1 ELSE 0 END), 0)
             FROM tasks WHERE list_id = ?1 AND deleted_at IS NULL",
        )?
        .query_row([list_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(ProjectProgress {
            list_id,
            completed: usize::try_from(completed.max(0)).unwrap_or(0),
            total: usize::try_from(total.max(0)).unwrap_or(0),
        })
    }

    pub fn all_project_progress(&self) -> Result<HashMap<Id, ProjectProgress>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT l.id,
                    COUNT(t.id),
                    COALESCE(SUM(CASE WHEN t.id IS NOT NULL AND (t.completed_at IS NOT NULL OR t.wont_do_at IS NOT NULL) THEN 1 ELSE 0 END), 0)
             FROM lists l
             LEFT JOIN tasks t ON t.list_id = l.id AND t.deleted_at IS NULL
             GROUP BY l.id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let list_id = parse_uuid(&row.get::<_, String>(0)?, 0)?;
                let total: i64 = row.get(1)?;
                let completed: i64 = row.get(2)?;
                Ok((
                    list_id,
                    ProjectProgress {
                        list_id,
                        completed: usize::try_from(completed.max(0)).unwrap_or(0),
                        total: usize::try_from(total.max(0)).unwrap_or(0),
                    },
                ))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(rows)
    }

    pub fn set_task_schedule(
        &self,
        id: Id,
        start_date: Option<NaiveDate>,
        bucket: ScheduleBucket,
    ) -> Result<()> {
        let date_str = start_date.map(|d| d.format(DATE_FORMAT).to_string());
        self.conn.execute(
            "UPDATE tasks SET start_date = ?2, schedule_bucket = ?3 WHERE id = ?1",
            params![id.to_string(), date_str, bucket.as_str()],
        )?;
        Ok(())
    }

    pub fn set_list_project_fields(
        &self,
        id: Id,
        notes: &str,
        deadline: Option<NaiveDate>,
        completed_at: Option<i64>,
    ) -> Result<()> {
        let deadline_str = deadline.map(|d| d.format(DATE_FORMAT).to_string());
        self.conn.execute(
            "UPDATE lists SET notes = ?2, deadline = ?3, completed_at = ?4 WHERE id = ?1",
            params![id.to_string(), notes, deadline_str, completed_at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ListDraft, TaskDraft};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn things_views_counts_and_project_progress() -> Result<()> {
        let store = Store::open_in_memory()?;
        let today = d(2026, 11, 22);
        let yesterday = d(2026, 11, 21);
        let tomorrow = d(2026, 11, 23);

        let folder = store.add_list_folder("Work")?;
        let proj = store.add_list(&ListDraft {
            name: "Launch".into(),
            folder: Some(folder.id),
            notes: "Ship v1".into(),
            deadline: Some(d(2026, 12, 1)),
            ..Default::default()
        })?;
        assert_eq!(proj.notes, "Ship v1");
        assert_eq!(proj.deadline, Some(d(2026, 12, 1)));

        let inbox = store.add_task("Inbox task")?;
        let today_day = store.add_task("Today day")?;
        store.set_task_schedule(today_day.id, Some(today), ScheduleBucket::None)?;
        let today_eve = store.add_task("Today evening")?;
        store.set_task_schedule(today_eve.id, Some(today), ScheduleBucket::Evening)?;
        let due_fallback = store.create_task(
            TaskDraft { title: "Overdue due".into(), due_date: Some(yesterday), ..Default::default() },
            true,
        )?;
        let upcoming = store.add_task("Upcoming")?;
        store.set_task_schedule(upcoming.id, Some(tomorrow), ScheduleBucket::None)?;
        let someday = store.add_task("Someday")?;
        store.set_task_schedule(someday.id, None, ScheduleBucket::Someday)?;
        let anytime_proj = store.create_task(
            TaskDraft { title: "Project item".into(), list_id: Some(proj.id), ..Default::default() },
            true,
        )?;
        let done_proj = store.create_task(
            TaskDraft { title: "Done project item".into(), list_id: Some(proj.id), ..Default::default() },
            true,
        )?;
        store.set_completed(done_proj.id, true)?;

        assert_eq!(
            store.things_tasks(&ThingsViewKind::Inbox, today)?.into_iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![inbox.id]
        );
        let today_ids: Vec<Id> = store.things_tasks(&ThingsViewKind::Today, today)?.into_iter().map(|t| t.id).collect();
        assert_eq!(today_ids.len(), 3);
        assert!(today_ids.contains(&today_day.id) && today_ids.contains(&today_eve.id) && today_ids.contains(&due_fallback.id));

        assert_eq!(
            store.things_tasks(&ThingsViewKind::Upcoming, today)?.into_iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![upcoming.id]
        );
        assert_eq!(
            store.things_tasks(&ThingsViewKind::Someday, today)?.into_iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![someday.id]
        );
        let anytime_ids: Vec<Id> = store.things_tasks(&ThingsViewKind::Anytime, today)?.into_iter().map(|t| t.id).collect();
        assert!(anytime_ids.contains(&anytime_proj.id));
        assert!(anytime_ids.contains(&today_day.id));
        assert!(!anytime_ids.contains(&inbox.id));
        assert!(!anytime_ids.contains(&upcoming.id));
        assert!(!anytime_ids.contains(&someday.id));

        assert_eq!(
            store.things_tasks(&ThingsViewKind::Area(folder.id), today)?.into_iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![anytime_proj.id]
        );
        assert_eq!(
            store.things_tasks(&ThingsViewKind::Logbook, today)?.into_iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![done_proj.id]
        );

        let counts = store.things_counts(today)?;
        assert_eq!(counts, ThingsCounts {
            inbox: 1,
            today: 3,
            today_evening: 1,
            upcoming: 1,
            anytime: 4,
            someday: 1,
            logbook: 1,
        });

        let prog = store.project_progress(proj.id)?;
        assert_eq!(prog, ProjectProgress { list_id: proj.id, completed: 1, total: 2 });
        assert_eq!(store.all_project_progress()?.get(&proj.id).copied(), Some(prog));

        store.set_list_project_fields(proj.id, "Updated notes", None, Some(5000))?;
        let updated = store.lists()?.into_iter().find(|l| l.id == proj.id).unwrap();
        assert_eq!(updated.notes, "Updated notes");
        assert_eq!(updated.deadline, None);
        assert_eq!(updated.completed_at, Some(5000));
        Ok(())
    }
}
