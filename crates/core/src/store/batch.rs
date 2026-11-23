use anyhow::Result;
use chrono::{NaiveDate, NaiveTime};

use crate::model::{Id, Kind, Priority, Task};

use super::Store;

impl Store {
    /// Completes or reopens all given tasks inside one transaction.
    pub fn batch_complete(&self, ids: &[Id], completed: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_completed(id, completed)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_wont_do(&self, ids: &[Id], wont_do: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_wont_do(id, wont_do)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_in_progress(&self, ids: &[Id], in_progress: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_in_progress(id, in_progress)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_due(&self, ids: &[Id], date: Option<NaiveDate>, time: Option<NaiveTime>) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_due(id, date, time)?;
            }
            Ok(())
        })
    }

    pub fn batch_move_dates(&self, ids: &[Id], date: NaiveDate) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.move_dates(id, date)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_priority(&self, ids: &[Id], priority: Priority) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_priority(id, priority)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_list(&self, ids: &[Id], list_id: Option<Id>) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_list(id, list_id)?;
            }
            Ok(())
        })
    }

    pub fn batch_add_tag(&self, ids: &[Id], tag: &str) -> Result<()> {
        let tag = Self::sanitize_tag_name(tag);
        if tag.is_empty() {
            return Ok(());
        }
        let tag = tag.as_str();
        self.in_tx(|| {
            for &id in ids {
                let Some(task) = self.task(id)? else { continue };
                if !task.tags.iter().any(|existing| existing.to_lowercase() == tag.to_lowercase()) {
                    let mut next = task.tags;
                    next.push(tag.to_owned());
                    self.replace_task_tags(id, &next)?;
                }
            }
            Ok(())
        })
    }

    pub fn batch_remove_tag(&self, ids: &[Id], tag: &str) -> Result<()> {
        let tag = Self::sanitize_tag_name(tag);
        if tag.is_empty() {
            return Ok(());
        }
        let tag = tag.as_str();
        self.in_tx(|| {
            for &id in ids {
                let Some(task) = self.task(id)? else { continue };
                let filtered: Vec<String> = task
                    .tags
                    .into_iter()
                    .filter(|existing| existing.to_lowercase() != tag.to_lowercase())
                    .collect();
                self.replace_task_tags(id, &filtered)?;
            }
            Ok(())
        })
    }

    pub fn batch_set_pinned(&self, ids: &[Id], pinned: bool) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_pinned(id, pinned)?;
            }
            Ok(())
        })
    }

    pub fn batch_delete(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.delete_task(id)?;
            }
            Ok(())
        })
    }

    pub fn batch_restore(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.restore_task(id)?;
            }
            Ok(())
        })
    }

    pub fn batch_purge(&self, ids: &[Id]) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.purge_task(id)?;
            }
            Ok(())
        })
    }

    pub fn batch_duplicate(&self, ids: &[Id]) -> Result<Vec<Task>> {
        self.in_tx(|| {
            let mut created = Vec::with_capacity(ids.len());
            for &id in ids {
                if self.task(id)?.is_some() {
                    created.push(self.duplicate_task(id)?);
                }
            }
            Ok(created)
        })
    }

    pub fn batch_set_kind(&self, ids: &[Id], kind: Kind) -> Result<()> {
        self.in_tx(|| {
            for &id in ids {
                self.set_kind(id, kind)?;
            }
            Ok(())
        })
    }

    /// Merges multiple selected tasks into the first task: subsequent tasks (and their checklist
    /// items) become checklist items on the first task, their tags and notes are merged in, and
    /// the absorbed tasks are moved to the Trash.
    pub fn batch_merge_tasks(&self, ids: &[Id]) -> Result<Option<Id>> {
        if ids.len() < 2 {
            return Ok(None);
        }
        self.in_tx(|| {
            let Some(target) = self.task(ids[0])? else {
                return Ok(None);
            };
            let mut tags = target.tags.clone();
            let mut tags_changed = false;
            let mut notes = target.notes.trim_end().to_owned();
            let mut notes_changed = false;

            for &other_id in &ids[1..] {
                if other_id == target.id {
                    continue;
                }
                let Some(other) = self.task(other_id)? else {
                    continue;
                };
                if !other.title.trim().is_empty() {
                    let added = self.add_checklist_item(target.id, other.title.trim(), None)?;
                    if other.is_completed() {
                        self.set_checklist_item_completed(added.id, true)?;
                    }
                }
                for item in self.checklist(other.id)? {
                    let copied = self.add_checklist_item(target.id, &item.title, None)?;
                    if item.completed {
                        self.set_checklist_item_completed(copied.id, true)?;
                    }
                }
                for tag in other.tags {
                    if !tags.iter().any(|existing| existing.to_lowercase() == tag.to_lowercase()) {
                        tags.push(tag);
                        tags_changed = true;
                    }
                }
                let other_notes = other.notes.trim();
                if !other_notes.is_empty() {
                    if !notes.is_empty() {
                        notes.push_str("\n\n");
                    }
                    notes.push_str(other_notes);
                    notes_changed = true;
                }
                self.delete_task(other.id)?;
            }
            if tags_changed {
                self.replace_task_tags(target.id, &tags)?;
            }
            if notes_changed {
                self.set_notes(target.id, &notes)?;
            }
            Ok(Some(target.id))
        })
    }
}
