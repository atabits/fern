use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::model::{ChecklistItem, Id, Kind, List, ListDraft, ListFolder, Tag};

use super::{DATE_FORMAT, FolderDropTarget, SortTable, Store, now_ms, parse_uuid};

const TAG_ORDER_KEY: &str = "tag_order";

impl Store {
    /// Replaces the task's tags; unknown tags are created.
    /// Tags match case-insensitively in Rust, because SQLite's `NOCASE` only folds ASCII.
    pub fn set_tags(&self, id: Id, tags: &[String]) -> Result<()> {
        self.in_tx(|| self.replace_task_tags(id, tags))
    }

    pub(super) fn sanitize_tag_name(raw: &str) -> String {
        raw.chars().filter(|c| !c.is_ascii_control()).collect::<String>().trim().to_owned()
    }

    pub(super) fn replace_task_tags(&self, id: Id, tags: &[String]) -> Result<()> {
        let mut existing: Vec<String> = self
            .conn
            .prepare_cached("SELECT name FROM tags")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        let id = id.to_string();
        self.conn.execute("DELETE FROM task_tags WHERE task_id = ?1", [&id])?;
        for tag in tags.iter().map(|t| Self::sanitize_tag_name(t)).filter(|t| !t.is_empty()) {
            let tag = tag.as_str();
            let name = match existing.iter().find(|e| e.to_lowercase() == tag.to_lowercase()) {
                Some(name) => name.clone(),
                None => {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)",
                        params![tag, now_ms()],
                    )?;
                    existing.push(tag.to_owned());
                    tag.to_owned()
                }
            };
            self.conn.execute(
                "INSERT OR IGNORE INTO task_tags (task_id, tag) VALUES (?1, ?2)",
                params![id, name],
            )?;
        }
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.name, t.color, COUNT(tk.id), t.parent, t.pinned_at
             FROM tags t
             LEFT JOIN task_tags tt ON tt.tag = t.name
             LEFT JOIN tasks tk ON tk.id = tt.task_id AND tk.completed_at IS NULL
                                AND tk.deleted_at IS NULL
             GROUP BY t.name
             ORDER BY t.created_at, t.rowid",
        )?;
        let mut tags: Vec<Tag> = stmt
            .query_map([], |row| {
                Ok(Tag {
                    name: row.get(0)?,
                    color: row.get(1)?,
                    open_count: row.get::<_, i64>(2)? as usize,
                    parent: row.get(3)?,
                    pinned: row.get::<_, Option<i64>>(4)?.is_some(),
                })
            })?
            .collect::<Result<_, _>>()?;
        if let Some(saved) = self.setting(TAG_ORDER_KEY)? {
            let mut ranks: HashMap<&str, usize> = HashMap::new();
            for (i, name) in saved.lines().filter(|s| !s.is_empty()).enumerate() {
                ranks.entry(name).or_insert(i);
            }
            tags.sort_by_key(|t| ranks.get(t.name.as_str()).copied().unwrap_or(usize::MAX));
        }
        Ok(tags)
    }

    pub fn set_tag_color(&self, name: &str, color: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE tags SET color = ?2 WHERE name = ?1", params![name, color])?;
        Ok(())
    }

    /// Checks a parent for `tag`: one level deep, as in the original — the parent is a top-level
    /// tag other than the tag itself, and a tag with sub-tags cannot become one.
    fn check_parent(&self, tag: Option<&str>, parent: Option<&str>) -> Result<()> {
        let Some(parent) = parent else { return Ok(()) };
        let tags = self.tags()?;
        let found = tags.iter().find(|t| t.name == parent).context("no such parent tag")?;
        anyhow::ensure!(found.parent.is_none(), "a sub-tag cannot have sub-tags");
        if let Some(tag) = tag {
            anyhow::ensure!(tag != parent, "a tag cannot be its own parent");
            anyhow::ensure!(!tags.iter().any(|t| t.parent.as_deref() == Some(tag)), "a tag with sub-tags cannot become a sub-tag");
        }
        Ok(())
    }

    /// The original's Add Tags: a new tag with its color and parent.
    pub fn add_tag(&self, name: &str, color: Option<&str>, parent: Option<&str>) -> Result<()> {
        let name = name.trim().trim_start_matches('#').trim();
        anyhow::ensure!(!name.is_empty(), "a tag needs a name");
        anyhow::ensure!(!name.chars().any(|c| c.is_ascii_control()), "tag name cannot contain control characters");
        anyhow::ensure!(!self.tags()?.iter().any(|t| t.name.to_lowercase() == name.to_lowercase()), "the tag already exists");
        self.check_parent(None, parent)?;
        self.conn.execute(
            "INSERT INTO tags (name, color, parent, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, color, parent, now_ms()],
        )?;
        Ok(())
    }

    /// The original's Edit Tag: name, color and parent. A new name is a new row (the name is the
    /// key, also for sync), so the tag's tasks and sub-tags move over to it.
    pub fn update_tag(&self, name: &str, new_name: &str, color: Option<&str>, parent: Option<&str>) -> Result<()> {
        let new_name = new_name.trim().trim_start_matches('#').trim();
        anyhow::ensure!(!new_name.is_empty(), "a tag needs a name");
        anyhow::ensure!(!new_name.chars().any(|c| c.is_ascii_control()), "tag name cannot contain control characters");
        self.check_parent(Some(name), parent)?;
        self.in_tx(|| {
            if new_name != name {
                anyhow::ensure!(
                    !self.tags()?.iter().any(|t| t.name != name && t.name.to_lowercase() == new_name.to_lowercase()),
                    "the tag already exists"
                );
                self.conn.execute(
                    "INSERT INTO tags (name, color, parent, pinned_at, created_at) SELECT ?2, color, parent, pinned_at, created_at FROM tags WHERE name = ?1",
                    params![name, new_name],
                )?;
                self.conn.execute("INSERT OR IGNORE INTO task_tags (task_id, tag) SELECT task_id, ?2 FROM task_tags WHERE tag = ?1", params![name, new_name])?;
                self.conn.execute("DELETE FROM task_tags WHERE tag = ?1", [name])?;
                self.conn.execute("UPDATE tags SET parent = ?2 WHERE parent = ?1", params![name, new_name])?;
                self.conn.execute("DELETE FROM tags WHERE name = ?1", [name])?;
                if let Some(saved) = self.setting(TAG_ORDER_KEY)? {
                    let updated: Vec<&str> = saved.lines().map(|n| if n == name { new_name } else { n }).collect();
                    self.set_setting(TAG_ORDER_KEY, &updated.join("\n"))?;
                }
                self.update_matrix_rules_for_tag_rename(name, new_name)?;
            }
            self.conn.execute("UPDATE tags SET color = ?2, parent = ?3 WHERE name = ?1", params![new_name, color, parent])?;
            Ok(())
        })
    }

    pub fn set_tag_pinned(&self, name: &str, pinned: bool) -> Result<()> {
        self.conn.execute("UPDATE tags SET pinned_at = ?2 WHERE name = ?1", params![name, pinned.then(now_ms)])?;
        Ok(())
    }

    /// Reorders or nests `dragged` tag relative to `target` tag, matching TickTick's tag drag rules:
    /// - `nest = true`: makes `dragged` a sub-tag of `target` (1 level deep: ignored if `dragged`
    ///   has sub-tags or `target` is itself a sub-tag).
    /// - `nest = false`: moves `dragged` (together with its sub-tags if it is a parent) before
    ///   (`!after`) or after (`after`) `target`. A standalone/sub-tag adopts `target`'s parent, so
    ///   dropping a sub-tag onto a top-level tag's edge promotes it to top-level.
    pub fn move_tag(&self, dragged: &str, target: &str, after: bool, nest: bool) -> Result<()> {
        if dragged == target {
            return Ok(());
        }
        self.in_tx(|| {
            let tags = self.tags()?;
            let Some(dragged_tag) = tags.iter().find(|t| t.name == dragged) else { return Ok(()) };
            let Some(target_tag) = tags.iter().find(|t| t.name == target) else { return Ok(()) };

            let dragged_children: Vec<String> =
                tags.iter().filter(|t| t.parent.as_deref() == Some(dragged)).map(|t| t.name.clone()).collect();
            let dragged_is_parent = !dragged_children.is_empty();
            let target_parent = target_tag.parent.clone();

            if nest {
                if dragged_is_parent || target_parent.is_some() {
                    return Ok(());
                }
                let mut remaining: Vec<String> = tags.iter().filter(|t| t.name != dragged).map(|t| t.name.clone()).collect();
                let last_child_pos = remaining.iter().rposition(|name| {
                    tags.iter().any(|t| t.name == *name && t.parent.as_deref() == Some(target))
                });
                let insert_at = match last_child_pos {
                    Some(idx) => idx + 1,
                    None => remaining.iter().position(|name| name == target).map_or(remaining.len(), |idx| idx + 1),
                };
                remaining.insert(insert_at, dragged.to_owned());

                self.conn.execute(
                    "UPDATE tags SET parent = ?2, pinned_at = NULL WHERE name = ?1",
                    params![dragged, target],
                )?;
                self.set_setting(TAG_ORDER_KEY, &remaining.join("\n"))?;
                return Ok(());
            }

            if dragged_is_parent && target_parent.is_some() {
                return Ok(());
            }

            let mut block = Vec::with_capacity(1 + dragged_children.len());
            block.push(dragged.to_owned());
            block.extend(dragged_children);

            let mut remaining: Vec<String> =
                tags.iter().filter(|t| !block.contains(&t.name)).map(|t| t.name.clone()).collect();
            let Some(target_pos) = remaining.iter().position(|name| name == target) else {
                return Ok(());
            };
            let insert_at = if after && target_parent.is_none() {
                remaining
                    .iter()
                    .rposition(|name| tags.iter().any(|t| t.name == *name && t.parent.as_deref() == Some(target)))
                    .map_or(target_pos + 1, |idx| idx + 1)
            } else {
                target_pos + usize::from(after)
            };
            remaining.splice(insert_at..insert_at, block);

            if !dragged_is_parent && dragged_tag.parent != target_parent {
                self.conn.execute(
                    "UPDATE tags SET parent = ?2 WHERE name = ?1",
                    params![dragged, target_parent.as_deref()],
                )?;
            }
            if target_parent.is_some() {
                self.conn.execute("UPDATE tags SET pinned_at = NULL WHERE name = ?1", [dragged])?;
            } else if dragged_tag.pinned != target_tag.pinned {
                self.conn.execute(
                    "UPDATE tags SET pinned_at = ?2 WHERE name = ?1",
                    params![dragged, target_tag.pinned.then(now_ms)],
                )?;
            }
            self.set_setting(TAG_ORDER_KEY, &remaining.join("\n"))?;
            Ok(())
        })
    }

    /// Moves a tag (and its children if any) to the top of the Tags section, out of any parent.
    pub fn move_tag_first(&self, dragged: &str) -> Result<()> {
        self.in_tx(|| {
            let tags = self.tags()?;
            if !tags.iter().any(|t| t.name == dragged) {
                return Ok(());
            }
            let mut block = vec![dragged.to_owned()];
            block.extend(tags.iter().filter(|t| t.parent.as_deref() == Some(dragged)).map(|t| t.name.clone()));
            let mut remaining: Vec<String> =
                tags.iter().filter(|t| !block.contains(&t.name)).map(|t| t.name.clone()).collect();
            remaining.splice(0..0, block);

            self.conn.execute("UPDATE tags SET parent = NULL WHERE name = ?1", [dragged])?;
            self.set_setting(TAG_ORDER_KEY, &remaining.join("\n"))?;
            Ok(())
        })
    }

    /// Commits a live-reordered tag slice (hierarchy, pinned state, and display order) atomically.
    pub fn save_tag_hierarchy_and_order(&self, tags: &[Tag]) -> Result<()> {
        self.in_tx(|| {
            let existing = self.tags()?;
            let by_name: HashMap<&str, &Tag> = existing.iter().map(|t| (t.name.as_str(), t)).collect();
            let valid_roots: HashSet<&str> = tags
                .iter()
                .filter(|t| t.parent.is_none() && by_name.contains_key(t.name.as_str()))
                .map(|t| t.name.as_str())
                .collect();

            for tag in tags {
                let Some(prev) = by_name.get(tag.name.as_str()) else { continue };
                let parent = tag
                    .parent
                    .as_deref()
                    .filter(|&p| p != tag.name.as_str() && valid_roots.contains(p));
                let pinned = parent.is_none() && tag.pinned;
                if prev.parent.as_deref() != parent {
                    self.conn.execute(
                        "UPDATE tags SET parent = ?2 WHERE name = ?1",
                        params![tag.name, parent],
                    )?;
                }
                if prev.pinned != pinned {
                    self.conn.execute(
                        "UPDATE tags SET pinned_at = ?2 WHERE name = ?1",
                        params![tag.name, pinned.then(now_ms)],
                    )?;
                }
            }

            let order: Vec<&str> = tags
                .iter()
                .filter(|t| by_name.contains_key(t.name.as_str()))
                .map(|t| t.name.as_str())
                .collect();
            self.set_setting(TAG_ORDER_KEY, &order.join("\n"))?;
            Ok(())
        })
    }

    fn prune_tag_order(&self, removed: &str) -> Result<()> {
        if let Some(saved) = self.setting(TAG_ORDER_KEY)? {
            let updated: Vec<&str> = saved.lines().filter(|s| !s.is_empty() && *s != removed).collect();
            self.set_setting(TAG_ORDER_KEY, &updated.join("\n"))?;
        }
        Ok(())
    }

    /// The original's Merge Tags: `from`'s tasks get `into` instead, and `from` is deleted; its
    /// sub-tags go under `into` (or to the top when `into` is itself a sub-tag).
    pub fn merge_tag(&self, from: &str, into: &str) -> Result<()> {
        anyhow::ensure!(from != into, "a tag cannot be merged into itself");
        self.in_tx(|| {
            let tags = self.tags()?;
            let target = tags.iter().find(|t| t.name == into).context("no such tag")?;
            let new_parent = if target.parent.is_none() { Some(into) } else { None };
            self.conn.execute("INSERT OR IGNORE INTO task_tags (task_id, tag) SELECT task_id, ?2 FROM task_tags WHERE tag = ?1", params![from, into])?;
            self.conn.execute("DELETE FROM task_tags WHERE tag = ?1", [from])?;
            self.conn.execute("UPDATE tags SET parent = ?2 WHERE parent = ?1 AND name != ?3", params![from, new_parent, into])?;
            self.conn.execute("DELETE FROM tags WHERE name = ?1", [from])?;
            self.prune_tag_order(from)?;
            self.update_matrix_rules_for_tag_rename(from, into)?;
            Ok(())
        })
    }

    /// Deletes a tag: its tasks keep everything else, its sub-tags move to the top.
    pub fn delete_tag(&self, name: &str) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("DELETE FROM task_tags WHERE tag = ?1", [name])?;
            self.conn.execute("UPDATE tags SET parent = NULL WHERE parent = ?1", [name])?;
            self.conn.execute("DELETE FROM tags WHERE name = ?1", [name])?;
            self.prune_tag_order(name)?;
            self.prune_matrix_rules_for_tag_delete(name)?;
            Ok(())
        })
    }

    /// A tag and all its descendant sub-tags (transitively, cycle-safe), lowercased: a parent tag's
    /// view shows its sub-tags' tasks too.
    pub(super) fn tag_family(&self, tag: &str) -> Result<HashSet<String>> {
        let root = tag.to_lowercase();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for t in self.tags()? {
            if let Some(parent) = t.parent {
                children.entry(parent.to_lowercase()).or_default().push(t.name.to_lowercase());
            }
        }
        let mut family = HashSet::from([root.clone()]);
        let mut stack = vec![root];
        while let Some(cur) = stack.pop() {
            if let Some(kids) = children.get(&cur) {
                for kid in kids {
                    if family.insert(kid.clone()) {
                        stack.push(kid.clone());
                    }
                }
            }
        }
        Ok(family)
    }

    /// The lists in their order, archived ones included (see `List::archived`).
    pub fn lists(&self) -> Result<Vec<List>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT l.id, l.name, l.color, COUNT(t.id), l.icon, l.folder_id, l.kind, l.in_smart_lists, l.archived_at,
                    l.notes, l.deadline, l.completed_at
             FROM lists l
             LEFT JOIN tasks t ON t.list_id = l.id AND t.completed_at IS NULL
                               AND t.deleted_at IS NULL
             GROUP BY l.id
             ORDER BY l.sort_order, l.created_at, l.rowid",
        )?;
        let lists = stmt
            .query_map([], |row| {
                Ok(List {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    open_count: row.get::<_, i64>(3)? as usize,
                    icon: row.get(4)?,
                    folder: row.get::<_, Option<String>>(5)?.and_then(|f| Uuid::parse_str(&f).ok()),
                    kind: Kind::parse(&row.get::<_, String>(6)?),
                    in_smart_lists: row.get(7)?,
                    archived: row.get::<_, Option<i64>>(8)?.is_some(),
                    notes: row.get(9)?,
                    deadline: row.get::<_, Option<String>>(10)?
                        .and_then(|d| NaiveDate::parse_from_str(&d, DATE_FORMAT).ok()),
                    completed_at: row.get(11)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(lists)
    }

    pub fn create_list(&self, name: &str) -> Result<List> {
        self.add_list(&ListDraft { name: name.to_owned(), ..ListDraft::default() })
    }

    /// Adds a list as the Add List dialog describes it, after the others.
    pub fn add_list(&self, draft: &ListDraft) -> Result<List> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        let id = Uuid::now_v7();
        self.conn.execute(
            "INSERT INTO lists (id, name, created_at, sort_order) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM lists))",
            params![id.to_string(), name, now_ms()],
        )?;
        self.update_list(id, draft)?;
        self.lists()?.into_iter().find(|l| l.id == id).context("list not found")
    }

    /// Saves the Edit List dialog's choices.
    pub fn update_list(&self, id: Id, draft: &ListDraft) -> Result<()> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        self.conn.execute(
            "UPDATE lists SET name = ?2, icon = ?3, color = ?4, folder_id = ?5, kind = ?6, in_smart_lists = ?7,
                              notes = ?8, deadline = ?9, completed_at = ?10 WHERE id = ?1",
            params![
                id.to_string(),
                name,
                draft.icon.as_deref().filter(|i| !i.is_empty()),
                draft.color,
                draft.folder.map(|f| f.to_string()),
                draft.kind.as_str(),
                draft.in_smart_lists,
                draft.notes,
                draft.deadline.map(|d| d.format(DATE_FORMAT).to_string()),
                draft.completed_at,
            ],
        )?;
        Ok(())
    }

    /// Archives a list or brings it back.
    pub fn set_list_archived(&self, id: Id, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE lists SET archived_at = ?2 WHERE id = ?1",
            params![id.to_string(), archived.then(now_ms)],
        )?;
        Ok(())
    }

    /// Returns a list's display name and whether it is archived.
    pub fn list_name_and_archived(&self, id: Id) -> Result<Option<(String, bool)>> {
        self.conn
            .prepare_cached("SELECT name, archived_at IS NOT NULL FROM lists WHERE id = ?1")?
            .query_row([id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(Into::into)
    }

    /// Whether a list is archived (and therefore its tasks are read-only, as in the original).
    pub fn is_list_archived(&self, id: Id) -> Result<bool> {
        Ok(self.list_name_and_archived(id)?.is_some_and(|(_, archived)| archived))
    }

    /// Whether a task is read-only: either in the trash or inside an archived list.
    pub fn is_task_read_only(&self, id: Id) -> Result<bool> {
        let row: Option<(bool, bool)> = self
            .conn
            .prepare_cached(
                "SELECT t.deleted_at IS NOT NULL, COALESCE(l.archived_at IS NOT NULL, 0)
                 FROM tasks t
                 LEFT JOIN lists l ON l.id = t.list_id
                 WHERE t.id = ?1",
            )?
            .query_row([id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        Ok(row.is_some_and(|(deleted, archived)| deleted || archived))
    }

    /// The original's Duplicate: a copy of the list, " copy" after its name, with copies of its
    /// open tasks (subtasks, tags and checklists included), right after it.
    pub fn duplicate_list(&self, id: Id, suffix: &str) -> Result<List> {
        self.in_tx(|| {
            let list = self.lists()?.into_iter().find(|l| l.id == id).context("list not found")?;
            let draft = ListDraft { name: format!("{}{suffix}", list.name), ..ListDraft::of(&list) };
            let copy = self.add_list(&draft)?;
            self.place_list(copy.id, Some(id), list.folder)?;
            for section in self.list_sections(Some(id))? {
                self.add_list_section(Some(copy.id), &section.name)?;
            }
            let root_ids: Vec<String> = self
                .conn
                .prepare_cached(
                    "SELECT id FROM tasks
                     WHERE list_id = ?1 AND parent_id IS NULL AND completed_at IS NULL AND deleted_at IS NULL
                     ORDER BY sort_order, rowid DESC",
                )?
                .query_map([id.to_string()], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            if !root_ids.is_empty() {
                let copy_list_id = copy.id.to_string();
                let base_sort: i64 = self.conn.query_row(
                    "SELECT COALESCE(MAX(sort_order), 0) + 1024 FROM tasks",
                    [],
                    |row| row.get(0),
                )?;
                let mut insert_task = self.conn.prepare_cached(
                    "INSERT INTO tasks (
                         id, parent_id, kind, title, notes, priority, due_date, due_time,
                         list_id, section_id, repeat, reminders, end_date, end_time,
                         estimate, location, timezone, is_floating, constant_reminder,
                         start_date, schedule_bucket, horizon, created_at, sort_order
                     )
                     SELECT
                         ?1, parent_id, kind, title, notes, priority, due_date, due_time,
                         ?3, NULL, repeat, reminders, end_date, end_time,
                         estimate, location, timezone, is_floating, constant_reminder,
                         start_date, schedule_bucket, horizon, ?4, ?5
                     FROM tasks WHERE id = ?2",
                )?;
                let mut copy_tags = self.conn.prepare_cached(
                    "INSERT INTO task_tags (task_id, tag) SELECT ?1, tag FROM task_tags WHERE task_id = ?2",
                )?;
                let mut read_checklist = self.conn.prepare_cached(
                    "SELECT title, completed_at IS NOT NULL FROM checklist_items WHERE task_id = ?1 ORDER BY sort_order, rowid",
                )?;
                let mut insert_checklist = self.conn.prepare_cached(
                    "INSERT INTO checklist_items (id, task_id, title, sort_order, completed_at, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                let now = now_ms();
                for (idx, src_id) in root_ids.iter().enumerate() {
                    let dst_id = Uuid::now_v7().to_string();
                    let sort_order = base_sort + (idx as i64) * 1024;
                    insert_task.execute(params![&dst_id, src_id, &copy_list_id, now, sort_order])?;
                    copy_tags.execute(params![&dst_id, src_id])?;
                    let items: Vec<(String, bool)> = read_checklist
                        .query_map([src_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<Result<_, _>>()?;
                    for (pos, (title, completed)) in items.into_iter().enumerate() {
                        insert_checklist.execute(params![
                            Uuid::now_v7().to_string(),
                            &dst_id,
                            title,
                            pos as i64,
                            completed.then_some(now),
                            now,
                        ])?;
                    }
                }
            }
            self.lists()?.into_iter().find(|l| l.id == copy.id).context("list not found")
        })
    }

    /// Moves a list right after `after` (first when `None`) and into `folder`.
    pub fn place_list(&self, id: Id, after: Option<Id>, folder: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let lists = self.lists()?;
            if !lists.iter().any(|l| l.id == id) {
                return Ok(());
            }
            if let Some(fid) = folder {
                anyhow::ensure!(
                    self.list_folders()?.iter().any(|f| f.id == fid),
                    "target folder not found"
                );
            }
            let mut order: Vec<Id> = lists.into_iter().map(|l| l.id).filter(|l| *l != id).collect();
            let at = after.and_then(|a| order.iter().position(|l| *l == a)).map_or(0, |i| i + 1);
            order.insert(at, id);
            self.write_sort_order(SortTable::Lists, &order)?;
            self.conn.execute(
                "UPDATE lists SET folder_id = ?2 WHERE id = ?1",
                params![id.to_string(), folder.map(|f| f.to_string())],
            )?;
            Ok(())
        })
    }

    /// Groups active lists into visual order so each folder's active lists sit contiguously at
    /// the position of its first active list, followed by archived lists in their existing order.
    fn visual_list_order(lists: &[List], folders: &[ListFolder]) -> Vec<Id> {
        let mut result = Vec::with_capacity(lists.len());
        let mut shown_folders: HashSet<Id> = HashSet::new();
        for list in lists.iter().filter(|l| !l.archived) {
            match list.folder.filter(|f| folders.iter().any(|x| x.id == *f)) {
                None => result.push(list.id),
                Some(fid) if shown_folders.insert(fid) => {
                    result.extend(
                        lists.iter().filter(|l| !l.archived && l.folder == Some(fid)).map(|l| l.id),
                    );
                }
                Some(_) => {}
            }
        }
        result.extend(lists.iter().filter(|l| l.archived).map(|l| l.id));
        result
    }

    /// Moves a list right before (`!after`) or right after (`after`) `target`, setting its `folder`.
    pub fn place_list_relative(&self, id: Id, target: Id, after: bool, folder: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let lists = self.lists()?;
            if !lists.iter().any(|l| l.id == id) {
                return Ok(());
            }
            let folders = self.list_folders()?;
            if let Some(fid) = folder {
                anyhow::ensure!(folders.iter().any(|f| f.id == fid), "target folder not found");
            }
            if id == target {
                self.conn.execute(
                    "UPDATE lists SET folder_id = ?2 WHERE id = ?1",
                    params![id.to_string(), folder.map(|f| f.to_string())],
                )?;
                return Ok(());
            }
            let mut order: Vec<Id> =
                Self::visual_list_order(&lists, &folders).into_iter().filter(|l| *l != id).collect();
            let Some(pos) = order.iter().position(|l| *l == target) else {
                return self.place_list(id, None, folder);
            };
            order.insert(pos + usize::from(after), id);
            self.write_sort_order(SortTable::Lists, &order)?;
            self.conn.execute(
                "UPDATE lists SET folder_id = ?2 WHERE id = ?1",
                params![id.to_string(), folder.map(|f| f.to_string())],
            )?;
            Ok(())
        })
    }

    /// Moves a list out of any folder and places it right before (`!after`) or right after (`after`)
    /// `folder`'s block of active lists.
    pub fn place_list_around_folder(&self, id: Id, folder: Id, after: bool) -> Result<()> {
        self.in_tx(|| {
            let lists = self.lists()?;
            let Some(dragged_list) = lists.iter().find(|l| l.id == id) else {
                return Ok(());
            };
            let folders = self.list_folders()?;
            let was_in_folder = dragged_list.folder == Some(folder);
            let visual = Self::visual_list_order(&lists, &folders);
            let active_by_id: HashMap<Id, &List> = lists.iter().map(|l| (l.id, l)).collect();
            let mut remaining: Vec<Id> = visual.iter().copied().filter(|&lid| lid != id).collect();
            let first = remaining.iter().position(|lid| active_by_id.get(lid).is_some_and(|l| !l.archived && l.folder == Some(folder)));
            let last = remaining.iter().rposition(|lid| active_by_id.get(lid).is_some_and(|l| !l.archived && l.folder == Some(folder)));
            let insert_at = match (first, last) {
                (Some(f), Some(l)) => if !after { f } else { l + 1 },
                _ if was_in_folder => visual.iter().position(|&lid| lid == id).unwrap_or(remaining.len()).min(remaining.len()),
                _ => remaining
                    .iter()
                    .rposition(|lid| active_by_id.get(lid).is_some_and(|l| !l.archived))
                    .map_or(0, |p| p + 1),
            };
            remaining.insert(insert_at, id);
            self.write_sort_order(SortTable::Lists, &remaining)?;
            self.conn.execute("UPDATE lists SET folder_id = NULL WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Drops `dragged` list onto top-level `target` list, creating a new folder containing both
    /// at `target`'s position (matching TickTick's `createProjectGroup`).
    pub fn group_lists_into_folder(&self, dragged: Id, target: Id, folder_name: &str) -> Result<ListFolder> {
        anyhow::ensure!(dragged != target, "cannot group a list with itself");
        let name = folder_name.trim();
        anyhow::ensure!(!name.is_empty(), "a folder needs a name");
        self.in_tx(|| {
            let lists = self.lists()?;
            let folders = self.list_folders()?;
            let target_list = lists.iter().find(|l| l.id == target).context("target list not found")?;
            let dragged_list = lists.iter().find(|l| l.id == dragged).context("dragged list not found")?;
            anyhow::ensure!(!target_list.archived && !dragged_list.archived, "cannot group archived lists");
            anyhow::ensure!(target_list.folder.is_none(), "target list is already inside a folder");

            let mut order: Vec<Id> =
                Self::visual_list_order(&lists, &folders).into_iter().filter(|&id| id != dragged).collect();
            let target_pos = order.iter().position(|&id| id == target).context("target list not found")?;
            order.insert(target_pos + 1, dragged);

            let folder = ListFolder { id: Uuid::now_v7(), name: name.to_owned() };
            self.conn.execute(
                "INSERT INTO list_folders (id, name, created_at, sort_order) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM list_folders))",
                params![folder.id.to_string(), folder.name, now_ms()],
            )?;
            self.write_sort_order(SortTable::Lists, &order)?;
            let folder_id_str = folder.id.to_string();
            self.conn.execute(
                "UPDATE lists SET folder_id = ?1 WHERE id IN (?2, ?3)",
                params![folder_id_str, target.to_string(), dragged.to_string()],
            )?;
            Ok(folder)
        })
    }

    /// Reorders an entire folder (and all its active lists together) before (`!after`) or after
    /// (`after`) another top-level list or folder.
    pub fn reorder_list_folder(&self, dragged: Id, target: FolderDropTarget, after: bool) -> Result<()> {
        if target == FolderDropTarget::Folder(dragged) {
            return Ok(());
        }
        self.in_tx(|| {
            let lists = self.lists()?;
            let folders = self.list_folders()?;
            if !folders.iter().any(|f| f.id == dragged) {
                return Ok(());
            }

            let moving: Vec<Id> = lists.iter().filter(|l| !l.archived && l.folder == Some(dragged)).map(|l| l.id).collect();
            let remaining_lists: Vec<List> = lists.into_iter().filter(|l| l.archived || l.folder != Some(dragged)).collect();

            if let FolderDropTarget::List(target_list) = target {
                let Some(tl) = remaining_lists.iter().find(|l| l.id == target_list) else {
                    return Ok(());
                };
                if tl.archived || tl.folder.is_some() {
                    return Ok(());
                }
            }

            if !moving.is_empty() {
                let mut visual = Self::visual_list_order(&remaining_lists, &folders);
                let by_id: HashMap<Id, &List> = remaining_lists.iter().map(|l| (l.id, l)).collect();
                let insert_at = match target {
                    FolderDropTarget::List(target_list) => {
                        let Some(pos) = visual.iter().position(|&id| id == target_list) else {
                            return Ok(());
                        };
                        pos + usize::from(after)
                    }
                    FolderDropTarget::Folder(target_folder) => {
                        let pos = if !after {
                            visual.iter().position(|id| by_id.get(id).is_some_and(|l| !l.archived && l.folder == Some(target_folder)))
                        } else {
                            visual.iter().rposition(|id| by_id.get(id).is_some_and(|l| !l.archived && l.folder == Some(target_folder))).map(|p| p + 1)
                        };
                        pos.unwrap_or_else(|| {
                            visual.iter().rposition(|id| by_id.get(id).is_some_and(|l| !l.archived)).map_or(0, |p| p + 1)
                        })
                    }
                };
                visual.splice(insert_at..insert_at, moving);
                self.write_sort_order(SortTable::Lists, &visual)?;
            }

            let mut folder_order: Vec<Id> = folders.iter().map(|f| f.id).filter(|&id| id != dragged).collect();
            let folder_insert = match target {
                FolderDropTarget::Folder(target_folder) => folder_order
                    .iter()
                    .position(|&id| id == target_folder)
                    .map_or(folder_order.len(), |pos| pos + usize::from(after)),
                FolderDropTarget::List(_) => if after { folder_order.len() } else { 0 },
            };
            folder_order.insert(folder_insert, dragged);
            self.write_sort_order(SortTable::ListFolders, &folder_order)?;
            Ok(())
        })
    }

    /// Commits live-reordered lists (including `folder` membership) and `list_folders` order in a
    /// single transaction.
    pub fn save_list_and_folder_order(&self, lists: &[List], folders: &[ListFolder]) -> Result<()> {
        self.in_tx(|| {
            let visual = Self::visual_list_order(lists, folders);
            let by_id: HashMap<Id, &List> = lists.iter().map(|l| (l.id, l)).collect();
            let valid_folders: HashSet<Id> = folders.iter().map(|f| f.id).collect();
            for (i, list_id) in visual.iter().enumerate() {
                if let Some(list) = by_id.get(list_id) {
                    let folder = list.folder.filter(|f| valid_folders.contains(f));
                    self.conn.execute(
                        "UPDATE lists SET sort_order = ?2, folder_id = ?3
                         WHERE id = ?1 AND (sort_order != ?2 OR folder_id IS NOT ?3)",
                        params![list_id.to_string(), i as i64, folder.map(|f| f.to_string())],
                    )?;
                }
            }
            let folder_ids: Vec<Id> = folders.iter().map(|f| f.id).collect();
            self.write_sort_order(SortTable::ListFolders, &folder_ids)?;
            Ok(())
        })
    }

    /// The folders of lists, in their order.
    pub fn list_folders(&self) -> Result<Vec<ListFolder>> {
        let mut stmt = self.conn.prepare_cached("SELECT id, name FROM list_folders ORDER BY sort_order, created_at, rowid")?;
        let folders = stmt
            .query_map([], |row| Ok(ListFolder { id: parse_uuid(&row.get::<_, String>(0)?, 0)?, name: row.get(1)? }))?
            .collect::<Result<_, _>>()?;
        Ok(folders)
    }

    pub fn add_list_folder(&self, name: &str) -> Result<ListFolder> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a folder needs a name");
        let folder = ListFolder { id: Uuid::now_v7(), name: name.to_owned() };
        self.conn.execute(
            "INSERT INTO list_folders (id, name, created_at, sort_order) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM list_folders))",
            params![folder.id.to_string(), folder.name, now_ms()],
        )?;
        Ok(folder)
    }

    pub fn rename_list_folder(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a folder needs a name");
        self.conn.execute("UPDATE list_folders SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// The original's Ungroup: the folder goes, its lists stay, out of any folder.
    pub fn ungroup_list_folder(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("UPDATE lists SET folder_id = NULL WHERE folder_id = ?1", [id.to_string()])?;
            self.conn.execute("DELETE FROM list_folders WHERE id = ?1", [id.to_string()])?;
            Ok(())
        })
    }

    /// Lists whose tasks stay out of the smart lists: archived, completed projects, and those set not to show.
    pub(super) fn hidden_lists(&self) -> Result<HashSet<Id>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM lists WHERE archived_at IS NOT NULL OR completed_at IS NOT NULL OR in_smart_lists = 0",
        )?;
        let ids = stmt
            .query_map([], |row| parse_uuid(&row.get::<_, String>(0)?, 0))?
            .collect::<Result<HashSet<Id>, _>>()?;
        Ok(ids)
    }

    pub fn rename_list(&self, id: Id, name: &str) -> Result<()> {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "a list needs a name");
        self.conn.execute("UPDATE lists SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    /// Sets the list color as `#RRGGBB`, or clears it with `None`.
    pub fn set_list_color(&self, id: Id, color: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE lists SET color = ?2 WHERE id = ?1", params![id.to_string(), color])?;
        Ok(())
    }

    /// Deletes the list together with its tasks and unlinks orphaned attachment files when an
    /// attachment directory is configured on this store.
    pub fn delete_list(&self, id: Id) -> Result<()> {
        self.delete_list_with_dir(id, None)
    }

    /// Deletes the list together with its tasks, pruning `sync_uploaded` and unlinking orphaned
    /// attachment files in `dir` (or the store's default attachment directory).
    pub fn delete_list_with_dir(&self, id: Id, dir: Option<&Path>) -> Result<()> {
        let removed_files = self.in_tx(|| {
            let files = self.collect_attachment_files_for_query(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE list_id = ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 SELECT a.id || '-' || a.name FROM task_attachments a JOIN sub ON a.task_id = sub.id",
                params![id.to_string()],
            )?;
            self.conn.execute(
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM tasks WHERE list_id = ?1
                     UNION
                     SELECT t.id FROM tasks t JOIN sub ON t.parent_id = sub.id
                 )
                 DELETE FROM plugin_entity_meta
                 WHERE entity_type = 'task' AND entity_id IN (SELECT id FROM sub)",
                [id.to_string()],
            )?;
            self.conn.execute("DELETE FROM lists WHERE id = ?1", [id.to_string()])?;
            self.prune_sync_uploaded(&files)?;
            Ok(files)
        })?;
        self.unlink_attachment_files(&removed_files, dir);
        Ok(())
    }

    /// Moves the task and its subtasks to a list (`None` means the Inbox).
    /// Moving a subtask to a different list than its parent unlinks it from that parent.
    pub fn set_list(&self, id: Id, list: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            let Some(task) = self.task(id)? else { return Ok(()) };
            let list_str = list.map(|l| l.to_string());
            let unlink = match task.parent_id {
                Some(parent_id) => {
                    let parent_list: Option<Option<String>> = self
                        .conn
                        .prepare_cached("SELECT list_id FROM tasks WHERE id = ?1")?
                        .query_row([parent_id.to_string()], |row| row.get(0))
                        .optional()?;
                    parent_list.is_none_or(|pl| pl != list_str)
                }
                None => false,
            };
            if task.list_id == list && !unlink {
                return Ok(());
            }
            if unlink {
                self.conn.execute("UPDATE tasks SET parent_id = NULL WHERE id = ?1", [id.to_string()])?;
            }
            let list_changed = task.list_id != list;
            let sql = if list_changed {
                // A section belongs to its list, so moving to another list leaves it.
                "UPDATE tasks SET list_id = ?2, section_id = NULL WHERE id = ?1"
            } else {
                "UPDATE tasks SET list_id = ?2 WHERE id = ?1"
            };
            let mut stmt = self.conn.prepare_cached(sql)?;
            for sub in std::iter::once(id).chain(self.descendants(id)?) {
                stmt.execute(params![sub.to_string(), list_str])?;
            }
            Ok(())
        })
    }

    /// The task's checklist in display order.
    pub fn checklist(&self, task: Id) -> Result<Vec<ChecklistItem>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, task_id, title, completed_at IS NOT NULL FROM checklist_items
             WHERE task_id = ?1 ORDER BY sort_order, rowid",
        )?;
        let items = stmt
            .query_map([task.to_string()], |row| {
                Ok(ChecklistItem {
                    id: parse_uuid(&row.get::<_, String>(0)?, 0)?,
                    task_id: parse_uuid(&row.get::<_, String>(1)?, 1)?,
                    title: row.get(2)?,
                    completed: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(items)
    }

    /// Adds an item to the end of the checklist, or right after `after` when given.
    pub fn add_checklist_item(&self, task: Id, title: &str, after: Option<Id>) -> Result<ChecklistItem> {
        self.in_tx(|| {
            let position: i64 = match after {
                Some(after) => {
                    let position: i64 = self.conn.query_row(
                        "SELECT sort_order FROM checklist_items WHERE id = ?1",
                        [after.to_string()],
                        |row| row.get(0),
                    )?;
                    self.conn.execute(
                        "UPDATE checklist_items SET sort_order = sort_order + 1
                         WHERE task_id = ?1 AND sort_order > ?2",
                        params![task.to_string(), position],
                    )?;
                    position + 1
                }
                None => self.conn.query_row(
                    "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM checklist_items WHERE task_id = ?1",
                    [task.to_string()],
                    |row| row.get(0),
                )?,
            };
            let item = ChecklistItem { id: Uuid::now_v7(), task_id: task, title: title.trim().to_owned(), completed: false };
            self.conn.execute(
                "INSERT INTO checklist_items (id, task_id, title, sort_order, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![item.id.to_string(), task.to_string(), item.title, position, now_ms()],
            )?;
            Ok(item)
        })
    }

    pub fn set_checklist_item_title(&self, id: Id, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE checklist_items SET title = ?2 WHERE id = ?1",
            params![id.to_string(), title],
        )?;
        Ok(())
    }

    pub fn set_checklist_item_completed(&self, id: Id, completed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE checklist_items SET completed_at = ?2 WHERE id = ?1",
            params![id.to_string(), completed.then(now_ms)],
        )?;
        Ok(())
    }

    pub fn delete_checklist_item(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM checklist_items WHERE id = ?1", [id.to_string()])?;
        Ok(())
    }

    pub(super) fn checklist_progress(&self, ids_json: Option<&str>) -> Result<HashMap<Id, (usize, usize)>> {
        let mut stmt = self.conn.prepare_cached(if ids_json.is_some() {
            "SELECT task_id, COUNT(completed_at), COUNT(*) FROM checklist_items
             WHERE task_id IN (SELECT value FROM json_each(?1))
             GROUP BY task_id"
        } else {
            "SELECT task_id, COUNT(completed_at), COUNT(*) FROM checklist_items GROUP BY task_id"
        })?;
        let mut progress = HashMap::new();
        for row in stmt.query_map(rusqlite::params_from_iter(ids_json), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })? {
            let (task, done, total) = row?;
            if let Ok(id) = Uuid::parse_str(&task) {
                progress.insert(id, (done as usize, total as usize));
            }
        }
        Ok(progress)
    }
}
