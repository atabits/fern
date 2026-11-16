//! Events of a subscribed calendar (the original's "URL" calendars): the `VEVENT`s of an
//! iCalendar (RFC 5545) feed, with their repeats laid out over the days shown.
//!
//! Only what a calendar needs to draw is read: the title, the start and end (a whole day or a
//! time), the place, the description and the repeat rule with its `UNTIL`, `COUNT` and
//! `EXDATE`s; a changed occurrence (`RECURRENCE-ID`) replaces the repeat's one. Recurrences
//! are expanded in the source time zone, then each occurrence is converted for display.
//! Civil dates and floating times are never shifted to a different date by a time zone.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, Local, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Tz;

use crate::recurrence::Recurrence;

mod time;
use time::{DurationValue, Identity, Time};

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
    pub location: String,
    pub notes: String,
    pub repeat: Option<Recurrence>,
    source_start: Time,
    length: EventLength,
    until: Option<Time>,
    count: Option<u32>,
    except: HashSet<Identity>,
}

/// A single instance in the display time zone, including its own DST-adjusted time and end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Occurrence {
    pub start: NaiveDate,
    pub start_time: Option<NaiveTime>,
    /// The last covered date; all-day and midnight ends are exclusive in the source feed.
    pub end: NaiveDate,
    pub end_time: Option<NaiveTime>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum EventLength {
    Days(i64),
    /// DTEND defines the same elapsed duration for every occurrence.
    Exact(Duration),
    /// DURATION defines nominal calendar days followed by elapsed hours/minutes/seconds.
    Nominal(DurationValue),
}

impl Event {
    pub fn all_day(&self) -> bool {
        matches!(self.source_start, Time::Date(_))
    }

    pub fn occurrences_in(&self, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence> {
        self.occurrences_in_timezone(from, to, &Local)
    }

    /// Expands in the event's civil calendar before converting each instance. The explicit
    /// target also lets callers test feeds without changing the process-wide local zone.
    pub fn occurrences_in_timezone<T: TimeZone>(&self, from: NaiveDate, to: NaiveDate, target: &T) -> Vec<Occurrence> {
        if from > to {
            return Vec::new();
        }
        let mut occurrences = Vec::new();
        let mut day = self.source_start.date();
        let mut seen = 0;
        // A source date can fall up to two dates either side of the display date.
        let last_source_day = to.checked_add_signed(Duration::days(2)).unwrap_or(NaiveDate::MAX);
        // A bound for broken rules: no feed repeats more than daily for this long.
        for index in 0..20_000 {
            let source = self.source_start.on_date(day);
            if day > last_source_day || self.until.is_some_and(|until| source.is_after(until))
                || self.count.is_some_and(|count| seen >= count)
            {
                break;
            }
            // DTSTART has the explicit-time gap policy. A nonexistent generated time is
            // skipped and does not consume COUNT, as required by RFC 5545 section 3.3.10.
            if let Some(occurrence) = self.occurrence(source, target, index != 0) {
                seen += 1;
                if occurrence.start <= to && occurrence.end >= from
                    && source.identity().is_some_and(|id| !self.except.contains(&id))
                {
                    occurrences.push(occurrence);
                }
            }
            let Some(rule) = &self.repeat else { break };
            if day >= last_source_day || self.count.is_some_and(|count| seen >= count) {
                break;
            }
            let next = rule.next_after(day);
            if next <= day {
                break;
            }
            day = next;
        }
        occurrences
    }

    /// The first days of occurrences that cover any day from `from` to `to`.
    pub fn starts_in(&self, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        self.occurrences_in(from, to).into_iter().map(|occurrence| occurrence.start).collect()
    }

    fn occurrence<T: TimeZone>(&self, source: Time, target: &T, strict: bool) -> Option<Occurrence> {
        let start = source.displayed(target, strict)?;
        if let EventLength::Days(days) = self.length {
            let end = start.date().checked_add_signed(Duration::try_days(days.saturating_sub(1))?)?;
            return Some(Occurrence { start: start.date(), start_time: None, end, end_time: None });
        }
        let end = match (source, self.length) {
            (Time::Zoned(..), EventLength::Exact(length)) => {
                source.instant(strict)?.checked_add_signed(length)?.with_timezone(target).naive_local()
            }
            (Time::Floating(at), EventLength::Exact(length)) => at.checked_add_signed(length)?,
            (_, EventLength::Nominal(length)) => {
                let end_day = source.plus_days(length.days)?;
                match end_day {
                    Time::Zoned(..) => end_day.instant(false)?.checked_add_signed(length.elapsed)?.with_timezone(target).naive_local(),
                    Time::Floating(at) => at.checked_add_signed(length.elapsed)?,
                    Time::Date(_) => return None,
                }
            }
            _ => return None,
        };
        // Calendar rendering uses an inclusive final day. Midnight belongs to the preceding
        // day's final minute, not to an additional all-day bar on the following date.
        let (end_date, end_time) = if end.time() == NaiveTime::MIN && end.date() > start.date() {
            (end.date().pred_opt()?, NaiveTime::from_hms_opt(23, 59, 0)?)
        } else {
            (end.date(), end.time())
        };
        Some(Occurrence { start: start.date(), start_time: Some(start.time()), end: end_date, end_time: Some(end_time) })
    }
}

#[derive(Debug, Default)]
pub struct ParsedCalendar {
    pub events: Vec<Event>,
    /// Unsupported zones and malformed event dates are reported instead of silently
    /// interpreting an unknown source time zone as the user's local time zone.
    pub warnings: Vec<String>,
}

/// The events of an iCalendar text. Use `parse_with_warnings` when showing import status.
pub fn parse(text: &str) -> Vec<Event> {
    parse_with_warnings(text).events
}

pub fn parse_with_warnings(text: &str) -> ParsedCalendar {
    let lines = unfold(text);
    let aliases = timezone_aliases(&lines);
    let mut result = ParsedCalendar::default();
    let mut events = Vec::new();
    let mut current: Option<Vec<(String, String)>> = None;
    let mut depth = 0usize;
    for line in lines {
        let upper = line.to_ascii_uppercase();
        match upper.as_str() {
            "BEGIN:VEVENT" => {
                current = Some(Vec::new());
                depth = 0;
            }
            "END:VEVENT" => {
                if let Some(props) = current.take() {
                    match event(&props, &aliases) {
                        Ok(event) => events.push(event),
                        Err(warning) if !result.warnings.contains(&warning) => result.warnings.push(warning),
                        Err(_) => {}
                    }
                }
                depth = 0;
            }
            _ if upper.starts_with("BEGIN:") && current.is_some() => depth += 1,
            _ if upper.starts_with("END:") && current.is_some() => depth = depth.saturating_sub(1),
            _ => {
                if let (Some(props), Some((name, value)), 0) = (current.as_mut(), content_line(&line), depth) {
                    props.push((name.to_owned(), value.to_owned()));
                }
            }
        }
    }
    // A changed occurrence takes the place of the repeat's one.
    let mut moved: HashMap<String, Vec<Identity>> = HashMap::new();
    for parsed in &events {
        if let Some(id) = parsed.recurrence_id {
            moved.entry(parsed.uid.clone()).or_default().push(id);
        }
    }
    result.events = events
        .into_iter()
        .filter_map(|parsed| {
            let mut event = parsed.event?;
            if parsed.recurrence_id.is_none()
                && event.repeat.is_some()
                && let Some(ids) = moved.get(&event.uid)
            {
                event.except.extend(ids);
            }
            Some(event)
        })
        .collect();
    result
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

struct ParsedEvent {
    uid: String,
    recurrence_id: Option<Identity>,
    event: Option<Event>,
}

fn event(props: &[(String, String)], aliases: &HashMap<String, Tz>) -> Result<ParsedEvent, String> {
    let get = |key: &str| props.iter().find(|(name, _)| property_name(name).eq_ignore_ascii_case(key));
    let text = |key: &str| get(key).map(|(_, value)| unescape(value)).unwrap_or_default();
    let read_time = |key: &str| get(key).map(|(name, value)| Time::parse(name, value, aliases)).transpose();
    let recurrence_id = read_time("RECURRENCE-ID")?.and_then(Time::identity);
    let uid = text("UID");
    if text("STATUS").eq_ignore_ascii_case("CANCELLED") {
        return Ok(ParsedEvent { uid, recurrence_id, event: None });
    }
    let source_start = read_time("DTSTART")?.ok_or("Calendar event has no DTSTART")?;
    let length = if let Some(end) = read_time("DTEND")? {
        match (source_start, end) {
            (Time::Date(start), Time::Date(end)) if end > start => EventLength::Days((end - start).num_days()),
            (Time::Floating(start), Time::Floating(end)) if end >= start => EventLength::Exact(end - start),
            (Time::Zoned(..), Time::Zoned(..)) => {
                let length = end.instant(false).ok_or("Invalid calendar end time")?
                    - source_start.instant(false).ok_or("Invalid calendar start time")?;
                if length < Duration::zero() {
                    return Err("Calendar event ends before it starts".into());
                }
                EventLength::Exact(length)
            }
            _ => return Err("Calendar DTSTART and DTEND have incompatible dates or time zones".into()),
        }
    } else if let Some((_, value)) = get("DURATION") {
        let length = DurationValue::parse(value).ok_or_else(|| format!("Invalid calendar duration: {value}"))?;
        if matches!(source_start, Time::Date(_)) {
            if length.elapsed != Duration::zero() || length.days == 0 {
                return Err("An all-day calendar event requires a whole-day duration".into());
            }
            EventLength::Days(length.days)
        } else {
            EventLength::Nominal(length)
        }
    } else if matches!(source_start, Time::Date(_)) {
        EventLength::Days(1)
    } else {
        EventLength::Exact(Duration::zero())
    };
    let mut until = None;
    let mut count = None;
    let repeat = get("RRULE").map(|(_, rule)| {
        for part in rule.split(';') {
            match part.split_once('=') {
                Some(("UNTIL", value)) => until = Some(Time::parse("UNTIL", value, aliases)?),
                Some(("COUNT", value)) => count = Some(value.parse().map_err(|_| "Invalid calendar recurrence count")?),
                _ => {}
            }
        }
        rule.parse::<Recurrence>().map_err(|err| err.to_string())
    }).transpose()?;
    if matches!((source_start, until),
        (Time::Zoned(..), Some(Time::Floating(_)))
        | (Time::Floating(_), Some(Time::Zoned(..)))
        | (Time::Date(_), Some(Time::Floating(_) | Time::Zoned(..))))
    {
        return Err("Calendar UNTIL does not use the DTSTART time basis".into());
    }
    let except = props
        .iter()
        .filter(|(name, _)| property_name(name).eq_ignore_ascii_case("EXDATE"))
        .flat_map(|(name, value)| value.split(',').map(move |value| Time::parse(name, value, aliases)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter().filter_map(Time::identity).collect();
    let event = Event {
        uid: uid.clone(),
        title: text("SUMMARY"),
        location: text("LOCATION"),
        notes: text("DESCRIPTION"),
        repeat,
        source_start,
        length,
        until,
        count,
        except,
    };
    Ok(ParsedEvent { uid, recurrence_id, event: Some(event) })
}

/// A property's name without its parameters: `DTSTART;TZID=…` is `DTSTART`.
fn property_name(name: &str) -> &str {
    name.split(';').next().unwrap_or(name)
}

/// The delimiter may not be inside a quoted parameter, e.g. a vendor-qualified TZID.
fn content_line(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (index, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some((&line[..index], &line[index + 1..])),
            _ => {}
        }
    }
    None
}

/// Some producers give a VTIMEZONE a private identifier and publish its IANA name here.
/// Custom transition definitions without an IANA location remain explicitly unsupported.
fn timezone_aliases(lines: &[String]) -> HashMap<String, Tz> {
    let mut aliases = HashMap::new();
    let (mut inside, mut depth) = (false, 0usize);
    let (mut id, mut location) = (None, None);
    for line in lines {
        let upper = line.to_ascii_uppercase();
        match upper.as_str() {
            "BEGIN:VTIMEZONE" => {
                inside = true;
                depth = 0;
                id = None;
                location = None;
            }
            "END:VTIMEZONE" => {
                if let (Some(id), Some(zone)) = (id.take(), location.take()) {
                    aliases.insert(id, zone);
                }
                inside = false;
            }
            _ if inside && upper.starts_with("BEGIN:") => depth += 1,
            _ if inside && upper.starts_with("END:") => depth = depth.saturating_sub(1),
            _ if inside && depth == 0 => {
                if let Some((name, value)) = content_line(line) {
                    if name.eq_ignore_ascii_case("TZID") {
                        id = Some(value.to_owned());
                    } else if name.eq_ignore_ascii_case("X-LIC-LOCATION") {
                        location = value.parse::<Tz>().ok();
                    }
                }
            }
            _ => {}
        }
    }
    aliases
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
        let first = standup.occurrences_in(d(2026, 10, 26), d(2026, 10, 26))[0];
        assert_eq!((standup.title.as_str(), first.start_time), ("Stand-up, daily", NaiveTime::from_hms_opt(9, 30, 0)));
        // Four occurrences from the 26th: the 28th left out, the 2nd moved.
        assert_eq!(standup.starts_in(d(2026, 10, 1), d(2026, 11, 30)), [d(2026, 10, 26), d(2026, 11, 4)]);
        let trip = &events[2];
        let first = trip.occurrences_in(d(2026, 10, 30), d(2026, 10, 30))[0];
        assert_eq!((trip.title.as_str(), trip.all_day(), first.end), ("Trip to the mountains", true, d(2026, 11, 1)));
        assert_eq!(trip.starts_in(d(2026, 11, 1), d(2026, 11, 1)), [d(2026, 10, 30)], "a day it spans");
    }
}

#[cfg(test)]
#[path = "ical/timezone_tests.rs"]
mod timezone_tests;
