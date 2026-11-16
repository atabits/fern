//! Civil dates and source-zone times are kept distinct until an occurrence is displayed.

use std::collections::HashMap;

use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::{GapInfo, Tz};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Time {
    Date(NaiveDate),
    Floating(NaiveDateTime),
    Zoned(NaiveDateTime, Tz),
}

/// EXDATE and RECURRENCE-ID refer to the original start, not its displayed day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Identity {
    Date(NaiveDate),
    Floating(NaiveDateTime),
    Instant(DateTime<Utc>),
}

impl Time {
    pub fn parse(name: &str, value: &str, aliases: &HashMap<String, Tz>) -> Result<Self, String> {
        let value = value.trim();
        let invalid = || format!("Invalid calendar date: {value}");
        if parameter(name, "VALUE").is_some_and(|value| value.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
            return NaiveDate::parse_from_str(value, "%Y%m%d").map(Self::Date).map_err(|_| invalid());
        }
        let (value, utc) = value.strip_suffix('Z').map_or((value, false), |value| (value, true));
        let at = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").map_err(|_| invalid())?;
        if utc {
            return Ok(Self::Zoned(at, chrono_tz::UTC));
        }
        match parameter(name, "TZID") {
            Some(id) => {
                let zone = id.parse::<Tz>().ok().or_else(|| aliases.get(id).copied())
                    .ok_or_else(|| format!("Unsupported calendar time zone: {id}"))?;
                Ok(Self::Zoned(at, zone))
            }
            None => Ok(Self::Floating(at)),
        }
    }

    pub fn date(self) -> NaiveDate {
        match self {
            Self::Date(date) => date,
            Self::Floating(at) | Self::Zoned(at, _) => at.date(),
        }
    }

    pub fn on_date(self, date: NaiveDate) -> Self {
        match self {
            Self::Date(_) => Self::Date(date),
            Self::Floating(at) => Self::Floating(date.and_time(at.time())),
            Self::Zoned(at, zone) => Self::Zoned(date.and_time(at.time()), zone),
        }
    }

    pub fn identity(self) -> Option<Identity> {
        match self {
            Self::Date(date) => Some(Identity::Date(date)),
            Self::Floating(at) => Some(Identity::Floating(at)),
            Self::Zoned(at, zone) => resolve(at, zone, false).map(Identity::Instant),
        }
    }

    /// `strict` skips nonexistent recurrence-generated times. A directly specified time
    /// uses the offset before the gap; ambiguous times use the first occurrence (RFC 5545).
    pub fn instant(self, strict: bool) -> Option<DateTime<Utc>> {
        match self {
            Self::Zoned(at, zone) => resolve(at, zone, strict),
            _ => None,
        }
    }

    pub fn is_after(self, until: Self) -> bool {
        match (self, until) {
            (Self::Date(date), Self::Date(limit)) => date > limit,
            (Self::Floating(at), Self::Floating(limit)) => at > limit,
            (Self::Zoned(..), Self::Zoned(..)) => self.instant(false) > until.instant(false),
            // Tolerate a DATE limit in a timed feed without applying a viewer-zone offset.
            (_, Self::Date(limit)) => self.date() > limit,
            _ => false,
        }
    }

    pub fn displayed<T: TimeZone>(self, target: &T, strict: bool) -> Option<NaiveDateTime> {
        match self {
            Self::Date(date) => date.and_hms_opt(0, 0, 0),
            Self::Floating(at) => Some(at),
            Self::Zoned(..) => Some(self.instant(strict)?.with_timezone(target).naive_local()),
        }
    }

    pub fn plus_days(self, days: i64) -> Option<Self> {
        let date = self.date().checked_add_signed(Duration::try_days(days)?)?;
        Some(self.on_date(date))
    }
}

fn resolve(at: NaiveDateTime, zone: Tz, strict: bool) -> Option<DateTime<Utc>> {
    match zone.from_local_datetime(&at) {
        LocalResult::Single(at) => Some(at.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, second) => Some(first.min(second).with_timezone(&Utc)),
        LocalResult::None if !strict => {
            let (_, before) = GapInfo::new(&at, &zone)?.begin?;
            before.fix().from_local_datetime(&at).single().map(|at| at.with_timezone(&Utc))
        }
        LocalResult::None => None,
    }
}

pub(super) fn parameter<'a>(name: &'a str, key: &str) -> Option<&'a str> {
    name.split(';').skip(1).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.eq_ignore_ascii_case(key).then(|| value.trim_matches('"'))
    })
}

/// DURATION distinguishes nominal calendar days from elapsed hours across DST.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DurationValue {
    pub days: i64,
    pub elapsed: Duration,
}

impl DurationValue {
    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.trim().trim_start_matches('+').strip_prefix('P')?;
        let (mut days, mut seconds, mut number) = (0i64, 0i64, String::new());
        let (mut time, mut any) = (false, false);
        for c in rest.chars() {
            match c {
                'T' if !time && number.is_empty() => time = true,
                '0'..='9' => number.push(c),
                unit => {
                    let n = std::mem::take(&mut number).parse::<i64>().ok()?;
                    any = true;
                    match (time, unit) {
                        (false, 'W') => days = days.checked_add(n.checked_mul(7)?)?,
                        (false, 'D') => days = days.checked_add(n)?,
                        (true, 'H') => seconds = seconds.checked_add(n.checked_mul(3600)?)?,
                        (true, 'M') => seconds = seconds.checked_add(n.checked_mul(60)?)?,
                        (true, 'S') => seconds = seconds.checked_add(n)?,
                        _ => return None,
                    }
                }
            }
        }
        (any && number.is_empty()).then_some(Self { days, elapsed: Duration::try_seconds(seconds)? })
    }
}
