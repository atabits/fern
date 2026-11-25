use std::collections::HashMap;

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use crate::model::{Id, Kind, Priority};

use super::{Store, now_ms};

const FILTER_ORDER_KEY: &str = "filter_order";

impl Store {
    /// Saved filters, in custom sidebar order (falling back to creation order).
    pub fn filters(&self) -> Result<Vec<crate::filter::SmartFilter>> {
        use crate::filter::{ConditionValue as V, DateRule, FilterRules, SmartFilter};
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, lists, tags, date, priorities, keywords, kind, match_any, chain, rules FROM filters ORDER BY created_at, rowid",
        )?;
        let mut filters: Vec<SmartFilter> = stmt.query_map([], |row| {
            let read = || -> Result<SmartFilter> {
                let rules = match row.get::<_, Option<String>>(10)? {
                    Some(text) => FilterRules::from_storage(&text)?,
                    None => {
                        let lists: String = row.get(2)?;
                        let tags: String = row.get(3)?;
                        let priorities: String = row.get(5)?;
                        let date = DateRule::from_key(&row.get::<_, String>(4)?).unwrap_or(DateRule::All);
                        let values = vec![
                            V::Lists(lists.split(',').filter(|s| !s.is_empty()).map(|s| Uuid::parse_str(s).ok()).collect()),
                            V::Tags { names: tags.lines().filter(|s| !s.is_empty()).map(str::to_owned).collect(), untagged: false },
                            V::Date(if date == DateRule::All { vec![] } else { vec![date] }),
                            V::Priority(priorities.split(',').filter_map(|s| s.parse().ok()).map(Priority::from_i64).collect()),
                            V::Include(row.get(6)?),
                            V::Type(row.get::<_, Option<String>>(7)?.map(|k| Kind::parse(&k))),
                        ];
                        crate::filter::from_legacy(values, row.get(8)?, &row.get::<_, String>(9)?)?
                    }
                };
                Ok(SmartFilter { id: Uuid::parse_str(&row.get::<_, String>(0)?)?, name: row.get(1)?, rules })
            };
            read().map_err(|e| rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, e.into()))
        })?.collect::<Result<_, _>>()?;
        if let Some(saved) = self.setting(FILTER_ORDER_KEY)? {
            let mut ranks: HashMap<Id, usize> = HashMap::new();
            for (i, id) in saved.split(',').filter_map(|s| Uuid::parse_str(s.trim()).ok()).enumerate() {
                ranks.entry(id).or_insert(i);
            }
            filters.sort_by_key(|f| ranks.get(&f.id).copied().unwrap_or(usize::MAX));
        }
        Ok(filters)
    }

    /// Adds or replaces a filter. Legacy columns remain for reading older databases and sync rows.
    pub fn save_filter(&self, filter: &crate::filter::SmartFilter) -> Result<()> {
        anyhow::ensure!(!filter.name.trim().is_empty(), "a filter needs a name");
        let rules = filter.rules.to_storage()?;
        self.conn.execute(
            "INSERT INTO filters (id, name, lists, tags, date, priorities, keywords, kind, created_at, match_any, chain, rules)
             VALUES (?1, ?2, '', '', 'all', '', '', NULL, ?3, 0, '', ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, rules = excluded.rules",
            params![filter.id.to_string(), filter.name.trim(), now_ms(), rules],
        )?;
        Ok(())
    }

    /// Persists the saved-filter display order in a single write.
    pub fn set_filter_order(&self, order: &[Id]) -> Result<()> {
        let value: Vec<String> = order.iter().map(Id::to_string).collect();
        self.set_setting(FILTER_ORDER_KEY, &value.join(","))?;
        Ok(())
    }

    /// Reorders a saved filter before (`!after`) or after (`after`) `target`.
    pub fn move_filter(&self, dragged: Id, target: Id, after: bool) -> Result<()> {
        if dragged == target {
            return Ok(());
        }
        self.in_tx(|| {
            let mut order: Vec<Id> = self.filters()?.into_iter().map(|f| f.id).collect();
            let Some(from) = order.iter().position(|&id| id == dragged) else { return Ok(()) };
            order.remove(from);
            let Some(to) = order.iter().position(|&id| id == target) else { return Ok(()) };
            order.insert(to + usize::from(after), dragged);
            self.set_filter_order(&order)
        })
    }

    /// Moves a saved filter to the top of the Filters section.
    pub fn move_filter_first(&self, dragged: Id) -> Result<()> {
        self.in_tx(|| {
            let mut order: Vec<Id> = self.filters()?.into_iter().map(|f| f.id).collect();
            if !order.contains(&dragged) {
                return Ok(());
            }
            order.retain(|&id| id != dragged);
            order.insert(0, dragged);
            self.set_filter_order(&order)
        })
    }

    pub fn delete_filter(&self, id: Id) -> Result<()> {
        self.in_tx(|| {
            self.conn.execute("DELETE FROM filters WHERE id = ?1", [id.to_string()])?;
            if let Some(saved) = self.setting(FILTER_ORDER_KEY)? {
                let id_str = id.to_string();
                let updated: Vec<&str> = saved
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != id_str)
                    .collect();
                self.set_setting(FILTER_ORDER_KEY, &updated.join(","))?;
            }
            Ok(())
        })
    }
}
