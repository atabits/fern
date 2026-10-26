//! Events of a subscribed calendar (the original's "URL" calendars): the `VEVENT`s of an
//! iCalendar (RFC 5545) feed, with their repeats laid out over the days shown.
//!
//! Only what a calendar needs to draw is read: the title, the start and end (a whole day or a
//! time), the place, the description and the repeat rule with its `UNTIL`, `COUNT` and
//! `EXDATE`s; a changed occurrence (`RECURRENCE-ID`) replaces the repeat's one. Times in UTC
//! are moved to the local time; times with a `TZID` are taken as local, there being no time
//! zone database here.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

use crate::recurrence::Recurrence;

/// A subscribed calendar and the last feed read from its address.
#[derive(Clone, Debug, PartialEq)]
pub struct Subscription {
    pub id: crate::model::Id,
    pub name: String,
    pub url: String,
    pub color: Option<String>,
    pub hidden: bool,
    pub feed: String,
    pub fetched_at: Option<i64>,
    pub error: Option<String>,
}

impl Subscription {
    /// Its name, or the address's host while it has none.
    pub fn title(&self) -> &str {
        if !self.name.trim().is_empty() {
            return self.name.trim();
        }
        let rest = self.url.split_once("://").map_or(self.url.as_str(), |(_, rest)| rest);
        rest.split('/').next().unwrap_or(rest)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub uid: String,
    pub title: String,
    pub start: NaiveDate,
    /// `None` for an all-day event.
    pub start_time: Option<NaiveTime>,
    /// The last day the event covers (for an all-day event, the day before `DTEND`).
    pub end: NaiveDate,
    pub end_time: Option<NaiveTime>,
    pub location: String,
    pub notes: String,
    pub repeat: Option<Recurrence>,
    until: Option<NaiveDate>,
    count: Option<u32>,
    except: HashSet<NaiveDate>,
}

impl Event {
    pub fn all_day(&self) -> bool {
        self.start_time.is_none()
    }

    /// How many days the event covers after its first.
    fn extra_days(&self) -> i64 {
        (self.end - self.start).num_days().max(0)
    }

    /// The first days of the event's occurrences that cover any day from `from` to `to`.
    pub fn starts_in(&self, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        let reach = from - Duration::days(self.extra_days());
        let Some(rule) = &self.repeat else {
            return (self.start <= to && self.start >= reach).then_some(self.start).into_iter().collect();
        };
        let mut starts = Vec::new();
        let mut day = self.start;
        let mut seen = 0;
        // A bound for broken rules: no feed repeats more than daily for this long.
        for _ in 0..20_000 {
            if day > to || self.until.is_some_and(|until| day > until) || self.count.is_some_and(|count| seen >= count) {
                break;
            }
            seen += 1;
            if day >= reach && !self.except.contains(&day) {
                starts.push(day);
            }
            let next = rule.next_after(day);
            if next <= day {
                break;
            }
            day = next;
        }
        starts
    }
}

/// The events of an iCalendar text; unreadable ones are left out.
pub fn parse(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut current: Option<Vec<(String, String)>> = None;
    for line in unfold(text) {
        match line.as_str() {
            "BEGIN:VEVENT" => current = Some(Vec::new()),
            "END:VEVENT" => {
                if let Some(event) = current.take().and_then(|props| event(&props)) {
                    events.push(event);
                }
            }
            _ => {
                if let (Some(props), Some((name, value))) = (current.as_mut(), line.split_once(':')) {
                    props.push((name.to_owned(), value.to_owned()));
                }
            }
        }
    }
    // A changed occurrence takes the place of the repeat's one.
    let mut moved: HashMap<String, Vec<NaiveDate>> = HashMap::new();
    for (event, recurrence_id) in &events {
        if let Some(day) = recurrence_id {
            moved.entry(event.uid.clone()).or_default().push(*day);
        }
    }
    events
        .into_iter()
        .map(|(mut event, recurrence_id)| {
            if recurrence_id.is_none()
                && event.repeat.is_some()
                && let Some(days) = moved.get(&event.uid)
            {
                event.except.extend(days);
            }
            event
        })
        .collect()
}

/// Joins folded lines (a line starting with a space or tab continues the one before).
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        match (raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(raw.trim_end_matches('\r').to_owned()),
        }
    }
    lines
}

/// An event from its properties, with its `RECURRENCE-ID` day when it changes one occurrence.
fn event(props: &[(String, String)]) -> Option<(Event, Option<NaiveDate>)> {
    let get = |key: &str| props.iter().find(|(name, _)| property_name(name) == key);
    let (start, start_time) = get("DTSTART").and_then(|(name, value)| date_time(name, value))?;
    let (mut end, mut end_time) = match get("DTEND").and_then(|(name, value)| date_time(name, value)) {
        Some(end) => end,
        None => match get("DURATION").and_then(|(_, value)| duration(value)) {
            Some(length) => {
                let at = start.and_time(start_time.unwrap_or(NaiveTime::MIN)) + length;
                (at.date(), start_time.map(|_| at.time()))
            }
            None => (start, start_time),
        },
    };
    // An all-day event ends the day before its `DTEND`; a timed one ending at midnight ends the
    // day before too.
    if start_time.is_none() && end > start {
        end -= Duration::days(1);
    } else if end_time == Some(NaiveTime::MIN) && end > start {
        end -= Duration::days(1);
        end_time = Some(NaiveTime::from_hms_opt(23, 59, 0).expect("a valid time"));
    }
    let mut until = None;
    let mut count = None;
    let repeat = get("RRULE").and_then(|(_, rule)| {
        for part in rule.split(';') {
            match part.split_once('=') {
                Some(("UNTIL", value)) => until = date_time("UNTIL", value).map(|(day, _)| day),
                Some(("COUNT", value)) => count = value.parse().ok(),
                _ => {}
            }
        }
        rule.parse::<Recurrence>().ok()
    });
    let except = props
        .iter()
        .filter(|(name, _)| property_name(name) == "EXDATE")
        .flat_map(|(name, value)| value.split(',').filter_map(|v| date_time(name, v).map(|(day, _)| day)).collect::<Vec<_>>())
        .collect();
    let recurrence_id = get("RECURRENCE-ID").and_then(|(name, value)| date_time(name, value)).map(|(day, _)| day);
    let text = |key: &str| get(key).map(|(_, value)| unescape(value)).unwrap_or_default();
    let event = Event {
        uid: text("UID"),
        title: text("SUMMARY"),
        start,
        start_time,
        end: end.max(start),
        end_time,
        location: text("LOCATION"),
        notes: text("DESCRIPTION"),
        repeat,
        until,
        count,
        except,
    };
    Some((event, recurrence_id))
}

/// A property's name without its parameters: `DTSTART;TZID=…` is `DTSTART`.
fn property_name(name: &str) -> &str {
    name.split(';').next().unwrap_or(name)
}

/// A `DATE` or `DATE-TIME` value as a local day and time (`None` for a date).
fn date_time(name: &str, value: &str) -> Option<(NaiveDate, Option<NaiveTime>)> {
    let value = value.trim();
    if name.contains("VALUE=DATE") && !name.contains("VALUE=DATE-TIME") || value.len() == 8 {
        return NaiveDate::parse_from_str(value, "%Y%m%d").ok().map(|day| (day, None));
    }
    let (text, utc) = match value.strip_suffix('Z') {
        Some(text) => (text, true),
        None => (value, false),
    };
    let at = NaiveDateTime::parse_from_str(text, "%Y%m%dT%H%M%S").ok()?;
    let at = if utc { Utc.from_utc_datetime(&at).with_timezone(&Local).naive_local() } else { at };
    Some((at.date(), Some(at.time())))
}

/// A `DURATION` such as `PT1H30M`, `P1D` or `P1W`.
fn duration(value: &str) -> Option<Duration> {
    let rest = value.trim().trim_start_matches('+').strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut number = String::new();
    for c in rest.chars() {
        match c {
            'T' => {}
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = std::mem::take(&mut number).parse().ok()?;
                total += match unit {
                    'W' => Duration::weeks(n),
                    'D' => Duration::days(n),
                    'H' => Duration::hours(n),
                    'M' => Duration::minutes(n),
                    'S' => Duration::seconds(n),
                    _ => return None,
                };
            }
        }
    }
    Some(total)
}

/// Text with the iCalendar escapes taken out.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn reads_events_and_lays_out_repeats() {
        let feed = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Stand-up\\, daily\r\nDTSTART:20261026T093000\r\nDTEND:20261026T094500\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4\r\nEXDATE:20261028T093000\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:a\r\nRECURRENCE-ID:20261102T093000\r\nSUMMARY:Stand-up (moved)\r\nDTSTART:20261103T100000\r\nDTEND:20261103T101500\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Trip to the\r\n  mountains\r\nDTSTART;VALUE=DATE:20261030\r\nDTEND;VALUE=DATE:20261102\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse(feed);
        assert_eq!(events.len(), 3);
        let standup = &events[0];
        assert_eq!((standup.title.as_str(), standup.start_time), ("Stand-up, daily", NaiveTime::from_hms_opt(9, 30, 0)));
        // Four occurrences from the 26th: the 28th left out, the 2nd moved.
        assert_eq!(standup.starts_in(d(2026, 10, 1), d(2026, 11, 30)), [d(2026, 10, 26), d(2026, 11, 4)]);
        let trip = &events[2];
        assert_eq!((trip.title.as_str(), trip.all_day(), trip.end), ("Trip to the mountains", true, d(2026, 11, 1)));
        assert_eq!(trip.starts_in(d(2026, 11, 1), d(2026, 11, 1)), [d(2026, 10, 30)], "a day it spans");
    }
}
