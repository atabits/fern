//! Saved filters. Each clause owns its value; date selections within a clause are ORed.
//! Storage is versioned and rejects unknown rules instead of silently selecting all tasks.

use crate::model::{Id, Kind, Priority, Task};
use anyhow::{Context, Result, bail, ensure};
use chrono::{Datelike, NaiveDate, Timelike, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateRule {
    All,
    Overdue,
    Today,
    Tomorrow,
    ThisWeek,
    NoDate,
    NextWeek,
    ThisMonth,
    NextMonth,
    NextDays(u32),
    Recurring,
    /// Inclusive day offsets from today; an absent endpoint is unbounded.
    Span {
        from: Option<i32>,
        to: Option<i32>,
    },
}

impl DateRule {
    pub const OPTIONS: [Self; 12] = [
        Self::All,
        Self::NoDate,
        Self::Overdue,
        Self::Recurring,
        Self::Today,
        Self::Tomorrow,
        Self::ThisWeek,
        Self::NextWeek,
        Self::ThisMonth,
        Self::NextMonth,
        Self::NextDays(7),
        Self::Span {
            from: Some(0),
            to: Some(6),
        },
    ];

    pub fn from_key(key: &str) -> Result<Self> {
        Ok(match key {
            "all" | "" => Self::All,
            "overdue" => Self::Overdue,
            "today" => Self::Today,
            "tomorrow" => Self::Tomorrow,
            "week" | "thisweek" => Self::ThisWeek,
            "none" | "nodue" => Self::NoDate,
            "next_week" | "nextweek" => Self::NextWeek,
            "this_month" | "thismonth" => Self::ThisMonth,
            "next_month" | "offset(1M)" => Self::NextMonth,
            "recurring" => Self::Recurring,
            _ if key.starts_with("next_days:") => {
                Self::NextDays(Self::parse_days(&key[10..]).context("invalid day count")?)
            }
            _ => bail!("unknown date rule: {key}"),
        })
    }

    pub fn parse_days(text: &str) -> Option<u32> {
        let text = text.trim();
        if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        text.parse().ok().filter(|&days| days > 0)
    }

    /// Accepts offsets -30..=30. An empty endpoint leaves that side unbounded.
    pub fn parse_span(from: &str, to: &str) -> Result<Self> {
        let parse = |s: &str| -> Result<Option<i32>> {
            if s.trim().is_empty() {
                return Ok(None);
            }
            let n = s
                .trim()
                .parse::<i32>()
                .context("enter a whole day offset")?;
            ensure!(
                (-30..=30).contains(&n),
                "day offsets must be between -30 and 30"
            );
            Ok(Some(n))
        };
        let value = Self::Span {
            from: parse(from)?,
            to: parse(to)?,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(self) -> Result<()> {
        match self {
            Self::NextDays(n) => ensure!(n > 0, "a date range needs at least one day"),
            Self::Span { from, to } => {
                ensure!(
                    from.into_iter().chain(to).all(|n| (-30..=30).contains(&n)),
                    "day offsets must be between -30 and 30"
                );
                ensure!(
                    !matches!((from, to), (Some(a), Some(b)) if a > b),
                    "range start must not exceed range end"
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn matches_legacy(self, due: Option<NaiveDate>, today: NaiveDate) -> Option<bool> {
        use DateRule::*;
        Some(match self {
            All => true,
            NoDate => due.is_none(),
            Overdue => due.is_some_and(|d| d < today),
            Today => due == Some(today),
            Tomorrow => due.is_some_and(|d| (d - today).num_days() == 1),
            ThisWeek => {
                let monday = i64::from(today.weekday().num_days_from_monday());
                due.is_some_and(|d| (0..7).contains(&((d - today).num_days() + monday)))
            }
            _ => return None,
        })
    }

    // Relative seconds avoid overflowing chrono's date limits when adding a month or many days.
    fn bounds(self, today: NaiveDate, week_start: Weekday) -> Option<(Option<i64>, Option<i64>)> {
        let day = 86_400;
        let back = i64::from(
            (7 + today.weekday().num_days_from_monday() - week_start.num_days_from_monday()) % 7,
        );
        let month_start = -i64::from(today.day0());
        let month_days = |year, month| {
            let first = NaiveDate::from_ymd_opt(year, month, 1)?;
            let next = if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)
            };
            // The last representable month is December and has 31 days.
            Some(next.map_or(31, |n| (n - first).num_days()))
        };
        let (from, until) = match self {
            Self::Today => (Some(0), Some(1)),
            Self::Tomorrow => (Some(1), Some(2)),
            Self::ThisWeek => (Some(-back), Some(7 - back)),
            Self::NextWeek => (Some(7 - back), Some(14 - back)),
            Self::ThisMonth => (
                Some(month_start),
                Some(month_start + month_days(today.year(), today.month())?),
            ),
            Self::NextMonth => {
                let from = month_start + month_days(today.year(), today.month())?;
                let (year, month) = if today.month() == 12 {
                    (today.year() + 1, 1)
                } else {
                    (today.year(), today.month() + 1)
                };
                (
                    Some(from),
                    Some(from + month_days(year, month).unwrap_or(31)),
                )
            }
            Self::NextDays(n) if n > 0 => (Some(0), Some(i64::from(n))),
            Self::Span { from, to } => (from.map(i64::from), to.map(|n| i64::from(n) + 1)),
            _ => return None,
        };
        Some((from.map(|n| n * day), until.map(|n| n * day)))
    }

    fn matches(
        self,
        task: &Task,
        today: NaiveDate,
        week_start: Weekday,
        has_note_type: bool,
        negated: bool,
    ) -> bool {
        if self == Self::All {
            return true;
        }
        if self == Self::Recurring {
            return task.repeat.is_some();
        }
        // Exclude past-dated notes from date predicates unless a Note type clause is present.
        // Undated notes can still match No Date.
        if task.kind == Kind::Note
            && task.due_date.is_some_and(|d| d < today)
            && (matches!(
                self,
                Self::NoDate
                    | Self::Span {
                        from: None,
                        to: None
                    }
            ) || !has_note_type)
        {
            return negated;
        }
        if self == Self::NoDate {
            return task.due_date.is_none();
        }
        let Some(start) = task.due_date else {
            return false;
        };
        let seconds = |d: NaiveDate, t: Option<chrono::NaiveTime>| {
            (d - today).num_days() * 86_400
                + t.map_or(0, |t| i64::from(t.num_seconds_from_midnight()))
        };
        let start = seconds(start, task.due_time);
        let end = task
            .end_date
            .map(|d| seconds(d, task.end_time) + if task.end_time.is_none() { 86_400 } else { 0 })
            .filter(|&e| e > start);
        if self == Self::Overdue {
            return end.map_or(start < 0, |e| e <= 0);
        }
        let Some((from, until)) = self.bounds(today, week_start) else {
            return false;
        };
        // With only an upper bound, the whole task must finish by that date.
        if let Self::Span {
            from: None,
            to: Some(_),
        } = self
        {
            return until.is_some_and(|hi| end.map_or(start < hi, |end| end <= hi));
        }
        if matches!(
            self,
            Self::Span {
                from: Some(_),
                to: None
            }
        ) && task.kind == Kind::Note
            && !has_note_type
            && from.is_some_and(|lo| start < lo)
        {
            return false;
        }
        match end {
            Some(end) => from.is_none_or(|lo| end > lo) && until.is_none_or(|hi| start < hi),
            None => from.is_none_or(|lo| start >= lo) && until.is_none_or(|hi| start < hi),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Lists,
    Tags,
    Date,
    Priority,
    Include,
    Type,
}
impl Field {
    pub const ALL: [Self; 6] = [
        Self::Lists,
        Self::Tags,
        Self::Date,
        Self::Priority,
        Self::Include,
        Self::Type,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Lists => "lists",
            Self::Tags => "tags",
            Self::Date => "date",
            Self::Priority => "priority",
            Self::Include => "include",
            Self::Type => "type",
        }
    }
    fn from_key(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|f| f.key() == s)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Join {
    #[default]
    And,
    Or,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", content = "value", rename_all = "snake_case")]
pub enum ConditionValue {
    Lists(Vec<Option<Id>>),
    Tags { names: Vec<String>, untagged: bool },
    Date(Vec<DateRule>),
    Priority(Vec<Priority>),
    Include(String),
    Type(Option<Kind>),
}
impl ConditionValue {
    pub fn field(&self) -> Field {
        match self {
            Self::Lists(_) => Field::Lists,
            Self::Tags { .. } => Field::Tags,
            Self::Date(_) => Field::Date,
            Self::Priority(_) => Field::Priority,
            Self::Include(_) => Field::Include,
            Self::Type(_) => Field::Type,
        }
    }
    pub fn empty(field: Field) -> Self {
        match field {
            Field::Lists => Self::Lists(vec![]),
            Field::Tags => Self::Tags {
                names: vec![],
                untagged: false,
            },
            Field::Date => Self::Date(vec![]),
            Field::Priority => Self::Priority(vec![]),
            Field::Include => Self::Include(String::new()),
            Field::Type => Self::Type(None),
        }
    }
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Lists(v) => v.is_empty(),
            Self::Tags { names, untagged } => names.is_empty() && !untagged,
            Self::Date(v) => v.is_empty() || v.contains(&DateRule::All),
            Self::Priority(v) => v.is_empty(),
            Self::Include(s) => s.trim().is_empty(),
            Self::Type(k) => k.is_none(),
        }
    }
    fn validate(&self) -> Result<()> {
        if let Self::Date(dates) = self {
            let mut has_range = false;
            for date in dates {
                date.validate()?;
                if matches!(date, DateRule::NextDays(_) | DateRule::Span { .. }) {
                    ensure!(!has_range, "a date condition can only have one custom range");
                    has_range = true;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub join: Join,
    pub not: bool,
    pub value: ConditionValue,
    /// Preserve the date and keyword matching of saved filters from earlier releases.
    #[serde(default)]
    pub legacy_matching: bool,
}
impl Condition {
    pub fn new(value: ConditionValue) -> Self {
        Self {
            join: Join::And,
            not: false,
            value,
            legacy_matching: false,
        }
    }
    pub fn empty(field: Field) -> Self {
        Self::new(ConditionValue::empty(field))
    }
    fn matches(
        &self,
        task: &Task,
        today: NaiveDate,
        week_start: Weekday,
        has_note_type: bool,
    ) -> bool {
        let met = match &self.value {
            ConditionValue::Lists(v) => v.is_empty() || v.contains(&task.list_id),
            ConditionValue::Tags { names, untagged } => {
                (names.is_empty() && !untagged)
                    || (*untagged && task.tags.is_empty())
                    || names.iter().any(|wanted| {
                        task.tags
                            .iter()
                            .any(|tag| tag.to_lowercase() == wanted.to_lowercase())
                    })
            }
            ConditionValue::Date(v) => {
                v.is_empty()
                    || v.iter().any(|d| {
                        if self.legacy_matching
                            && let Some(matches) = d.matches_legacy(task.due_date, today)
                        {
                            return matches;
                        }
                        d.matches(task, today, week_start, has_note_type, self.not)
                    })
            }
            ConditionValue::Priority(v) => v.is_empty() || v.contains(&task.priority),
            ConditionValue::Include(s) => {
                let text = format!("{}\n{}", task.title, task.notes).to_lowercase();
                let matches = |word: &str| text.contains(&word.to_lowercase());
                if self.legacy_matching {
                    s.split_whitespace().all(matches)
                } else {
                    s.trim().is_empty() || s.split_whitespace().any(matches)
                }
            }
            ConditionValue::Type(k) => k.is_none_or(|k| k == task.kind),
        };
        met != self.not
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterRules {
    pub version: u32,
    pub advanced: bool,
    pub conditions: Vec<Condition>,
}
impl Default for FilterRules {
    fn default() -> Self {
        Self {
            version: 1,
            advanced: false,
            conditions: Field::ALL.into_iter().map(Condition::empty).collect(),
        }
    }
}
impl FilterRules {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported filter version: {}",
            self.version
        );
        for c in &self.conditions {
            c.value.validate()?;
        }
        if !self.advanced {
            let mut fields = std::collections::HashSet::new();
            ensure!(
                self.conditions
                    .iter()
                    .all(|c| c.join == Join::And && !c.not && fields.insert(c.value.field())),
                "normal filters need unique AND conditions"
            );
        }
        Ok(())
    }
    pub fn from_storage(text: &str) -> Result<Self> {
        let rules: Self = serde_json::from_str(text).context("invalid saved filter")?;
        rules.validate()?;
        Ok(rules)
    }
    pub fn to_storage(&self) -> Result<String> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }
    /// Refuse a lossy Advanced -> Normal conversion. The editor retains the current rules.
    pub fn set_advanced(&mut self, advanced: bool) -> Result<()> {
        if advanced == self.advanced {
            return Ok(());
        }
        if advanced {
            self.conditions.retain(|c| !c.value.is_empty());
            if self.conditions.is_empty() {
                self.conditions.push(Condition::empty(Field::Lists));
            }
        } else {
            let mut normal = Self::default();
            let mut seen = std::collections::HashSet::new();
            for (i, c) in self.conditions.iter().enumerate() {
                ensure!(
                    !c.not && (i == 0 || c.join == Join::And) && seen.insert(c.value.field()),
                    "these advanced conditions cannot be represented in normal mode"
                );
                let mut condition = c.clone();
                condition.join = Join::And;
                *normal
                    .conditions
                    .iter_mut()
                    .find(|n| n.value.field() == c.value.field())
                    .unwrap() = condition;
            }
            self.conditions = normal.conditions;
        }
        self.advanced = advanced;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmartFilter {
    pub id: Id,
    pub name: String,
    pub rules: FilterRules,
}
impl SmartFilter {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::now_v7(),
            ..Self::default()
        }
    }
    pub fn matches(&self, task: &Task, today: NaiveDate, week_start: Weekday) -> bool {
        if task.is_deleted() || task.is_closed() {
            return false;
        }
        let has_note_type = self
            .rules
            .conditions
            .iter()
            .any(|c| c.value == ConditionValue::Type(Some(Kind::Note)));
        let mut any = false;
        let mut group = true;
        for (i, c) in self.rules.conditions.iter().enumerate() {
            if i > 0 && c.join == Join::Or {
                any |= group;
                group = true;
            }
            group &= c.matches(task, today, week_start, has_note_type);
        }
        any || group
    }
    pub fn preview<'a>(
        &self,
        tasks: &'a [Task],
        today: NaiveDate,
        week_start: Weekday,
    ) -> Vec<&'a Task> {
        tasks
            .iter()
            .filter(|t| self.matches(t, today, week_start))
            .collect()
    }
}

/// Expands legacy shared-value links into independent clauses without changing their meaning.
pub fn from_legacy(
    values: Vec<ConditionValue>,
    match_any: bool,
    chain: &str,
) -> Result<FilterRules> {
    let legacy = |value: ConditionValue, join, not| Condition {
        legacy_matching: matches!(value.field(), Field::Date | Field::Include) && !value.is_empty(),
        value,
        join,
        not,
    };
    let links: Vec<_> = chain
        .split_whitespace()
        .filter_map(|part| {
            let (join, rest) = match part.split_once(':') {
                Some(("or", rest)) => (Join::Or, rest),
                Some((_, rest)) => (Join::And, rest),
                None => (Join::And, part),
            };
            let (key, not) = part_not(rest);
            Some((join, Field::from_key(key)?, not))
        })
        .collect();
    let advanced = match_any || !links.is_empty();
    let conditions = if links.is_empty() {
        values
            .into_iter()
            .filter(|v| !advanced || !v.is_empty())
            .map(|value| legacy(value, if match_any { Join::Or } else { Join::And }, false))
            .collect()
    } else {
        links
            .into_iter()
            .map(|(join, field, not)| {
                let value = values
                    .iter()
                    .find(|v| v.field() == field)
                    .cloned()
                    .context("missing legacy condition")?;
                Ok(legacy(value, join, not))
            })
            .collect::<Result<Vec<_>>>()?
    };
    let rules = FilterRules {
        version: 1,
        advanced,
        conditions,
    };
    rules.validate()?;
    Ok(rules)
}
fn part_not(s: &str) -> (&str, bool) {
    s.strip_suffix('!').map_or((s, false), |s| (s, true))
}

pub fn examples() -> [SmartFilter; 3] {
    use ConditionValue::*;
    let make = |conditions| SmartFilter {
        rules: FilterRules {
            advanced: true,
            conditions,
            ..FilterRules::default()
        },
        ..SmartFilter::default()
    };
    [
        make(vec![
            Condition::new(Date(vec![DateRule::Today])),
            Condition::new(Priority(vec![
                crate::Priority::High,
                crate::Priority::Medium,
            ])),
        ]),
        make(vec![
            Condition::new(Date(vec![DateRule::Overdue])),
            Condition {
                join: Join::Or,
                ..Condition::new(Priority(vec![crate::Priority::High]))
            },
        ]),
        make(vec![
            Condition {
                not: true,
                ..Condition::new(Lists(vec![None]))
            },
            Condition::new(Date(vec![DateRule::NoDate])),
            Condition::new(Type(Some(Kind::Task))),
        ]),
    ]
}

#[cfg(test)]
mod tests;
