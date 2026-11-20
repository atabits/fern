//! Subscribed calendars, the original's "URL" calendars: their feeds are read from their
//! addresses now and then, kept in the store for offline use, and their events laid out for the
//! calendar.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::{Local, NaiveDate, TimeZone};
use fern_core::ical::{self, Event, Subscription};
use fern_core::{Id, Store};
use gpui::{App, AppContext as _, Context, Entity, Global};

/// Not right at startup, so that the first frame does not wait on the network.
const FIRST_READ_AFTER: Duration = Duration::from_secs(5);
const READ_EVERY: Duration = Duration::from_secs(30 * 60);
/// The largest feed read, so that a wrong address cannot fill the memory.
const MAX_FEED: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChinaHolidaySource {
    Google,
    Apple,
}

impl ChinaHolidaySource {
    pub const ALL: [Self; 2] = [Self::Google, Self::Apple];

    pub const fn url(self) -> &'static str {
        match self {
            Self::Google => fern_i18n::CHINA_HOLIDAY_GOOGLE_URL,
            Self::Apple => "https://calendars.icloud.com/holidays/cn_zh.ics",
        }
    }

    pub fn from_url(url: &str) -> Option<Self> {
        let url = url.trim();
        let address = url.strip_prefix("https://").or_else(|| url.strip_prefix("webcal://"))?;
        Self::ALL.into_iter().find(|source| &source.url()["https://".len()..] == address)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google Calendar",
            Self::Apple => "Apple iCloud",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChinaHolidayStatus {
    #[default]
    Idle,
    Reading,
    Failed,
}

/// A subscribed calendar with the events of its last feed.
pub struct Calendar {
    pub subscription: Subscription,
    pub events: Vec<Event>,
}

/// An event on one of the days asked for: the day its occurrence starts.
pub struct Occurrence<'a> {
    pub event: &'a Event,
    pub calendar: &'a Subscription,
    pub times: ical::Occurrence,
}

pub struct Feeds {
    store: Rc<Store>,
    calendars: Vec<Calendar>,
    /// The active read request for each subscription.
    reading: HashMap<Id, Id>,
    china_holiday_status: ChinaHolidayStatus,
}

struct GlobalFeeds(Entity<Feeds>);

impl Global for GlobalFeeds {}

pub fn init(store: Rc<Store>, cx: &mut App) {
    let feeds = cx.new(|cx| {
        let mut feeds = Feeds::new(store);
        feeds.start(cx);
        feeds
    });
    cx.set_global(GlobalFeeds(feeds));
}

pub fn feeds(cx: &App) -> Option<Entity<Feeds>> {
    cx.try_global::<GlobalFeeds>().map(|global| global.0.clone())
}

impl Feeds {
    fn new(store: Rc<Store>) -> Self {
        let mut feeds = Self {
            store,
            calendars: Vec::new(),
            reading: HashMap::new(),
            china_holiday_status: ChinaHolidayStatus::Idle,
        };
        feeds.load();
        feeds
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_READ_AFTER).await;
            loop {
                if this.update(cx, |this, cx| this.read_all(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_EVERY).await;
            }
        })
        .detach();
    }

    /// Takes the subscriptions and their feeds from the store.
    fn load(&mut self) {
        self.calendars = self
            .store
            .subscriptions()
            .unwrap_or_default()
            .into_iter()
            .map(|mut subscription| {
                let parsed = ical::parse_with_warnings(&subscription.feed);
                if !parsed.warnings.is_empty() {
                    let warning = parsed.warnings.join("; ");
                    subscription.error = Some(match subscription.error {
                        Some(error) => format!("{error}; {warning}"),
                        None => warning,
                    });
                }
                Calendar { events: parsed.events, subscription }
            })
            .collect();
    }

    pub fn calendars(&self) -> &[Calendar] {
        &self.calendars
    }

    pub fn is_reading(&self, id: Id) -> bool {
        self.reading.contains_key(&id)
    }

    pub fn china_holiday_status(&self) -> ChinaHolidayStatus {
        self.china_holiday_status
    }

    /// The events of the shown calendars that cover any day from `from` to `to`.
    pub fn occurrences(&self, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence<'_>> {
        self.occurrences_in_timezone(from, to, &Local)
    }

    fn occurrences_in_timezone<Tz: TimeZone>(&self, from: NaiveDate, to: NaiveDate, timezone: &Tz) -> Vec<Occurrence<'_>> {
        self.calendars
            .iter()
            .filter(|calendar| !calendar.subscription.hidden)
            .flat_map(|calendar| {
                calendar.events.iter().flat_map(move |event| {
                    event.occurrences_in_timezone(from, to, timezone).into_iter()
                        .map(move |times| Occurrence { event, calendar: &calendar.subscription, times })
                })
            })
            .collect()
    }

    pub fn read_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<Id> = self.calendars.iter().map(|calendar| calendar.subscription.id).collect();
        for id in ids {
            self.read(id, cx);
        }
    }

    /// Reads a subscription's feed from its address, off the UI thread.
    pub fn read(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(url) = self.calendars.iter().find(|c| c.subscription.id == id).map(|c| c.subscription.url.clone()) else {
            return;
        };
        if self.is_reading(id) {
            return;
        }
        let request = Id::now_v7();
        self.reading.insert(id, request);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let read_url = url.clone();
            let feed = cx.background_executor().spawn(async move { fetch(&read_url) }).await;
            this.update(cx, |this, cx| {
                if this.reading.get(&id) != Some(&request) {
                    return;
                }
                this.reading.remove(&id);
                let error = feed.as_ref().err().map(|err| format!("{err:#}"));
                if let Err(err) = this.store.save_subscription_feed(id, &url, feed.as_deref().map_err(|_| error.as_deref().unwrap_or_default())) {
                    eprintln!("failed to keep the calendar feed: {err:#}");
                }
                this.load();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn select_china_holiday_source(
        &mut self,
        current: Option<Id>,
        source: ChinaHolidaySource,
        name: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.china_holiday_status == ChinaHolidayStatus::Reading {
            return;
        }
        let mut previous = None;
        for calendar in &self.calendars {
            let subscription = &calendar.subscription;
            if let Some(existing) = ChinaHolidaySource::from_url(&subscription.url) {
                if current.is_none() || existing == source {
                    return;
                }
                if current == Some(subscription.id) {
                    previous = Some((subscription.id, subscription.url.clone()));
                }
            }
        }
        if current.is_some() && previous.is_none() {
            return;
        }
        self.china_holiday_status = ChinaHolidayStatus::Reading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                let feed = fetch(source.url())?;
                anyhow::ensure!(!ical::parse(&feed).is_empty(), "the holiday feed has no events");
                Ok(feed)
            }).await;
            this.update(cx, |this, cx| {
                this.finish_china_holiday_change(previous.as_ref().map(|(id, url)| (*id, url.as_str())), source, name, result);
                cx.notify();
            }).ok();
        }).detach();
    }

    fn finish_china_holiday_change(
        &mut self,
        previous: Option<(Id, &str)>,
        source: ChinaHolidaySource,
        name: &str,
        result: Result<String>,
    ) {
        let saved = result.and_then(|feed| {
            let subscriptions = self.store.subscriptions()?;
            let has_provider = subscriptions.iter().any(|subscription| {
                ChinaHolidaySource::from_url(&subscription.url).is_some_and(|existing| {
                    previous.is_none() || existing == source
                })
            });
            anyhow::ensure!(!has_provider, "the holiday calendar is already subscribed");
            if let Some((id, url)) = previous {
                anyhow::ensure!(
                    self.store.replace_subscription_source(id, url, source.url(), &feed)?,
                    "the subscription changed while the holiday feed was loading",
                );
                self.reading.remove(&id);
            } else {
                self.store.add_subscription_with_feed(name, source.url(), Some("#22C55E"), &feed)?;
            }
            Ok(())
        });
        self.china_holiday_status = match saved {
            Ok(()) => ChinaHolidayStatus::Idle,
            Err(err) => {
                eprintln!("failed to select the holiday calendar: {err:#}");
                ChinaHolidayStatus::Failed
            }
        };
        self.load();
    }

    pub fn add(&mut self, name: &str, url: &str, color: Option<&str>, cx: &mut Context<Self>) {
        match self.store.add_subscription(name, url, color) {
            Ok(id) => {
                self.load();
                self.read(id, cx);
            }
            Err(err) => eprintln!("failed to subscribe to the calendar: {err:#}"),
        }
        cx.notify();
    }

    pub fn update_subscription(&mut self, id: Id, name: &str, url: &str, color: Option<&str>, hidden: bool, cx: &mut Context<Self>) {
        let moved = self.calendars.iter().find(|c| c.subscription.id == id).is_some_and(|c| c.subscription.url.trim() != url.trim());
        if let Err(err) = self.store.update_subscription(id, name, url, color, hidden) {
            eprintln!("failed to change the subscription: {err:#}");
        }
        self.load();
        if moved {
            self.reading.remove(&id);
            self.read(id, cx);
        }
        cx.notify();
    }

    pub fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_subscription(id) {
            eprintln!("failed to unsubscribe: {err:#}");
        } else {
            self.reading.remove(&id);
        }
        self.load();
        cx.notify();
    }
}

/// Reads a feed; `webcal://` addresses are read over HTTPS, as calendar apps do.
fn fetch(url: &str) -> Result<String> {
    let url = url.trim();
    let url = match url.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent.get(&url)
        .header("User-Agent", &format!("fern/{}", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("cannot read {url}"))?;
    let text = response.body_mut().with_config().limit(MAX_FEED).read_to_string()?;
    anyhow::ensure!(text.contains("BEGIN:VCALENDAR"), "the address gives no calendar");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:new-year\nSUMMARY:New Year\nDTSTART;VALUE=DATE:20260101\nEND:VEVENT\nEND:VCALENDAR";

    #[test]
    fn subscribed_tzid_and_utc_events_keep_the_same_time_after_reload() -> Result<()> {
        // The reported Windows/Moscow case, fetched through the same path as a URL subscription.
        let feed = concat!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n",
            "BEGIN:VEVENT\r\nUID:NY\r\nSUMMARY:NY\r\n",
            "DTSTART;TZID=America/New_York:20261115T090000\r\n",
            "DTEND;TZID=America/New_York:20261115T100000\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nUID:UTC\r\nSUMMARY:UTC\r\n",
            "DTSTART:20261115T140000Z\r\nDTEND:20261115T150000Z\r\n",
            "END:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}/calendar.ics", server.server_addr());
        let worker = std::thread::spawn(move || {
            let request = server.recv_timeout(Duration::from_secs(10)).unwrap().expect("feed requested");
            request.respond(tiny_http::Response::from_string(feed)).unwrap();
        });
        let fetched = fetch(&url);
        worker.join().unwrap();
        let fetched = fetched?;
        let store = Rc::new(Store::open_in_memory()?);
        let id = store.add_subscription("Synthetic calendar", &url, None)?;
        store.save_subscription_feed(id, &url, Ok(&fetched))?;
        let mut feeds = Feeds::new(store.clone());
        let day = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let moscow = chrono::FixedOffset::east_opt(3 * 3600).unwrap();
        for offline in [false, true] {
            if offline {
                store.save_subscription_feed(id, &url, Err("offline"))?;
                feeds.load();
            }
            let occurrences = feeds.occurrences_in_timezone(day, day, &moscow);
            assert_eq!(occurrences.len(), 2);
            assert_eq!(occurrences[0].times, occurrences[1].times);
            for occurrence in occurrences {
                assert_eq!((occurrence.times.start, occurrence.times.end), (day, day));
                assert_eq!(occurrence.times.start_time, chrono::NaiveTime::from_hms_opt(17, 0, 0));
                assert_eq!(occurrence.times.end_time, chrono::NaiveTime::from_hms_opt(18, 0, 0));
            }
            assert_eq!(store.subscriptions()?[0].feed, feed, "source data must remain intact");
        }
        // A new display timezone must not require fetching or reparsing the feed.
        let utc = feeds.occurrences_in_timezone(day, day, &chrono::Utc);
        assert_eq!(utc[0].times.start_time, chrono::NaiveTime::from_hms_opt(14, 0, 0));
        assert_eq!(utc[0].times, utc[1].times);
        Ok(())
    }

    #[test]
    fn unsupported_timezone_is_reported_without_discarding_valid_events() -> Result<()> {
        let unknown = FEED.replace("END:VCALENDAR", concat!(
            "BEGIN:VEVENT\nUID:unknown\nSUMMARY:Unsupported zone\n",
            "DTSTART;TZID=Unknown/Timezone:20260101T090000\nEND:VEVENT\nEND:VCALENDAR",
        ));
        let store = Rc::new(Store::open_in_memory()?);
        let url = "https://example.com/synthetic.ics";
        let id = store.add_subscription_with_feed("Mixed calendar", url, None, &unknown)?;
        let mut feeds = Feeds::new(store.clone());
        let calendar = &feeds.calendars()[0];
        assert_eq!(calendar.events.len(), 1);
        assert!(calendar.subscription.error.as_ref().unwrap().contains("Unknown/Timezone"));
        assert!(store.subscriptions()?[0].error.is_none(), "parse warnings are separate from fetch errors");
        assert_eq!(store.subscriptions()?[0].feed, unknown);

        store.save_subscription_feed(id, url, Err("offline"))?;
        feeds.load();
        let error = feeds.calendars()[0].subscription.error.as_ref().unwrap();
        assert!(error.contains("offline") && error.contains("Unknown/Timezone"));

        store.save_subscription_feed(id, url, Ok(FEED))?;
        feeds.load();
        assert!(feeds.calendars()[0].subscription.error.is_none());
        assert_eq!(feeds.calendars()[0].events.len(), 1);
        Ok(())
    }

    #[test]
    fn holiday_source_changes_only_after_a_successful_read() -> Result<()> {
        let store = Rc::new(Store::open_in_memory()?);
        let google = ChinaHolidaySource::Google;
        let apple = ChinaHolidaySource::Apple;
        let id = store.add_subscription_with_feed("My holidays", google.url(), Some("#22C55E"), FEED)?;
        store.update_subscription(id, "My holidays", google.url(), Some("#22C55E"), true)?;
        let mut feeds = Feeds::new(store.clone());
        let request = Id::now_v7();
        feeds.reading.insert(id, request);
        let replacement = FEED.replace("20260101", "20270101");

        feeds.finish_china_holiday_change(
            Some((id, google.url())),
            apple,
            "Holidays",
            Err(anyhow::anyhow!("feed unavailable")),
        );
        assert_eq!(feeds.china_holiday_status(), ChinaHolidayStatus::Failed);
        assert!(feeds.is_reading(id));
        assert_eq!(feeds.calendars()[0].subscription.url, google.url());
        assert_eq!(feeds.calendars()[0].subscription.feed, FEED);

        feeds.finish_china_holiday_change(Some((id, google.url())), apple, "Holidays", Ok(replacement.clone()));
        assert_eq!(feeds.china_holiday_status(), ChinaHolidayStatus::Idle);
        assert!(!feeds.is_reading(id));
        assert_eq!(feeds.calendars().len(), 1);
        let changed = &feeds.calendars()[0].subscription;
        assert_eq!(changed.id, id);
        assert_eq!(changed.name, "My holidays");
        assert_eq!(changed.color.as_deref(), Some("#22C55E"));
        assert!(changed.hidden);
        assert_eq!(changed.url, apple.url());
        assert_eq!(changed.feed, replacement);
        assert_eq!(feeds.calendars()[0].events.len(), 1);

        // A refresh started before the switch may finish after it.
        assert!(!store.save_subscription_feed(id, google.url(), Ok(FEED))?);
        assert_eq!(store.subscriptions()?[0].feed, replacement);
        Ok(())
    }

    #[test]
    fn a_second_holiday_source_cannot_be_added_after_one_is_selected() -> Result<()> {
        let store = Rc::new(Store::open_in_memory()?);
        store.add_subscription("Personal", "https://example.com/private.ics", None)?;
        let mut feeds = Feeds::new(store);

        feeds.finish_china_holiday_change(None, ChinaHolidaySource::Apple, "Holidays", Ok(FEED.to_owned()));
        feeds.finish_china_holiday_change(None, ChinaHolidaySource::Google, "Holidays", Ok(FEED.to_owned()));

        assert_eq!(feeds.calendars().len(), 2);
        assert_eq!(feeds.calendars()[0].subscription.name, "Personal");
        assert_eq!(feeds.calendars()[1].subscription.url, ChinaHolidaySource::Apple.url());
        Ok(())
    }
}
