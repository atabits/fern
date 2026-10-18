//! The Eisenhower Matrix's quadrants and their rules, as the original's "Edit Matrix" sets them:
//! lists, tags, dates and priorities. A task goes into the first quadrant whose rule it meets.

use chrono::{Duration, NaiveDate};

use crate::model::{Id, Priority, Task};
use crate::store::Store;

/// The rule's "Date" choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixDate {
    Overdue,
    Today,
    Tomorrow,
    /// The original's "The next 2nd day ~": from the day after tomorrow on.
    Later,
    NoDate,
}

impl MatrixDate {
    pub const ALL: [Self; 5] = [Self::Overdue, Self::Today, Self::Tomorrow, Self::Later, Self::NoDate];

    fn key(self) -> &'static str {
        ["overdue", "today", "tomorrow", "later", "none"][self as usize]
    }

    fn matches(self, due: Option<NaiveDate>, today: NaiveDate) -> bool {
        match (self, due) {
            (Self::NoDate, due) => due.is_none(),
            (_, None) => false,
            (Self::Overdue, Some(d)) => d < today,
            (Self::Today, Some(d)) => d == today,
            (Self::Tomorrow, Some(d)) => d == today + Duration::days(1),
            (Self::Later, Some(d)) => d > today + Duration::days(1),
        }
    }
}

/// One quadrant's rule. An empty condition matches everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuadrantRule {
    /// The quadrant's own name; empty keeps the default title.
    pub name: String,
    /// Lists to include (`None` is the Inbox).
    pub lists: Vec<Option<Id>>,
    pub tags: Vec<String>,
    pub dates: Vec<MatrixDate>,
    pub priorities: Vec<Priority>,
}

impl QuadrantRule {
    /// The original's rules: urgent is overdue, today or tomorrow; important is high or medium
    /// priority.
    pub fn defaults() -> [Self; 4] {
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

    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        (self.lists.is_empty() || self.lists.contains(&task.list_id))
            && (self.tags.is_empty()
                || self.tags.iter().any(|tag| task.tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase())))
            && (self.dates.is_empty() || self.dates.iter().any(|d| d.matches(task.due_date, today)))
            && (self.priorities.is_empty() || self.priorities.contains(&task.priority))
    }

    fn to_storage(&self) -> String {
        let lists: Vec<String> = self.lists.iter().map(|l| l.map_or("inbox".to_owned(), |id| id.to_string())).collect();
        let dates: Vec<&str> = self.dates.iter().map(|d| d.key()).collect();
        let priorities: Vec<String> = self.priorities.iter().map(|p| p.to_i64().to_string()).collect();
        serde_json::json!({
            "name": self.name,
            "lists": lists,
            "tags": self.tags,
            "dates": dates,
            "priorities": priorities,
        })
        .to_string()
    }

    fn from_storage(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let strings = |key: &str| -> Vec<String> {
            value[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
        };
        Some(Self {
            name: value["name"].as_str().unwrap_or_default().to_owned(),
            lists: strings("lists").iter().map(|l| if l == "inbox" { Some(None) } else { l.parse().ok().map(Some) }).flatten().collect(),
            tags: strings("tags"),
            dates: strings("dates").iter().filter_map(|k| MatrixDate::ALL.into_iter().find(|d| d.key() == k)).collect(),
            priorities: strings("priorities").iter().filter_map(|p| p.parse().ok()).map(Priority::from_i64).collect(),
        })
    }
}

/// The quadrant (0–3) a task goes into: the first whose rule it meets.
pub fn quadrant_of(rules: &[QuadrantRule; 4], task: &Task, today: NaiveDate) -> Option<usize> {
    rules.iter().position(|rule| rule.matches(task, today))
}

impl Store {
    /// The four quadrants' rules: the saved ones, the original's defaults otherwise.
    pub fn matrix_rules(&self) -> anyhow::Result<[QuadrantRule; 4]> {
        let mut rules = QuadrantRule::defaults();
        for (i, rule) in rules.iter_mut().enumerate() {
            if let Some(saved) = self.setting(&format!("matrix_rule_{i}"))?.and_then(|t| QuadrantRule::from_storage(&t)) {
                *rule = saved;
            }
        }
        Ok(rules)
    }

    /// Saves a quadrant's rule; `None` restores the original's default ("Restore").
    pub fn save_matrix_rule(&self, quadrant: usize, rule: Option<&QuadrantRule>) -> anyhow::Result<()> {
        anyhow::ensure!(quadrant < 4, "there are four quadrants");
        let key = format!("matrix_rule_{quadrant}");
        match rule {
            Some(rule) => self.set_setting(&key, &rule.to_storage()),
            None => self.delete_setting(&key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_quadrants() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let rules = store.matrix_rules()?;
        let task = |priority: Priority, due: Option<NaiveDate>| {
            let task = store.add_task("t").unwrap();
            store.set_priority(task.id, priority).unwrap();
            store.set_due(task.id, due, None).unwrap();
            store.task(task.id).unwrap().unwrap()
        };
        let tomorrow = Some(today + Duration::days(1));
        let next_week = Some(today + Duration::days(7));
        assert_eq!(quadrant_of(&rules, &task(Priority::High, tomorrow), today), Some(0));
        assert_eq!(quadrant_of(&rules, &task(Priority::Medium, next_week), today), Some(1));
        assert_eq!(quadrant_of(&rules, &task(Priority::Low, Some(today - Duration::days(3))), today), Some(2));
        assert_eq!(quadrant_of(&rules, &task(Priority::None, None), today), Some(3));
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
            dates: vec![MatrixDate::Today],
            priorities: vec![Priority::High],
        };
        store.save_matrix_rule(0, Some(&rule))?;
        assert_eq!(store.matrix_rules()?[0], rule);
        assert_eq!(store.matrix_rules()?[1], QuadrantRule::defaults()[1]);
        store.save_matrix_rule(0, None)?;
        assert_eq!(store.matrix_rules()?[0], QuadrantRule::defaults()[0], "Restore brings the default back");
        assert!(store.save_matrix_rule(4, None).is_err());
        Ok(())
    }
}
