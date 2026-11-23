use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::model::{Id, Kind, Priority, ScheduleBucket, Task};

use super::{MAX_SUBTASK_DEPTH, SortTable, Store, now_ms, parse_uuid, task_from_row};

const SELECT_SUBTASKS_SQL: &str = concat!(
    "SELECT ",
    task_columns!(),
    " FROM tasks WHERE parent_id = ?1 AND deleted_at IS NULL ",
    "ORDER BY completed_at IS NOT NULL, completed_at DESC, sort_order, rowid DESC"
);

fn list_section_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::model::ListSection> {
    Ok(crate::model::ListSection {
        id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
        list_id: row
            .get::<_, Option<String>>(1)?
            .and_then(|l| Uuid::parse_str(&l).ok()),
        name: row.get(2)?,
    })
}

impl Store {
    /// Moves a task right before (or after) `target` in the manual order, then renumbers the
    /// order with gaps. Renumbering everything keeps the code simple; personal task lists are
    /// small enough for that.
    pub fn move_task(&self, id: Id, target: Id, after: bool) -> Result<()> {
        if id == target {
            return Ok(());
        }
        self.in_tx(|| {
            let mut stmt = self.conn.prepare_cached("SELECT id FROM tasks ORDER BY sort_order, rowid DESC")?;
            let mut order: Vec<String> = stmt.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
            let (id, target) = (id.to_string(), target.to_string());
            order.retain(|other| *other != id);
            let index = order.iter().position(|other| *other == target).context("target task not found")?;
            order.insert(if after { index + 1 } else { index }, id);

            let mut update = self
                .conn
                .prepare_cached("UPDATE tasks SET sort_order = ?2 WHERE id = ?1 AND sort_order != ?2")?;
            for (position, task) in order.iter().enumerate() {
                update.execute(params![task, position as i64 * 1024])?;
            }
            Ok(())
        })
    }

    /// Direct subtasks of `parent` that are not in the trash, oldest first.
    pub fn subtasks(&self, parent: Id) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare_cached(SELECT_SUBTASKS_SQL)?;
        let mut subtasks: Vec<Task> = stmt
            .query_map([parent.to_string()], task_from_row)?
            .collect::<Result<_, _>>()?;
        subtasks.reverse();
        self.hydrate_tasks(&mut subtasks)?;
        Ok(subtasks)
    }

    /// Creates a subtask in the parent's list and section (and inherits closed state if the parent is closed).
    pub fn add_subtask(&self, parent: Id, title: &str) -> Result<Task> {
        self.in_tx(|| {
            let parent_task = self.task(parent)?.ok_or_else(|| anyhow::anyhow!("parent task not found"))?;
            anyhow::ensure!(!parent_task.is_deleted(), "trashed tasks cannot have subtasks");
            anyhow::ensure!(parent_task.kind != Kind::Note, "notes cannot have subtasks");
            anyhow::ensure!(self.task_depth(parent)? < MAX_SUBTASK_DEPTH, "maximum subtask nesting depth reached");
            let task = Task {
                id: Uuid::now_v7(),
                list_id: parent_task.list_id,
                parent_id: Some(parent),
                kind: Kind::Task,
                title: title.trim().to_owned(),
                notes: String::new(),
                priority: Priority::None,
                due_date: None,
                due_time: None,
                repeat: None,
                tags: Vec::new(),
                checklist: None,
                completed_at: parent_task.completed_at,
                deleted_at: None,
                wont_do_at: parent_task.wont_do_at,
                in_progress_at: None,
                end_date: None,
                end_time: None,
                location: None,
                timezone: None,
                is_floating: false,
                snoozed_until: None,
                constant_reminder: false,
                modified_at: None,
                pinned_at: None,
                section_id: parent_task.section_id,
                reminders: Vec::new(),
                estimate: None,
                start_date: None,
                schedule_bucket: ScheduleBucket::None,
                horizon: None,
                created_at: now_ms(),
            };
            self.conn.execute(
                "INSERT INTO tasks (id, parent_id, title, notes, list_id, section_id, kind, priority,
                                    completed_at, wont_do_at, created_at, sort_order)
                 VALUES (?1, ?2, ?3, '', ?4, ?5, 'task', 0, ?6, ?7, ?8,
                         (SELECT COALESCE(MIN(sort_order), 0) - 1024 FROM tasks))",
                params![
                    task.id.to_string(),
                    parent.to_string(),
                    task.title,
                    task.list_id.map(|l| l.to_string()),
                    task.section_id.map(|s| s.to_string()),
                    task.completed_at,
                    task.wont_do_at,
                    task.created_at,
                ],
            )?;
            Ok(task)
        })
    }

    /// Sections of a list ("Add Section"), oldest first.
    /// A list's sections, or the Inbox's for `None`.
    pub fn list_sections(&self, list: Option<Id>) -> Result<Vec<crate::model::ListSection>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, list_id, name FROM list_sections WHERE list_id IS ?1 ORDER BY sort_order, created_at, rowid",
        )?;
        let sections = stmt
            .query_map([list.map(|l| l.to_string())], list_section_from_row)?
            .collect::<Result<_, _>>()?;
        Ok(sections)
    }

    /// Loads all list sections across every list (and Inbox under `None`) in a single query,
    /// preserving each list's `sort_order, created_at, rowid` order.
    pub fn all_list_sections(&self) -> Result<HashMap<Option<Id>, Vec<crate::model::ListSection>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, list_id, name FROM list_sections ORDER BY sort_order, created_at, rowid",
        )?;
        let mut by_list: HashMap<Option<Id>, Vec<crate::model::ListSection>> = HashMap::new();
        for section in stmt.query_map([], list_section_from_row)? {
            let section = section?;
            by_list.entry(section.list_id).or_default().push(section);
        }
        Ok(by_list)
    }

    pub fn add_list_section(&self, list: Option<Id>, name: &str) -> Result<crate::model::ListSection> {
        self.insert_list_section(list, name, None)
    }

    /// Adds a section at `index` among the list's sections (last for `None`), as the kanban's
    /// "Add Section to Left / Right".
    pub fn insert_list_section(&self, list: Option<Id>, name: &str, index: Option<usize>) -> Result<crate::model::ListSection> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        let section = crate::model::ListSection { id: Uuid::now_v7(), list_id: list, name: name.to_owned() };
        self.in_tx(|| {
            let mut order: Vec<Id> = self.list_sections(list)?.into_iter().map(|s| s.id).collect();
            let at = index.unwrap_or(order.len()).min(order.len());
            order.insert(at, section.id);
            self.conn.execute(
                "INSERT INTO list_sections (id, list_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![section.id.to_string(), list.map(|l| l.to_string()), section.name, now_ms()],
            )?;
            self.write_sort_order(SortTable::ListSections, &order)?;
            Ok(())
        })?;
        Ok(section)
    }

    /// The kanban column menu's "Move to": the section and its tasks go to another list, after
    /// its sections.
    pub fn move_list_section(&self, id: Id, to: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id FROM tasks WHERE section_id = ?1 AND parent_id IS NULL")?;
            let tasks: Vec<Id> = stmt
                .query_map([id.to_string()], |row| {
                    let raw: String = row.get(0)?;
                    parse_uuid(&raw, 0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let order: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM list_sections WHERE list_id IS ?1",
                [to.map(|l| l.to_string())],
                |row| row.get(0),
            )?;
            self.conn.execute(
                "UPDATE list_sections SET list_id = ?2, sort_order = ?3 WHERE id = ?1",
                params![id.to_string(), to.map(|l| l.to_string()), order],
            )?;
            for task in tasks {
                self.set_list(task, to)?;
                self.set_section(task, Some(id))?;
            }
            Ok(())
        })
    }

    pub fn rename_list_section(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a section needs a name");
        self.conn.execute("UPDATE list_sections SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Reorders a section among its list's sections by moving `dragged` to `target`'s position.
    pub fn reorder_list_sections(&self, list: Option<Id>, dragged: Id, target: Id) -> Result<()> {
        if dragged == target {
            return Ok(());
        }
        self.in_tx(|| {
            let mut order: Vec<Id> = self.list_sections(list)?.into_iter().map(|s| s.id).collect();
            let Some(from) = order.iter().position(|&id| id == dragged) else { return Ok(()) };
            let Some(to) = order.iter().position(|&id| id == target) else { return Ok(()) };
            let id = order.remove(from);
            order.insert(to, id);
            self.write_sort_order(SortTable::ListSections, &order)
        })
    }

    /// Deletes a section; its tasks stay in the list without a section.
    pub fn delete_list_section(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("UPDATE tasks SET section_id = NULL WHERE section_id = ?1", [id.to_string()])?;
            self.conn.execute("DELETE FROM list_sections WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Puts a task (and its subtasks) into a section of its list, or out of any.
    pub fn set_section(&self, id: Id, section: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let task = self.task(id)?.context("task not found")?;
            if let Some(section) = section {
                let list: Option<Option<String>> = self
                    .conn
                    .query_row("SELECT list_id FROM list_sections WHERE id = ?1", [section.to_string()], |row| row.get(0))
                    .optional()?;
                let task_list = task.list_id.map(|l| l.to_string());
                anyhow::ensure!(list == Some(task_list), "the section belongs to another list");
            }
            let section_str = section.map(|s| s.to_string());
            let unlink = match task.parent_id {
                Some(parent_id) => {
                    let parent_section: Option<Option<String>> = self
                        .conn
                        .prepare_cached("SELECT section_id FROM tasks WHERE id = ?1")?
                        .query_row([parent_id.to_string()], |row| row.get(0))
                        .optional()?;
                    parent_section.is_none_or(|ps| ps != section_str)
                }
                None => false,
            };
            if unlink {
                self.conn.execute("UPDATE tasks SET parent_id = NULL WHERE id = ?1", [id.to_string()])?;
            }
            let mut stmt = self.conn.prepare_cached("UPDATE tasks SET section_id = ?2 WHERE id = ?1")?;
            for sub in std::iter::once(id).chain(self.descendants(id)?) {
                stmt.execute(params![sub.to_string(), section_str])?;
            }
            Ok(())
        })
    }

    /// Ancestor IDs of `id`, from its immediate parent up to the root (cycle-safe).
    pub fn ancestors(&self, id: Id) -> Result<Vec<Id>> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([id]);
        let mut current = id;
        while let Some(parent_str) = self
            .conn
            .prepare_cached("SELECT parent_id FROM tasks WHERE id = ?1")?
            .query_row([current.to_string()], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
        {
            let Ok(parent) = Uuid::parse_str(&parent_str) else { break };
            if !seen.insert(parent) {
                break;
            }
            out.push(parent);
            current = parent;
        }
        Ok(out)
    }

    /// Nesting depth of `id` (`0` for a top-level task, up to `MAX_SUBTASK_DEPTH`).
    pub fn task_depth(&self, id: Id) -> Result<usize> {
        Ok(self.ancestors(id)?.len())
    }

    /// Removes `id`'s parent link, promoting it (with its own subtasks) to a top-level task.
    pub fn unlink_parent(&self, id: Id) -> Result<()> {
        self.set_parent(id, None)
    }

    /// "Link Parent Task": makes a task a subtask of `parent`, moving it (and its descendants,
    /// including any flattened onto `parent` at `MAX_SUBTASK_DEPTH`) to the parent's list and
    /// section and clamping any descendants that exceed `MAX_SUBTASK_DEPTH`.
    pub fn link_parent(&self, id: Id, parent: Id) -> Result<()> {
        self.in_tx(|| {
            let parent_task = self.task(parent)?.context("parent task not found")?;
            // Capture the full subtree BEFORE `set_parent` flattens deep descendants onto `parent`.
            let subtree: Vec<Id> = std::iter::once(id).chain(self.descendants(id)?).collect();
            self.set_parent(id, Some(parent))?;
            let list = parent_task.list_id.map(|l| l.to_string());
            let section = parent_task.section_id.map(|s| s.to_string());
            let mut move_stmt = self
                .conn
                .prepare_cached("UPDATE tasks SET list_id = ?2, section_id = ?3 WHERE id = ?1")?;
            for task_id in &subtree {
                move_stmt.execute(params![task_id.to_string(), list, section])?;
            }
            if parent_task.is_completed() {
                let now = parent_task.completed_at.unwrap_or_else(now_ms);
                let mut close_stmt = self.conn.prepare_cached(
                    "UPDATE tasks SET completed_at = COALESCE(completed_at, ?2) WHERE id = ?1 AND wont_do_at IS NULL AND deleted_at IS NULL",
                )?;
                for task_id in &subtree {
                    close_stmt.execute(params![task_id.to_string(), now])?;
                }
            } else if parent_task.is_wont_do() {
                let now = parent_task.wont_do_at.unwrap_or_else(now_ms);
                let mut close_stmt = self.conn.prepare_cached(
                    "UPDATE tasks SET wont_do_at = COALESCE(wont_do_at, ?2) WHERE id = ?1 AND completed_at IS NULL AND deleted_at IS NULL",
                )?;
                for task_id in &subtree {
                    close_stmt.execute(params![task_id.to_string(), now])?;
                }
            }
            Ok(())
        })
    }

    /// Makes the task a subtask of `parent`, or a top-level task for `None`.
    /// Notes cannot be nested or hold subtasks, and descendants beyond `MAX_SUBTASK_DEPTH`
    /// are flattened onto level `MAX_SUBTASK_DEPTH`.
    pub fn set_parent(&self, id: Id, parent: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            if let Some(parent) = parent {
                let task = self.task(id)?.context("task not found")?;
                let parent_task = self.task(parent)?.context("parent task not found")?;
                anyhow::ensure!(!parent_task.is_deleted(), "trashed tasks cannot be parents");
                anyhow::ensure!(task.kind != Kind::Note && parent_task.kind != Kind::Note, "notes cannot be nested");
                let ancestors = self.ancestors(parent)?;
                anyhow::ensure!(
                    parent != id && !ancestors.contains(&id),
                    "a task cannot be nested inside itself"
                );
                let parent_depth = ancestors.len();
                anyhow::ensure!(parent_depth < MAX_SUBTASK_DEPTH, "maximum subtask nesting depth reached");
                self.update(id, "parent_id", Some(parent.to_string()))?;
                self.flatten_subtask_depth(id, parent_depth + 1, parent)?;
                return Ok(());
            }
            self.update(id, "parent_id", None::<String>)
        })
    }

    /// Flattens any descendants of `root` (currently at `root_depth`, child of `root_parent`)
    /// whose depth would exceed `MAX_SUBTASK_DEPTH` onto `MAX_SUBTASK_DEPTH`.
    fn flatten_subtask_depth(&self, root: Id, root_depth: usize, root_parent: Id) -> Result<()> {
        let mut stack: Vec<(Id, usize, Option<Id>)> = vec![(
            root,
            root_depth,
            if root_depth == MAX_SUBTASK_DEPTH {
                Some(root_parent)
            } else if root_depth == MAX_SUBTASK_DEPTH - 1 {
                Some(root)
            } else {
                None
            },
        )];
        let mut reparents: Vec<(Id, Id)> = Vec::new();
        let mut seen = HashSet::from([root]);
        while let Some((node, depth, cap_parent)) = stack.pop() {
            let mut stmt = self.conn.prepare_cached("SELECT id FROM tasks WHERE parent_id = ?1")?;
            let children: Vec<Id> = stmt
                .query_map([node.to_string()], |row| {
                    let raw: String = row.get(0)?;
                    parse_uuid(&raw, 0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for child in children {
                if !seen.insert(child) {
                    continue;
                }
                let child_depth = depth + 1;
                if child_depth > MAX_SUBTASK_DEPTH {
                    if let Some(target_parent) = cap_parent {
                        reparents.push((child, target_parent));
                    }
                    stack.push((child, child_depth, cap_parent));
                } else {
                    let next_cap = if child_depth == MAX_SUBTASK_DEPTH - 1 {
                        Some(child)
                    } else {
                        cap_parent
                    };
                    stack.push((child, child_depth, next_cap));
                }
            }
        }
        if !reparents.is_empty() {
            self.in_tx(|| {
                let mut stmt = self.conn.prepare_cached("UPDATE tasks SET parent_id = ?2 WHERE id = ?1")?;
                for (child, target_parent) in reparents {
                    stmt.execute(params![child.to_string(), target_parent.to_string()])?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Moves `id` to `new_parent` (nesting or unnesting) and optionally positions it relative to
    /// `target` (`(target_id, after)`). When nesting directly after `new_parent`, places `id` as
    /// the first child of `new_parent`.
    pub fn move_task_in_tree(&self, id: Id, new_parent: Option<Id>, target: Option<(Id, bool)>) -> Result<()> {
        self.in_tx(|| {
            match new_parent {
                Some(parent) => self.link_parent(id, parent)?,
                None => self.set_parent(id, None)?,
            }
            if let Some((target_id, after)) = target {
                if new_parent == Some(target_id) && after {
                    let first_child: Option<Id> = self
                        .conn
                        .prepare_cached("SELECT id FROM tasks WHERE parent_id = ?1 AND id != ?2 AND deleted_at IS NULL ORDER BY sort_order, rowid DESC LIMIT 1")?
                        .query_row(params![target_id.to_string(), id.to_string()], |row| row.get::<_, String>(0))
                        .optional()?
                        .and_then(|s| Uuid::parse_str(&s).ok());
                    if let Some(first_child) = first_child {
                        self.move_task(id, first_child, false)?;
                    } else if target_id != id {
                        self.move_task(id, target_id, true)?;
                    }
                } else if target_id != id {
                    self.move_task(id, target_id, after)?;
                }
            }
            Ok(())
        })
    }

    /// Subtasks of a task, at any depth (cycle-safe via `UNION` deduplication).
    pub fn descendants(&self, id: Id) -> Result<Vec<Id>> {
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE sub(id) AS (
                 SELECT id FROM tasks WHERE parent_id = ?1
                 UNION
                 SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
             )
             SELECT id FROM sub",
        )?;
        let ids = stmt
            .query_map([id.to_string()], |row| {
                let raw: String = row.get(0)?;
                parse_uuid(&raw, 0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }
}
