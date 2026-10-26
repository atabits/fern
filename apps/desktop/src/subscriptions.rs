//! Subscribed calendars, the original's "URL" calendars: their feeds are read from their
//! addresses now and then, kept in the store for offline use, and their events laid out for the
//! calendar.

use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::NaiveDate;
use fern_core::ical::{self, Event, Subscription};
use fern_core::{Id, Store};
use gpui::{App, AppContext as _, Context, Entity, Global};

/// Not right at startup, so that the first frame does not wait on the network.
const FIRST_READ_AFTER: Duration = Duration::from_secs(5);
const READ_EVERY: Duration = Duration::from_secs(30 * 60);
/// The largest feed read, so that a wrong address cannot fill the memory.
const MAX_FEED: u64 = 32 * 1024 * 1024;

/// A subscribed calendar with the events of its last feed.
pub struct Calendar {
    pub subscription: Subscription,
    pub events: Vec<Event>,
}

/// An event on one of the days asked for: the day its occurrence starts.
pub struct Occurrence<'a> {
    pub event: &'a Event,
    pub calendar: &'a Subscription,
    pub start: NaiveDate,
}

impl Occurrence<'_> {
    /// The last day this occurrence covers.
    pub fn end(&self) -> NaiveDate {
        self.start + (self.event.end - self.event.start)
    }
}

pub struct Feeds {
    store: Rc<Store>,
    calendars: Vec<Calendar>,
    /// The subscriptions being read now.
    reading: Vec<Id>,
}

struct GlobalFeeds(Entity<Feeds>);

impl Global for GlobalFeeds {}

pub fn init(store: Rc<Store>, cx: &mut App) {
    let feeds = cx.new(|cx| {
        let mut feeds = Feeds { store, calendars: Vec::new(), reading: Vec::new() };
        feeds.load();
        feeds.start(cx);
        feeds
    });
    cx.set_global(GlobalFeeds(feeds));
}

pub fn feeds(cx: &App) -> Option<Entity<Feeds>> {
    cx.try_global::<GlobalFeeds>().map(|global| global.0.clone())
}

impl Feeds {
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
            .map(|subscription| Calendar { events: ical::parse(&subscription.feed), subscription })
            .collect();
    }

    pub fn calendars(&self) -> &[Calendar] {
        &self.calendars
    }

    pub fn is_reading(&self, id: Id) -> bool {
        self.reading.contains(&id)
    }

    /// The events of the shown calendars that cover any day from `from` to `to`.
    pub fn occurrences(&self, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence<'_>> {
        self.calendars
            .iter()
            .filter(|calendar| !calendar.subscription.hidden)
            .flat_map(|calendar| {
                calendar.events.iter().flat_map(move |event| {
                    event.starts_in(from, to).into_iter().map(move |start| Occurrence { event, calendar: &calendar.subscription, start })
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
        if self.reading.contains(&id) {
            return;
        }
        self.reading.push(id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let feed = cx.background_executor().spawn(async move { fetch(&url) }).await;
            this.update(cx, |this, cx| {
                this.reading.retain(|reading| *reading != id);
                let error = feed.as_ref().err().map(|err| format!("{err:#}"));
                let saved = this.store.save_subscription_feed(id, feed.as_deref().map_err(|_| error.as_deref().unwrap_or_default()));
                if let Err(err) = saved {
                    eprintln!("failed to keep the calendar feed: {err:#}");
                }
                this.load();
                cx.notify();
            })
            .ok();
        })
        .detach();
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
            self.read(id, cx);
        }
        cx.notify();
    }

    pub fn delete(&mut self, id: Id, cx: &mut Context<Self>) {
        if let Err(err) = self.store.delete_subscription(id) {
            eprintln!("failed to unsubscribe: {err:#}");
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
    let mut response = ureq::get(&url)
        .header("User-Agent", &format!("fern/{}", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("cannot read {url}"))?;
    let text = response.body_mut().with_config().limit(MAX_FEED).read_to_string()?;
    anyhow::ensure!(text.contains("BEGIN:VCALENDAR"), "the address gives no calendar");
    Ok(text)
}
