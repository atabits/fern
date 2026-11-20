//! The Eisenhower Matrix: four quadrants, each with independent filter rules on lists, tags,
//! dates, priorities, and item kind (task/note). Rules are stored in `matrix_rules` and synced
//! between devices.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, NaiveDate};

use crate::model::{Id, Kind, Priority, Task};
use crate::store::Store;

/// Date conditions supported by a quadrant rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixDate {
    Overdue,
    Today,
    Tomorrow,
    /// Two or more days from today (legacy `"later"`).
    Later,
    NoDate,
    /// `days` or more days from today (`days >= 2`).
    DaysLater(u32),
}

impl MatrixDate {
    /// The five standard date chips shown in the rule editor.
    pub const ALL: [Self; 5] = [Self::Overdue, Self::Today, Self::Tomorrow, Self::Later, Self::NoDate];

    pub fn key(self) -> String {
        match self {
            Self::Overdue => "overdue".into(),
            Self::Today => "today".into(),
            Self::Tomorrow => "tomorrow".into(),
            Self::Later => "later".into(),
            Self::NoDate => "none".into(),
            Self::DaysLater(n) => format!("days_later:{}", n.max(2)),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "overdue" => Some(Self::Overdue),
            "today" => Some(Self::Today),
            "tomorrow" => Some(Self::Tomorrow),
            "later" => Some(Self::Later),
            "none" => Some(Self::NoDate),
            other => other
                .strip_prefix("days_later:")
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n >= 2)
                .map(Self::DaysLater),
        }
    }

    pub fn matches(self, due: Option<NaiveDate>, today: NaiveDate) -> bool {
        match (self, due) {
            (Self::NoDate, due) => due.is_none(),
            (_, None) => false,
            (Self::Overdue, Some(d)) => d < today,
            (Self::Today, Some(d)) => d == today,
            (Self::Tomorrow, Some(d)) => d == today + Duration::days(1),
            (Self::Later, Some(d)) => d > today + Duration::days(1),
            (Self::DaysLater(n), Some(d)) => d >= today + Duration::days(n.max(2) as i64),
        }
    }
}

/// Whether a quadrant matches tasks, notes, or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MatrixKind {
    #[default]
    All,
    TaskOnly,
    NoteOnly,
}

impl MatrixKind {
    pub const ALL: [Self; 3] = [Self::All, Self::TaskOnly, Self::NoteOnly];

    pub fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::TaskOnly => "task",
            Self::NoteOnly => "note",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "task" => Self::TaskOnly,
            "note" => Self::NoteOnly,
            _ => Self::All,
        }
    }

    pub fn matches(self, kind: Kind) -> bool {
        match self {
            Self::All => true,
            Self::TaskOnly => kind == Kind::Task,
            Self::NoteOnly => kind == Kind::Note,
        }
    }
}

/// One quadrant's rule. An empty vector condition matches everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuadrantRule {
    /// Custom quadrant title (may include an emoji); empty uses the localized default title.
    pub name: String,
    /// Lists to include (`None` is the Inbox).
    pub lists: Vec<Option<Id>>,
    pub tags: Vec<String>,
    pub dates: Vec<MatrixDate>,
    pub priorities: Vec<Priority>,
    pub kind: MatrixKind,
}

/// Computed changes to apply to a task when dropped onto a quadrant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropPatch {
    pub priority: Option<Priority>,
    /// `Some(None)` clears the due date; `Some(Some(d))` sets it to `d`.
    pub due_date: Option<Option<NaiveDate>>,
    /// `Some(list)` moves the task to `list` (`None` = Inbox).
    pub list_id: Option<Option<Id>>,
    /// Tag to append if the task didn't have any matching tag.
    pub add_tag: Option<String>,
    pub kind: Option<Kind>,
}

/// Outcome of dropping a task onto a quadrant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropOutcome {
    /// The task already satisfies the target quadrant's rule.
    AlreadyMatches,
    /// The target quadrant's rule cannot be satisfied (e.g. setting priority on a note, or no available list).
    Unsupported,
    /// Apply these field updates to move the task into the quadrant.
    Apply(DropPatch),
}

impl QuadrantRule {
    /// Default rules matching the web reference: pure priority quadrants
    /// (Q1 High, Q2 Medium, Q3 Low, Q4 None).
    pub fn defaults() -> [Self; 4] {
        [
            Self { priorities: vec![Priority::High], ..Self::default() },
            Self { priorities: vec![Priority::Medium], ..Self::default() },
            Self { priorities: vec![Priority::Low], ..Self::default() },
            Self { priorities: vec![Priority::None], ..Self::default() },
        ]
    }

    /// Date + Priority template ("Urgent/Important by dates"):
    /// urgent = Overdue / Today / Tomorrow, not urgent = NoDate / Later;
    /// important = High / Medium, unimportant = Low / None.
    pub fn date_priority_template() -> [Self; 4] {
        let urgent = vec![MatrixDate::Overdue, MatrixDate::Today, MatrixDate::Tomorrow];
        let not_urgent = vec![MatrixDate::NoDate, MatrixDate::Later];
        let important = vec![Priority::High, Priority::Medium];
        let unimportant = vec![Priority::Low, Priority::None];
        let rule = |dates: &Vec<MatrixDate>, priorities: &Vec<Priority>| Self {
            dates: dates.clone(),
            priorities: priorities.clone(),
            ..Self::default()
        };
        [
            rule(&urgent, &important),
            rule(&not_urgent, &important),
            rule(&urgent, &unimportant),
            rule(&not_urgent, &unimportant),
        ]
    }

    /// Validates a custom quadrant name: must not start with `#` or contain control characters.
    pub fn validate_name(name: &str) -> bool {
        let trimmed = name.trim();
        !trimmed.starts_with('#') && !trimmed.chars().any(|c| c.is_control())
    }

    /// Deterministic preferred priority for newly created or dropped tasks (highest allowed priority,
    /// independent of chip click order).
    pub fn preferred_priority(&self) -> Option<Priority> {
        self.priorities.iter().copied().max()
    }

    /// Nearest upcoming date admitted by the quadrant, for newly created tasks.
    /// An overdue-only rule starts today.
    pub fn new_task_defaults(&self, today: NaiveDate) -> (Option<Priority>, Option<NaiveDate>) {
        let date = if self.dates.is_empty() || self.dates.contains(&MatrixDate::NoDate) {
            None
        } else if self.dates.contains(&MatrixDate::Today) {
            Some(today)
        } else if self.dates.contains(&MatrixDate::Tomorrow) {
            today.succ_opt()
        } else if self.dates.contains(&MatrixDate::Later) {
            today.checked_add_signed(Duration::days(2))
        } else if let Some(days) = self
            .dates
            .iter()
            .filter_map(|d| match d {
                MatrixDate::DaysLater(n) => Some(*n),
                _ => None,
            })
            .min()
        {
            today.checked_add_signed(Duration::days(days.max(2) as i64))
        } else {
            Some(today)
        };
        (self.preferred_priority(), date)
    }

    /// Computes what fields must change when a task is dropped onto this quadrant.
    pub fn drop_changes(
        &self,
        task: &Task,
        today: NaiveDate,
        available_lists: &[Id],
        tag_families: &HashMap<String, HashSet<String>>,
    ) -> DropOutcome {
        if self.matches_with_tags(task, today, tag_families) {
            return DropOutcome::AlreadyMatches;
        }
        let mut patch = DropPatch::default();
        let effective_kind = match self.kind {
            MatrixKind::All => task.kind,
            MatrixKind::TaskOnly => {
                if task.kind != Kind::Task {
                    patch.kind = Some(Kind::Task);
                }
                Kind::Task
            }
            MatrixKind::NoteOnly => {
                if task.kind != Kind::Note {
                    patch.kind = Some(Kind::Note);
                }
                Kind::Note
            }
        };

        if !self.priorities.is_empty() && !self.priorities.contains(&task.priority) {
            if effective_kind == Kind::Note {
                return DropOutcome::Unsupported;
            }
            patch.priority = self.preferred_priority();
        }

        if !self.dates.is_empty() && !self.dates.iter().any(|d| d.matches(task.due_date, today)) {
            if self.dates.contains(&MatrixDate::NoDate) {
                patch.due_date = Some(None);
            } else if self.dates.contains(&MatrixDate::Today) {
                patch.due_date = Some(Some(today));
            } else if self.dates.contains(&MatrixDate::Tomorrow) {
                patch.due_date = Some(today.succ_opt());
            } else if self.dates.contains(&MatrixDate::Later) {
                patch.due_date = Some(today.checked_add_signed(Duration::days(2)));
            } else if let Some(days) = self
                .dates
                .iter()
                .filter_map(|d| match d {
                    MatrixDate::DaysLater(n) => Some(*n),
                    _ => None,
                })
                .min()
            {
                patch.due_date = Some(today.checked_add_signed(Duration::days(days.max(2) as i64)));
            } else if self.dates.contains(&MatrixDate::Overdue) {
                patch.due_date = Some(today.pred_opt());
            }
        }

        if !self.lists.is_empty() && !self.lists.contains(&task.list_id) {
            let target = self
                .lists
                .iter()
                .copied()
                .find(|l| l.is_none_or(|id| available_lists.contains(&id)));
            match target {
                Some(list) => patch.list_id = Some(list),
                None => return DropOutcome::Unsupported,
            }
        }

        if !self.tags.is_empty()
            && !self.tags_match(task, tag_families)
            && let Some(tag) = self.tags.first()
        {
            patch.add_tag = Some(tag.clone());
        }

        DropOutcome::Apply(patch)
    }

    fn tags_match(&self, task: &Task, tag_families: &HashMap<String, HashSet<String>>) -> bool {
        self.tags.iter().any(|rule_tag| {
            let key = rule_tag.to_lowercase();
            let family = tag_families.get(&key);
            task.tags.iter().any(|t| {
                let lower = t.to_lowercase();
                lower == key || family.is_some_and(|set| set.contains(&lower))
            })
        })
    }

    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        self.matches_with_tags(task, today, &HashMap::new())
    }

    pub fn matches_with_tags(
        &self,
        task: &Task,
        today: NaiveDate,
        tag_families: &HashMap<String, HashSet<String>>,
    ) -> bool {
        self.kind.matches(task.kind)
            && (self.lists.is_empty() || self.lists.contains(&task.list_id))
            && (self.tags.is_empty() || self.tags_match(task, tag_families))
            && (self.dates.is_empty() || self.dates.iter().any(|d| d.matches(task.due_date, today)))
            && (self.priorities.is_empty() || self.priorities.contains(&task.priority))
    }

    fn to_storage(&self) -> String {
        let lists: Vec<String> = self.lists.iter().map(|l| l.map_or("inbox".to_owned(), |id| id.to_string())).collect();
        let dates: Vec<String> = self.dates.iter().map(|d| d.key()).collect();
        let priorities: Vec<String> = self.priorities.iter().map(|p| p.to_i64().to_string()).collect();
        serde_json::json!({
            "name": self.name,
            "lists": lists,
            "tags": self.tags,
            "dates": dates,
            "priorities": priorities,
            "kind": self.kind.key(),
        })
        .to_string()
    }

    fn from_storage(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let strings = |key: &str| -> Vec<String> {
            value[key]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default()
        };
        Some(Self {
            name: value["name"].as_str().unwrap_or_default().to_owned(),
            lists: strings("lists")
                .iter()
                .filter_map(|l| if l == "inbox" { Some(None) } else { l.parse().ok().map(Some) })
                .collect(),
            tags: strings("tags"),
            dates: strings("dates").iter().filter_map(|k| MatrixDate::from_key(k)).collect(),
            priorities: strings("priorities").iter().filter_map(|p| p.parse().ok()).map(Priority::from_i64).collect(),
            kind: value["kind"].as_str().map(MatrixKind::from_key).unwrap_or_default(),
        })
    }
}

/// First quadrant (0–3) whose rule `task` satisfies (used for destination notices).
pub fn quadrant_of(rules: &[QuadrantRule; 4], task: &Task, today: NaiveDate) -> Option<usize> {
    rules.iter().position(|rule| rule.matches(task, today))
}

/// All quadrants (0–3) whose rules `task` satisfies (independent per-quadrant filtering).
pub fn quadrants_of(
    rules: &[QuadrantRule; 4],
    task: &Task,
    today: NaiveDate,
    tag_families: &HashMap<String, HashSet<String>>,
) -> Vec<usize> {
    rules
        .iter()
        .enumerate()
        .filter_map(|(i, rule)| rule.matches_with_tags(task, today, tag_families).then_some(i))
        .collect()
}

/// Builds a map from lowercased parent tag name to its set of descendant tag names (lowercased).
pub fn build_tag_families(tags: &[crate::model::Tag]) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    for tag in tags {
        let lower = tag.name.to_lowercase();
        map.entry(lower.clone()).or_default().insert(lower.clone());
        if let Some(parent) = &tag.parent {
            map.entry(parent.to_lowercase()).or_default().insert(lower);
        }
    }
    map
}

/// A named 4-quadrant Eisenhower Matrix profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixProfile {
    pub id: Id,
    pub name: String,
    pub rules: [QuadrantRule; 4],
    pub sort_order: i64,
}

fn encode_profile_rules(rules: &[QuadrantRule; 4]) -> String {
    let items: [String; 4] = [
        rules[0].to_storage(),
        rules[1].to_storage(),
        rules[2].to_storage(),
        rules[3].to_storage(),
    ];
    serde_json::to_string(&items).unwrap_or_default()
}

fn decode_profile_rules(raw: &str) -> [QuadrantRule; 4] {
    let mut rules = QuadrantRule::defaults();
    if let Ok(items) = serde_json::from_str::<Vec<String>>(raw) {
        for (i, text) in items.into_iter().take(4).enumerate() {
            if let Some(parsed) = QuadrantRule::from_storage(&text) {
                rules[i] = parsed;
            }
        }
    }
    rules
}

impl Store {
    /// Loads all four quadrants' rules in one query, falling back to defaults for unsaved rows.
    pub fn matrix_rules(&self) -> anyhow::Result<[QuadrantRule; 4]> {
        let mut rules = QuadrantRule::defaults();
        let mut stmt = self.conn.prepare_cached("SELECT quadrant, rule FROM matrix_rules")?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        for row in rows {
            let (quadrant, text) = row?;
            if let Ok(index) = quadrant.parse::<usize>()
                && index < 4
                && let Some(saved) = QuadrantRule::from_storage(&text)
            {
                rules[index] = saved;
            }
        }
        Ok(rules)
    }

    /// Saves a quadrant's rule; `None` restores the default.
    pub fn save_matrix_rule(&self, quadrant: usize, rule: Option<&QuadrantRule>) -> anyhow::Result<()> {
        anyhow::ensure!(quadrant < 4, "there are four quadrants");
        let key = quadrant.to_string();
        match rule {
            Some(rule) => self.conn.execute(
                "INSERT INTO matrix_rules (quadrant, rule) VALUES (?1, ?2)
                 ON CONFLICT(quadrant) DO UPDATE SET rule = excluded.rule",
                rusqlite::params![key, rule.to_storage()],
            )?,
            None => self.conn.execute("DELETE FROM matrix_rules WHERE quadrant = ?1", [key])?,
        };
        if let Ok((first_id, raw)) = self
            .conn
            .query_row("SELECT id, rules_json FROM matrix_profiles ORDER BY sort_order, rowid LIMIT 1", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
        {
            let mut rules = decode_profile_rules(&raw);
            rules[quadrant] = rule.cloned().unwrap_or_else(|| QuadrantRule::defaults()[quadrant].clone());
            self.conn.execute(
                "UPDATE matrix_profiles SET rules_json = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![first_id, encode_profile_rules(&rules), crate::store::now_ms()],
            )?;
        }
        Ok(())
    }

    /// Applies the 4-quadrant Date + Priority template, preserving any custom quadrant names.
    pub fn apply_matrix_date_template(&self) -> anyhow::Result<[QuadrantRule; 4]> {
        let current = self.matrix_rules()?;
        let mut template = QuadrantRule::date_priority_template();
        for (i, rule) in template.iter_mut().enumerate() {
            rule.name = current[i].name.clone();
        }
        self.in_tx(|| {
            for (i, rule) in template.iter().enumerate() {
                self.save_matrix_rule(i, Some(rule))?;
            }
            Ok(())
        })?;
        Ok(template)
    }

    /// Ensures at least one profile exists (seeding the first one from legacy `matrix_rules`).
    fn ensure_default_matrix_profile(&self) -> anyhow::Result<Id> {
        if let Some(id_str) = self
            .conn
            .query_row("SELECT id FROM matrix_profiles ORDER BY sort_order, rowid LIMIT 1", [], |r| r.get::<_, String>(0))
            .ok()
            && let Ok(id) = uuid::Uuid::parse_str(&id_str)
        {
            return Ok(id);
        }
        let id = uuid::Uuid::now_v7();
        let rules = self.matrix_rules()?;
        let json = encode_profile_rules(&rules);
        self.conn.execute(
            "INSERT INTO matrix_profiles (id, name, rules_json, sort_order, updated_at) VALUES (?1, 'Default', ?2, 0, ?3)",
            rusqlite::params![id.to_string(), json, crate::store::now_ms()],
        )?;
        Ok(id)
    }

    /// All Eisenhower Matrix profiles in display order (creating `"Default"` on first call).
    pub fn matrix_profiles(&self) -> anyhow::Result<Vec<MatrixProfile>> {
        self.ensure_default_matrix_profile()?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name, rules_json, sort_order FROM matrix_profiles ORDER BY sort_order, rowid")?;
        let rows = stmt
            .query_map([], |row| {
                let id_str: String = row.get(0)?;
                let id = uuid::Uuid::parse_str(&id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
                })?;
                let raw_rules: String = row.get(2)?;
                Ok(MatrixProfile {
                    id,
                    name: row.get(1)?,
                    rules: decode_profile_rules(&raw_rules),
                    sort_order: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The currently active Matrix profile ID (falls back to the first profile).
    pub fn active_matrix_profile_id(&self) -> anyhow::Result<Id> {
        let profiles = self.matrix_profiles()?;
        if let Some(saved) = self.setting("matrix_active_profile")?
            && let Ok(id) = uuid::Uuid::parse_str(&saved)
            && profiles.iter().any(|p| p.id == id)
        {
            return Ok(id);
        }
        let first = profiles[0].id;
        self.set_setting("matrix_active_profile", &first.to_string())?;
        Ok(first)
    }

    pub fn set_active_matrix_profile(&self, id: Id) -> anyhow::Result<()> {
        let profiles = self.matrix_profiles()?;
        anyhow::ensure!(profiles.iter().any(|p| p.id == id), "matrix profile not found");
        self.set_setting("matrix_active_profile", &id.to_string())?;
        Ok(())
    }

    /// Creates a new Matrix profile (using `rules` or defaults) and returns it.
    pub fn create_matrix_profile(&self, name: &str, rules: Option<[QuadrantRule; 4]>) -> anyhow::Result<MatrixProfile> {
        let trimmed = name.trim();
        anyhow::ensure!(!trimmed.is_empty(), "matrix profile name cannot be empty");
        self.ensure_default_matrix_profile()?;
        let max_order: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(sort_order), -1) FROM matrix_profiles", [], |r| r.get(0))?;
        let profile = MatrixProfile {
            id: uuid::Uuid::now_v7(),
            name: trimmed.to_owned(),
            rules: rules.unwrap_or_else(QuadrantRule::defaults),
            sort_order: max_order + 1,
        };
        self.conn.execute(
            "INSERT INTO matrix_profiles (id, name, rules_json, sort_order, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                profile.id.to_string(),
                profile.name,
                encode_profile_rules(&profile.rules),
                profile.sort_order,
                crate::store::now_ms()
            ],
        )?;
        Ok(profile)
    }

    pub fn rename_matrix_profile(&self, id: Id, name: &str) -> anyhow::Result<()> {
        let trimmed = name.trim();
        anyhow::ensure!(!trimmed.is_empty(), "matrix profile name cannot be empty");
        let changed = self.conn.execute(
            "UPDATE matrix_profiles SET name = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id.to_string(), trimmed, crate::store::now_ms()],
        )?;
        anyhow::ensure!(changed > 0, "matrix profile not found");
        Ok(())
    }

    /// Deletes a non-last Matrix profile and switches active selection if needed.
    pub fn delete_matrix_profile(&self, id: Id) -> anyhow::Result<bool> {
        let profiles = self.matrix_profiles()?;
        if profiles.len() <= 1 || !profiles.iter().any(|p| p.id == id) {
            return Ok(false);
        }
        self.conn.execute("DELETE FROM matrix_profiles WHERE id = ?1", [id.to_string()])?;
        if self.setting("matrix_active_profile")?.as_deref() == Some(id.to_string().as_str())
            && let Some(next) = profiles.into_iter().find(|p| p.id != id)
        {
            self.set_setting("matrix_active_profile", &next.id.to_string())?;
        }
        Ok(true)
    }

    /// Saves one quadrant rule for `profile_id` (`None` restores the quadrant's default).
    pub fn save_matrix_profile_rule(&self, profile_id: Id, quadrant: usize, rule: Option<&QuadrantRule>) -> anyhow::Result<()> {
        anyhow::ensure!(quadrant < 4, "there are four quadrants");
        let profiles = self.matrix_profiles()?;
        let is_first = profiles.first().is_some_and(|p| p.id == profile_id);
        let mut target = profiles.into_iter().find(|p| p.id == profile_id).ok_or_else(|| anyhow::anyhow!("matrix profile not found"))?;
        target.rules[quadrant] = rule.cloned().unwrap_or_else(|| QuadrantRule::defaults()[quadrant].clone());
        self.conn.execute(
            "UPDATE matrix_profiles SET rules_json = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![profile_id.to_string(), encode_profile_rules(&target.rules), crate::store::now_ms()],
        )?;
        if is_first {
            let key = quadrant.to_string();
            match rule {
                Some(rule) => self.conn.execute(
                    "INSERT INTO matrix_rules (quadrant, rule) VALUES (?1, ?2)
                     ON CONFLICT(quadrant) DO UPDATE SET rule = excluded.rule",
                    rusqlite::params![key, rule.to_storage()],
                )?,
                None => self.conn.execute("DELETE FROM matrix_rules WHERE quadrant = ?1", [key])?,
            };
        }
        Ok(())
    }

    /// Applies the Date + Priority template to `profile_id`, keeping its custom quadrant names.
    pub fn apply_matrix_profile_date_template(&self, profile_id: Id) -> anyhow::Result<[QuadrantRule; 4]> {
        let profiles = self.matrix_profiles()?;
        let is_first = profiles.first().is_some_and(|p| p.id == profile_id);
        let current = profiles
            .into_iter()
            .find(|p| p.id == profile_id)
            .ok_or_else(|| anyhow::anyhow!("matrix profile not found"))?;
        let mut template = QuadrantRule::date_priority_template();
        for (i, rule) in template.iter_mut().enumerate() {
            rule.name = current.rules[i].name.clone();
        }
        self.conn.execute(
            "UPDATE matrix_profiles SET rules_json = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![profile_id.to_string(), encode_profile_rules(&template), crate::store::now_ms()],
        )?;
        if is_first {
            self.in_tx(|| {
                for (i, rule) in template.iter().enumerate() {
                    let key = i.to_string();
                    self.conn.execute(
                        "INSERT INTO matrix_rules (quadrant, rule) VALUES (?1, ?2)
                         ON CONFLICT(quadrant) DO UPDATE SET rule = excluded.rule",
                        rusqlite::params![key, rule.to_storage()],
                    )?;
                }
                Ok(())
            })?;
        }
        Ok(template)
    }

    /// Loads tasks visible in the matrix: excludes deleted, Won't Do, and archived lists;
    /// lists with `in_smart_lists == false` are excluded UNLESS explicitly selected in a quadrant rule.
    /// Completed tasks are capped at `completed_limit` most recent when `show_completed` is true.
    pub fn matrix_tasks(
        &self,
        rules: &[QuadrantRule; 4],
        show_completed: bool,
        completed_limit: usize,
    ) -> anyhow::Result<Vec<Task>> {
        let lists = self.lists()?;
        let archived: HashSet<Id> = lists.iter().filter(|l| l.archived).map(|l| l.id).collect();
        let explicit_lists: HashSet<Id> = rules
            .iter()
            .flat_map(|r| r.lists.iter().flatten().copied())
            .collect();
        let hidden_from_smart: HashSet<Id> = lists
            .iter()
            .filter(|l| !l.archived && !l.in_smart_lists && !explicit_lists.contains(&l.id))
            .map(|l| l.id)
            .collect();

        let mut open = Vec::new();
        let mut completed = Vec::new();
        for task in self.tasks()? {
            if task.is_deleted() || task.is_wont_do() {
                continue;
            }
            if let Some(list_id) = task.list_id
                && (archived.contains(&list_id) || hidden_from_smart.contains(&list_id))
            {
                continue;
            }
            if task.is_completed() {
                if show_completed {
                    completed.push(task);
                }
            } else {
                open.push(task);
            }
        }
        if show_completed && completed.len() > completed_limit {
            completed.sort_by_key(|t| std::cmp::Reverse(t.completed_at.unwrap_or(0)));
            completed.truncate(completed_limit);
        }
        open.extend(completed);
        Ok(open)
    }

    /// Updates saved `matrix_rules` rows when a tag is renamed or merged (`from` -> `into`).
    pub(crate) fn update_matrix_rules_for_tag_rename(&self, from: &str, into: &str) -> anyhow::Result<()> {
        let from_lower = from.to_lowercase();
        let into_lower = into.to_lowercase();
        let mut stmt = self.conn.prepare_cached("SELECT quadrant, rule FROM matrix_rules")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (quadrant, text) in rows {
            if let Ok(index) = quadrant.parse::<usize>()
                && index < 4
                && let Some(mut rule) = QuadrantRule::from_storage(&text)
            {
                let mut changed = false;
                let mut next_tags: Vec<String> = Vec::with_capacity(rule.tags.len());
                for tag in rule.tags {
                    if tag.to_lowercase() == from_lower {
                        changed = true;
                        if !next_tags.iter().any(|t| t.to_lowercase() == into_lower) {
                            next_tags.push(into.to_owned());
                        }
                    } else if !next_tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                        next_tags.push(tag);
                    }
                }
                if changed {
                    rule.tags = next_tags;
                    self.save_matrix_rule(index, Some(&rule))?;
                }
            }
        }
        let mut p_stmt = self.conn.prepare_cached("SELECT id, rules_json FROM matrix_profiles")?;
        let p_rows: Vec<(String, String)> = p_stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        drop(p_stmt);
        for (id, raw_rules) in p_rows {
            let mut rules = decode_profile_rules(&raw_rules);
            let mut any_changed = false;
            for rule in &mut rules {
                let mut changed = false;
                let mut next_tags: Vec<String> = Vec::with_capacity(rule.tags.len());
                for tag in rule.tags.drain(..) {
                    if tag.to_lowercase() == from_lower {
                        changed = true;
                        if !next_tags.iter().any(|t| t.to_lowercase() == into_lower) {
                            next_tags.push(into.to_owned());
                        }
                    } else if !next_tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                        next_tags.push(tag);
                    }
                }
                rule.tags = next_tags;
                any_changed |= changed;
            }
            if any_changed {
                self.conn.execute(
                    "UPDATE matrix_profiles SET rules_json = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, encode_profile_rules(&rules), crate::store::now_ms()],
                )?;
            }
        }
        Ok(())
    }

    /// Removes a deleted tag from any saved `matrix_rules` and `matrix_profiles` rows.
    pub(crate) fn prune_matrix_rules_for_tag_delete(&self, name: &str) -> anyhow::Result<()> {
        let target = name.to_lowercase();
        let mut stmt = self.conn.prepare_cached("SELECT quadrant, rule FROM matrix_rules")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (quadrant, text) in rows {
            if let Ok(index) = quadrant.parse::<usize>()
                && index < 4
                && let Some(mut rule) = QuadrantRule::from_storage(&text)
            {
                let before = rule.tags.len();
                rule.tags.retain(|t| t.to_lowercase() != target);
                if rule.tags.len() != before {
                    self.save_matrix_rule(index, Some(&rule))?;
                }
            }
        }
        let mut p_stmt = self.conn.prepare_cached("SELECT id, rules_json FROM matrix_profiles")?;
        let p_rows: Vec<(String, String)> = p_stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        drop(p_stmt);
        for (id, raw_rules) in p_rows {
            let mut rules = decode_profile_rules(&raw_rules);
            let mut any_changed = false;
            for rule in &mut rules {
                let before = rule.tags.len();
                rule.tags.retain(|t| t.to_lowercase() != target);
                any_changed |= rule.tags.len() != before;
            }
            if any_changed {
                self.conn.execute(
                    "UPDATE matrix_profiles SET rules_json = ?2, updated_at = ?3 WHERE id = ?1",
                    rusqlite::params![id, encode_profile_rules(&rules), crate::store::now_ms()],
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tasks_land_in_their_quadrant() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let [q1, q2, q3, q4] = QuadrantRule::defaults();
        assert_eq!(q1.new_task_defaults(today), (Some(Priority::High), None));
        assert_eq!(q2.new_task_defaults(today), (Some(Priority::Medium), None));
        assert_eq!(q3.new_task_defaults(today), (Some(Priority::Low), None));
        assert_eq!(q4.new_task_defaults(today), (Some(Priority::None), None));

        // Preferred priority is deterministic regardless of chip click order.
        let unordered = QuadrantRule {
            priorities: vec![Priority::Medium, Priority::High],
            dates: vec![MatrixDate::Today],
            ..QuadrantRule::default()
        };
        assert_eq!(unordered.new_task_defaults(today), (Some(Priority::High), Some(today)));
    }

    #[test]
    fn default_quadrants_and_date_template() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let rules = store.matrix_rules()?;
        let task = |priority: Priority, due: Option<NaiveDate>| {
            let task = store.add_task("t").unwrap();
            store.set_priority(task.id, priority).unwrap();
            store.set_due(task.id, due, None).unwrap();
            store.task(task.id).unwrap().unwrap()
        };
        assert_eq!(quadrant_of(&rules, &task(Priority::High, None), today), Some(0));
        assert_eq!(quadrant_of(&rules, &task(Priority::Medium, None), today), Some(1));
        assert_eq!(quadrant_of(&rules, &task(Priority::Low, None), today), Some(2));
        assert_eq!(quadrant_of(&rules, &task(Priority::None, None), today), Some(3));

        let tpl = store.apply_matrix_date_template()?;
        let tomorrow = Some(today + Duration::days(1));
        let next_week = Some(today + Duration::days(7));
        assert_eq!(quadrant_of(&tpl, &task(Priority::High, tomorrow), today), Some(0));
        assert_eq!(quadrant_of(&tpl, &task(Priority::Medium, next_week), today), Some(1));
        assert_eq!(quadrant_of(&tpl, &task(Priority::Low, Some(today - Duration::days(3))), today), Some(2));
        assert_eq!(quadrant_of(&tpl, &task(Priority::None, None), today), Some(3));
        Ok(())
    }

    #[test]
    fn drop_changes_adjusts_priority_date_list_tags_and_kind() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let work = store.create_list("Work")?;
        let task = store.add_task("Draft")?;
        store.set_priority(task.id, Priority::Low)?;
        store.set_due(task.id, Some(today), None)?;
        let task = store.task(task.id)?.unwrap();

        let target = QuadrantRule {
            priorities: vec![Priority::High, Priority::Medium],
            dates: vec![MatrixDate::NoDate, MatrixDate::Later],
            lists: vec![Some(work.id)],
            tags: vec!["focus".into()],
            ..QuadrantRule::default()
        };
        let outcome = target.drop_changes(&task, today, &[work.id], &HashMap::new());
        assert_eq!(
            outcome,
            DropOutcome::Apply(DropPatch {
                priority: Some(Priority::High),
                due_date: Some(None),
                list_id: Some(Some(work.id)),
                add_tag: Some("focus".into()),
                kind: None,
            })
        );
        Ok(())
    }

    #[test]
    fn tag_rename_and_delete_cascade_to_matrix_rules() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let t = store.add_task("Tagged")?;
        store.set_tags(t.id, &["OldTag".into()])?;
        let rule = QuadrantRule { tags: vec!["OldTag".into()], ..QuadrantRule::default() };
        store.save_matrix_rule(0, Some(&rule))?;

        store.update_tag("OldTag", "NewTag", None, None)?;
        assert_eq!(store.matrix_rules()?[0].tags, ["NewTag"]);

        store.delete_tag("NewTag")?;
        assert!(store.matrix_rules()?[0].tags.is_empty());
        Ok(())
    }

    #[test]
    fn edited_rules_are_kept() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let list = store.create_list("Work")?;
        let rule = QuadrantRule {
            name: "Do now".into(),
            lists: vec![Some(list.id), None],
            tags: vec!["urgent".into()],
            dates: vec![MatrixDate::Today, MatrixDate::DaysLater(5)],
            priorities: vec![Priority::High],
            kind: MatrixKind::TaskOnly,
        };
        store.save_matrix_rule(0, Some(&rule))?;
        assert_eq!(store.matrix_rules()?[0], rule);
        assert_eq!(store.matrix_rules()?[1], QuadrantRule::defaults()[1]);
        store.save_matrix_rule(0, None)?;
        assert_eq!(store.matrix_rules()?[0], QuadrantRule::defaults()[0]);
        assert!(store.save_matrix_rule(4, None).is_err());
        Ok(())
    }

    #[test]
    fn matrix_profiles_crud_and_tag_cascade() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        // First profile seeds from legacy matrix_rules.
        let legacy = QuadrantRule { name: "Legacy Q1".into(), ..QuadrantRule::default() };
        store.save_matrix_rule(0, Some(&legacy))?;
        let profiles = store.matrix_profiles()?;
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].rules[0].name, "Legacy Q1");
        assert_eq!(store.active_matrix_profile_id()?, profiles[0].id);

        // Cannot delete the sole profile.
        assert!(!store.delete_matrix_profile(profiles[0].id)?);

        // Create second profile, activate, rename, edit rule with tag, verify tag rename/delete cascades.
        let work = store.create_matrix_profile("Work Mode", None)?;
        store.set_active_matrix_profile(work.id)?;
        assert_eq!(store.active_matrix_profile_id()?, work.id);

        store.rename_matrix_profile(work.id, "Deep Work")?;
        let rule = QuadrantRule { tags: vec!["Alpha".into()], ..QuadrantRule::default() };
        store.save_matrix_profile_rule(work.id, 1, Some(&rule))?;

        let t = store.add_task("Task")?;
        store.set_tags(t.id, &["Alpha".into()])?;
        store.update_tag("Alpha", "Beta", None, None)?;

        let all = store.matrix_profiles()?;
        let updated = all.iter().find(|p| p.id == work.id).unwrap();
        assert_eq!(updated.name, "Deep Work");
        assert_eq!(updated.rules[1].tags, ["Beta"]);

        store.delete_tag("Beta")?;
        let after_del_tag = store.matrix_profiles()?.into_iter().find(|p| p.id == work.id).unwrap();
        assert!(after_del_tag.rules[1].tags.is_empty());

        // Deleting active second profile switches back to first.
        assert!(store.delete_matrix_profile(work.id)?);
        assert_eq!(store.active_matrix_profile_id()?, profiles[0].id);
        Ok(())
    }
}
