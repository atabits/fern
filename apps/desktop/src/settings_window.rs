//! The Settings window from the account menu, modeled on the original's separate desktop window:
//! pages on the left, grouped rows on the right, values picked from small dropdowns.

use std::rc::Rc;

use chrono::Datelike as _;
use fern_core::reminder::{ALL_DAY_OFFSETS, TIMED_OFFSETS};
use fern_core::{DURATIONS, DefaultDate, Priority, format_minutes, DateFormat, Preferences, SmartList, Store, Visibility, WEEK_STARTS};
use gpui::{
    Bounds, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, MouseButton, ScrollHandle,
    TitlebarOptions, WindowBounds, WindowControlArea, WindowDecorations, WindowHandle, WindowOptions,
    point, size,
};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, VerticalScrollbar, popover};

use crate::actions::{CloseWindow, DismissSettings};
use crate::calendar;
use crate::i18n::{self, CurrentLocale, Locale};
use crate::release_notes::{self, ReleaseNotes, action_button};
use crate::subscriptions::{ChinaHolidaySource, ChinaHolidayStatus};
#[path = "appearance_view.rs"]
mod appearance_view;
#[path = "shortcuts_settings_view.rs"]
mod shortcuts_settings_view;
#[path = "sticky_settings_view.rs"]
mod sticky_settings_view;

/// A row of a Settings page, and of the sections column, as the original's.
const ROW_HEIGHT: f32 = 40.;
pub const WIDTH: f32 = 840.;
pub const HEIGHT: f32 = 680.;
const MIN_WIDTH: f32 = 640.;
const MIN_HEIGHT: f32 = 400.;
const SYNC_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

type SelectPick = Box<dyn Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>)>;
type SelectOption = (SharedString, bool, SelectPick);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// The original's Features: the optional sections on or off.
    Features,
    /// Daily notification, desktop reminders and constant reminders.
    Notifications,
    SmartList,
    #[default]
    DateTime,
    Appearance,
    More,
    /// The sync server's address and token.
    Sync,
    /// The original's Integrations: calendars subscribed to by address.
    Integrations,
    Sticky,
    Shortcuts,
    About,
}

/// The open dropdown, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Visibility(SmartList),
    DefaultDate,
    DefaultDuration,
    TimedReminder,
    AllDayReminder,
    DefaultPriority,
    DefaultTag,
    AddTo,
    OverdueAt,
    TimeFormat,
    DateFormat,
    WeekStart,
    Language,
    DefaultList,
    DailyTime,
    ChinaHolidays(Option<fern_core::Id>),
}

pub enum SettingsEvent {
    /// A preference was saved and other windows should reflect it immediately.
    Changed,
    Dismissed,
}

pub struct SettingsWindow {
    store: Rc<Store>,
    preferences: Preferences,
    page: Page,
    page_scroll: ScrollHandle,
    menu: Option<Menu>,
    focus_handle: FocusHandle,
    /// Guard against emitting `SettingsEvent::Dismissed` twice when close is triggered repeatedly.
    closing: bool,
    /// Linux and macOS: a press on the drag strip turns into a window move once the mouse moves.
    should_move: bool,
    /// The outcome of the last backup or import.
    backup_message: Option<String>,
    /// Error saving Sticky Note preferences (kept separate from backup/import messages).
    sticky_error: Option<String>,
    /// Cached lookups so `render()` never runs synchronous SQLite queries per frame.
    cached_tags: Vec<fern_core::Tag>,
    cached_lists: Vec<fern_core::List>,
    sound_enabled: bool,
    #[cfg(windows)]
    minimize_to_tray: bool,
    #[cfg(windows)]
    always_on_top: bool,
    installed_notes: Option<Rc<ReleaseNotes>>,
    release_notes: Option<Rc<ReleaseNotes>>,
    /// Settings → Sync: the server's address and the token, debounced on edit and flushed on submit/close.
    sync_server: Entity<crate::text_input::TextInput>,
    sync_token: Entity<crate::text_input::TextInput>,
    sync_save_task: Option<gpui::Task<()>>,
    sync_dirty: bool,
    /// Integrations: the subscription being added (`Some(None)`) or changed, its color, and its
    /// name and address fields.
    subscription_form: Option<(Option<fern_core::Id>, Option<String>)>,
    subscription_name: Entity<crate::text_input::TextInput>,
    subscription_url: Entity<crate::text_input::TextInput>,
    subscription_kind: fern_core::ical::CalendarKind,
    subscription_provider: fern_core::ical::CalendarProvider,
    subscription_username: Entity<crate::text_input::TextInput>,
    subscription_password: Entity<crate::text_input::TextInput>,
    subscription_writable: bool,
    subscription_discovering: bool,
    subscription_discovered: Vec<crate::subscriptions::DiscoveredCalendar>,
    subscription_discover_error: Option<String>,
    recording_shortcut: Option<shortcuts_settings_view::RecordingTarget>,
    recording_rejected: bool,
    confirm_reset_shortcuts: bool,
    _updates: Option<gpui::Subscription>,
    _feeds: Option<gpui::Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsWindow {}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Opens Settings in its own desktop window centered on `parent`'s display.
pub fn open(
    store: Rc<Store>,
    notes: Option<Rc<ReleaseNotes>>,
    initial_page: Option<Page>,
    parent: &Window,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<SettingsWindow>> {
    let bounds = Bounds::centered(
        parent.display(cx).map(|display| display.id()),
        size(px(WIDTH), px(HEIGHT)),
        cx,
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t(cx).settings.into()),
            appears_transparent: true,
            traffic_light_position: cfg!(target_os = "macos").then_some(point(px(12.), px(14.))),
        }),
        window_decorations: Some(WindowDecorations::Client),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    let handle = cx.open_window(options, |window, cx| {
        crate::touch::init(window, cx);
        cx.new(|cx| {
            let mut view = SettingsWindow::new(store, cx);
            view.attach_window(window, cx);
            if let Some(notes) = notes {
                view.show_release_notes(notes, window, cx);
            } else {
                if let Some(page) = initial_page {
                    view.set_page(page, cx);
                }
                window.focus(&view.focus_handle, cx);
            }
            view
        })
    })?;
    handle.update(cx, |_, window, _| window.activate_window()).ok();
    Ok(handle)
}

/// Installs the theme and the first day of the week from `preferences`.
pub fn apply(preferences: &Preferences, window: &Window, cx: &mut App) {
    let refreshed = crate::user_themes::apply_theme(&preferences.theme, window, cx);
    calendar::set_week_start(preferences.week_start);
    calendar::set_hour12(preferences.hour12);
    calendar::set_date_format(preferences.date_format);
    calendar::set_week_numbers(preferences.week_numbers);
    if !refreshed {
        cx.refresh_windows();
    }
}

impl SettingsWindow {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        let preferences = store.preferences().unwrap_or_default();
        let config = store.sync_config().ok().flatten().unwrap_or_default();
        let cached_tags = store.tags().unwrap_or_default();
        let cached_lists = store.lists().unwrap_or_default();
        let sound_enabled = desktop_common::sound::enabled(&store);
        #[cfg(windows)]
        let tray_prefs = crate::tray::TrayPrefs::current(&store, cx);
        let field = |placeholder: &'static str, text: String, masked: bool, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut input = crate::text_input::TextInput::new(crate::text_input::Mode::SingleLine, placeholder, cx);
                if masked {
                    input = input.masked();
                }
                input.set_text(text, cx);
                input
            });
            cx.subscribe(&input, |this: &mut Self, _, _: &crate::text_input::Changed, cx| {
                this.schedule_save_sync(cx);
            })
            .detach();
            cx.subscribe(&input, |this: &mut Self, _, _: &crate::text_input::Submitted, cx| {
                this.flush_sync_save(cx);
            })
            .detach();
            input
        };
        let sync_server = field("https://fern.example.org", config.server, false, cx);
        let sync_token = field("", config.token, true, cx);
        let updates = crate::updater::updater(cx).map(|updater| cx.observe(&updater, |_, _, cx| cx.notify()));
        let feeds = crate::subscriptions::feeds(cx).map(|feeds| cx.observe(&feeds, |_, _, cx| cx.notify()));
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            this.refresh_cached_collections(cx);
        })
        .detach();
        #[cfg(windows)]
        cx.observe_global::<crate::tray::TrayPrefs>(|this, cx| {
            let prefs = *cx.global::<crate::tray::TrayPrefs>();
            this.minimize_to_tray = prefs.minimize_to_tray;
            this.always_on_top = prefs.always_on_top;
            cx.notify();
        })
        .detach();
        let plain = |placeholder: &'static str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| crate::text_input::TextInput::new(crate::text_input::Mode::SingleLine, placeholder, cx));
            cx.subscribe(&input, |this: &mut Self, _, _: &crate::text_input::Submitted, cx| {
                this.save_subscription_form(cx);
            })
            .detach();
            input
        };
        let subscription_name = plain(i18n::t(cx).subscription_name_placeholder, cx);
        let subscription_url = plain("https://…/calendar.ics", cx);
        cx.subscribe(&subscription_url, |_, _, _: &crate::text_input::Changed, cx| cx.notify()).detach();
        let subscription_username = plain("", cx);
        let subscription_password = cx.new(|cx| {
            crate::text_input::TextInput::new(crate::text_input::Mode::SingleLine, "", cx).masked()
        });
        cx.subscribe(&subscription_password, |this: &mut Self, _, _: &crate::text_input::Submitted, cx| {
            this.save_subscription_form(cx);
        })
        .detach();
        let installed_notes = release_notes::installed(&store).map(Rc::new);
        Self {
            store,
            preferences,
            page: Page::default(),
            page_scroll: ScrollHandle::new(),
            menu: None,
            focus_handle: cx.focus_handle(),
            closing: false,
            should_move: false,
            backup_message: None,
            sticky_error: None,
            cached_tags,
            cached_lists,
            sound_enabled,
            #[cfg(windows)]
            minimize_to_tray: tray_prefs.minimize_to_tray,
            #[cfg(windows)]
            always_on_top: tray_prefs.always_on_top,
            installed_notes,
            release_notes: None,
            sync_server,
            sync_token,
            sync_save_task: None,
            sync_dirty: false,
            subscription_form: None,
            subscription_name,
            subscription_url,
            subscription_kind: fern_core::ical::CalendarKind::Ics,
            subscription_provider: fern_core::ical::CalendarProvider::Url,
            subscription_username,
            subscription_password,
            subscription_writable: true,
            subscription_discovering: false,
            subscription_discovered: Vec::new(),
            subscription_discover_error: None,
            recording_shortcut: None,
            recording_rejected: false,
            confirm_reset_shortcuts: false,
            _updates: updates,
            _feeds: feeds,
        }
    }

    fn refresh_cached_collections(&mut self, cx: &mut Context<Self>) {
        self.cached_tags = self.store.tags().unwrap_or_default();
        self.cached_lists = self.store.lists().unwrap_or_default();
        cx.notify();
    }

    fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| {
                this.flush_sync_save(cx);
                if !this.closing {
                    this.closing = true;
                    cx.emit(SettingsEvent::Dismissed);
                }
            })
            .ok();
            true
        });
        cx.observe_global_in::<CurrentLocale>(window, |_, window, cx| {
            window.set_window_title(i18n::t(cx).settings);
        })
        .detach();
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_sync_save(cx);
        if self.closing {
            return;
        }
        self.closing = true;
        cx.emit(SettingsEvent::Dismissed);
        window.defer(cx, |window, _| window.remove_window());
    }

    fn schedule_save_sync(&mut self, cx: &mut Context<Self>) {
        self.sync_dirty = true;
        self.sync_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SYNC_SAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| this.flush_sync_save(cx)).ok();
        }));
    }

    fn flush_sync_save(&mut self, cx: &mut Context<Self>) {
        self.sync_save_task = None;
        if !self.sync_dirty {
            return;
        }
        self.sync_dirty = false;
        let config = fern_core::sync::SyncConfig {
            server: self.sync_server.read(cx).text().to_owned(),
            token: self.sync_token.read(cx).text().to_owned(),
        };
        match self.store.set_sync_config(&config) {
            Ok(()) => cx.emit(SettingsEvent::Changed),
            Err(err) => eprintln!("failed to save the sync settings: {err:#}"),
        }
    }

    pub fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        let changed = self.page != page;
        if changed {
            self.page_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.page = page;
        self.menu = None;
        self.release_notes = None;
        self.recording_shortcut = None;
        self.recording_rejected = false;
        self.confirm_reset_shortcuts = false;
        #[cfg(not(test))]
        if changed && page == Page::Shortcuts {
            self.reload_keymap(cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn current_page(&self) -> Page {
        self.page
    }

    pub fn show_release_notes(&mut self, notes: Rc<ReleaseNotes>, window: &mut Window, cx: &mut Context<Self>) {
        self.set_page(Page::About, cx);
        self.release_notes = Some(notes);
        window.focus(&self.focus_handle, cx);
    }

    fn back_to_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_page(Page::About, cx);
        window.focus(&self.focus_handle, cx);
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_reset_shortcuts {
            self.confirm_reset_shortcuts = false;
            cx.notify();
        } else if self.recording_shortcut.take().is_some() {
            self.recording_rejected = false;
            cx.notify();
        } else if self.menu.is_some() {
            self.set_menu(None, cx);
        } else if self.release_notes.is_some() {
            self.back_to_about(window, cx);
        } else if self.subscription_form.is_some() {
            self.subscription_form = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    fn set_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        self.menu = menu;
        cx.notify();
    }

    fn update(&mut self, window: &mut Window, cx: &mut Context<Self>, change: impl FnOnce(&mut Preferences)) {
        self.menu = None;
        match self.store.update_preferences(change) {
            Ok(preferences) => {
                self.preferences = preferences;
                apply(&self.preferences, window, cx);
                cx.emit(SettingsEvent::Changed);
            }
            Err(err) => {
                eprintln!("failed to save the settings: {err:#}");
                cx.notify();
            }
        }
    }

    /// "Generate Backup": all data in one file, saved where the user chooses.
    /// Runs SQLite backup + attachment bundling off the UI thread when backed by an on-disk DB.
    fn generate_backup(&mut self, cx: &mut Context<Self>) {
        let read = if let Some(db_path) = cx.try_global::<crate::DbPath>().map(|p| p.0.clone()) {
            cx.background_spawn(async move {
                let dir = crate::scratch_dir()?;
                let temp = dir.join(format!("fern-backup-{}.db", std::process::id()));
                let attachments = crate::attachments_dir().ok();
                let bg_store = Store::open(&db_path)?;
                bg_store.backup_to(&temp, attachments.as_deref())?;
                let bytes = std::fs::read(&temp);
                std::fs::remove_file(&temp).ok();
                Ok::<Vec<u8>, anyhow::Error>(bytes?)
            })
        } else {
            let temp = match crate::scratch_dir().map_err(anyhow::Error::from).and_then(|dir| {
                let temp = dir.join(format!("fern-backup-{}.db", std::process::id()));
                let attachments = crate::attachments_dir().ok();
                self.store.backup_to(&temp, attachments.as_deref())?;
                Ok(temp)
            }) {
                Ok(temp) => temp,
                Err(err) => {
                    self.backup_message = Some(err.to_string());
                    cx.notify();
                    return;
                }
            };
            cx.background_spawn(async move {
                let bytes = std::fs::read(&temp);
                std::fs::remove_file(&temp).ok();
                Ok::<Vec<u8>, anyhow::Error>(bytes?)
            })
        };
        cx.spawn(async move |this, cx| {
            let bytes = read.await;
            this.update(cx, |this, cx| match bytes {
                Ok(bytes) => {
                    let name = format!("fern-backup-{}.db", chrono::Local::now().format("%Y-%m-%d"));
                    crate::export::save_as(name, bytes, cx, |this, result, cx| {
                        this.backup_message = crate::export::outcome(result, i18n::t(cx));
                        cx.notify();
                    });
                }
                Err(err) => {
                    this.backup_message = Some(err.to_string());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Import Backups": a Fern backup is checked now and restored on the next start; a TickTick
    /// backup (its CSV file) is parsed and imported off the UI thread.
    fn import_backup(&mut self, cx: &mut Context<Self>) {
        let db_path = cx.try_global::<crate::DbPath>().map(|p| p.0.clone());
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Some(path) = chosen.await.ok().and_then(Result::ok).flatten().and_then(|paths| paths.into_iter().next()) else {
                return;
            };
            if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("csv")) {
                if let Some(db_path) = db_path {
                    let imported = cx
                        .background_spawn(async move {
                            let text = std::fs::read_to_string(&path)?;
                            let bg_store = Store::open(&db_path)?;
                            bg_store.import_ticktick(&text)
                        })
                        .await;
                    this.update(cx, |this, cx| {
                        let strings = i18n::t(cx);
                        this.backup_message = Some(match imported {
                            Ok(summary) => strings
                                .imported_from_ticktick
                                .replace("{tasks}", &summary.tasks.to_string())
                                .replace("{lists}", &summary.lists.to_string()),
                            Err(err) => format!("{err:#}"),
                        });
                        this.refresh_cached_collections(cx);
                        cx.emit(SettingsEvent::Changed);
                        crate::sticky_note::data_changed(cx.entity_id(), cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                let text = cx.background_spawn(async move { std::fs::read_to_string(&path) }).await;
                this.update(cx, |this, cx| {
                    let strings = i18n::t(cx);
                    let imported = text.map_err(anyhow::Error::from).and_then(|text| this.store.import_ticktick(&text));
                    this.backup_message = Some(match imported {
                        Ok(summary) => strings
                            .imported_from_ticktick
                            .replace("{tasks}", &summary.tasks.to_string())
                            .replace("{lists}", &summary.lists.to_string()),
                        Err(err) => format!("{err:#}"),
                    });
                    this.refresh_cached_collections(cx);
                    cx.emit(SettingsEvent::Changed);
                    crate::sticky_note::data_changed(cx.entity_id(), cx);
                    cx.notify();
                })
                .ok();
                return;
            }
            let outcome = cx
                .background_spawn(async move {
                    fern_core::Store::check_backup(&path)
                        .and_then(|()| Ok(std::fs::copy(&path, crate::pending_restore_path()?)?))
                        .map(|_| ())
                })
                .await;
            this.update(cx, |this, cx| {
                let strings = i18n::t(cx);
                this.backup_message = Some(match outcome {
                    Ok(()) => strings.restore_on_restart.to_owned(),
                    Err(err) => format!("{err}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting("locale", locale.code()) {
            eprintln!("failed to save the language: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        self.menu = None;
        cx.refresh_windows();
    }

    /// Opens a dropdown from the debug-build dev state: `settings=<page>[>menu]`.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, spec: &str, cx: &mut Context<Self>) {
        let (page, menu) = spec.split_once('>').unwrap_or((spec, ""));
        // `about>import:<path>`: imports a TickTick backup as "Import Backups" does.
        if let Some(path) = menu.strip_prefix("import:") {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let strings = i18n::t(cx);
            self.backup_message = Some(match self.store.import_ticktick(&text) {
                Ok(summary) => strings
                    .imported_from_ticktick
                    .replace("{tasks}", &summary.tasks.to_string())
                    .replace("{lists}", &summary.lists.to_string()),
                Err(err) => format!("{err:#}"),
            });
            crate::sticky_note::data_changed(cx.entity_id(), cx);
        }
        let page = match page {
            "smart-list" => Page::SmartList,
            "appearance" => Page::Appearance,
            "more" => Page::More,
            "about" => Page::About,
            "sync" => Page::Sync,
            "integrations" => Page::Integrations,
            "sticky" => Page::Sticky,
            "shortcuts" => Page::Shortcuts,
            "features" => Page::Features,
            "notifications" => Page::Notifications,
            _ => Page::DateTime,
        };
        self.set_page(page, cx);
        self.menu = match menu {
            "week" => Some(Menu::WeekStart),
            "time" => Some(Menu::TimeFormat),
            "date" => Some(Menu::DateFormat),
            "today" => Some(Menu::Visibility(SmartList::Today)),
            "tags" => Some(Menu::Visibility(SmartList::Tags)),
            "language" => Some(Menu::Language),
            "list" => Some(Menu::DefaultList),
            "duration" => Some(Menu::DefaultDuration),
            "add-to" => Some(Menu::AddTo),
            "overdue" => Some(Menu::OverdueAt),
            "reminder" => Some(Menu::TimedReminder),
            _ => None,
        };
        cx.notify();
    }

    /// Task Default's first group: date, duration and reminders of new tasks.
    fn render_task_defaults(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let pick = |change: Box<dyn Fn(&mut fern_core::TaskDefaults)>| -> SelectPick {
            Box::new(move |this, window, cx| this.update(window, cx, |p| change(&mut p.task_defaults)))
        };
        let date_name = |date: Option<DefaultDate>| match date {
            None => strings.none_option,
            Some(date) => strings.default_dates[DefaultDate::ALL.iter().position(|d| *d == date).unwrap_or(0)],
        };
        let dates = std::iter::once(None)
            .chain(DefaultDate::ALL.into_iter().map(Some))
            .map(|date| (SharedString::from(date_name(date)), date == defaults.date, pick(Box::new(move |t| t.date = date))))
            .collect();
        let duration_name = |minutes: i64| {
            DURATIONS.iter().position(|d| *d == minutes).map_or_else(|| format_minutes(minutes), |i| strings.durations[i].to_owned())
        };
        let durations = DURATIONS
            .into_iter()
            .map(|minutes| (SharedString::from(duration_name(minutes)), minutes == defaults.duration, pick(Box::new(move |t| t.duration = minutes))))
            .collect();
        let reminder_options = |offsets: [i64; 5], names: [&'static str; 5], current: Option<i64>, set: fn(&mut fern_core::TaskDefaults, Option<i64>)| {
            std::iter::once((None, strings.none_option))
                .chain(offsets.into_iter().zip(names).map(|(o, n)| (Some(o), n)))
                .map(|(offset, name)| (SharedString::from(name), offset == current, pick(Box::new(move |t| set(t, offset)))))
                .collect::<Vec<_>>()
        };
        let reminder_name = |offsets: [i64; 5], names: [&'static str; 5], current: Option<i64>| {
            current.and_then(|o| offsets.iter().position(|x| *x == o)).map_or(strings.none_option, |i| names[i])
        };
        v_flex()
            .py_1p5()
            .rounded(px(8.))
            .bg(cx.theme().colors().surface_background)
            .child(self.select_row(None, strings.default_date, date_name(defaults.date).into(), Menu::DefaultDate, dates, cx))
            .child(self.select_row(None, strings.default_duration, duration_name(defaults.duration).into(), Menu::DefaultDuration, durations, cx))
            .child(self.select_row(
                None,
                strings.default_timed_reminder,
                reminder_name(TIMED_OFFSETS, strings.reminders_timed, defaults.timed_reminder).into(),
                Menu::TimedReminder,
                reminder_options(TIMED_OFFSETS, strings.reminders_timed, defaults.timed_reminder, |t, o| t.timed_reminder = o),
                cx,
            ))
            .child(self.select_row(
                None,
                strings.default_all_day_reminder,
                reminder_name(ALL_DAY_OFFSETS, strings.reminders_all_day, defaults.all_day_reminder).into(),
                Menu::AllDayReminder,
                reminder_options(ALL_DAY_OFFSETS, strings.reminders_all_day, defaults.all_day_reminder, |t, o| t.all_day_reminder = o),
                cx,
            ))
    }

    /// Task Default's priority and tag rows, above the default list.
    fn render_priority_and_tag(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let defaults = self.preferences.task_defaults.clone();
        let priorities = [
            (Priority::None, strings.priority_none),
            (Priority::Low, strings.priority_low),
            (Priority::Medium, strings.priority_medium),
            (Priority::High, strings.priority_high),
        ];
        let priority_name = priorities.iter().find(|(p, _)| *p == defaults.priority).map_or(strings.priority_none, |(_, n)| n);
        let priority_options = priorities
            .into_iter()
            .map(|(priority, name)| {
                let pick: SelectPick =
                    Box::new(move |this, window, cx| this.update(window, cx, |p| p.task_defaults.priority = priority));
                (SharedString::from(name), priority == defaults.priority, pick)
            })
            .collect();
        let tag_options = std::iter::once(None)
            .chain(self.cached_tags.iter().map(|t| Some(t.name.clone())))
            .map(|tag| {
                let selected = tag == defaults.tag;
                let label = SharedString::from(tag.clone().unwrap_or_else(|| strings.none_option.to_owned()));
                let pick: SelectPick =
                    Box::new(move |this, window, cx| this.update(window, cx, |p| p.task_defaults.tag = tag.clone()));
                (label, selected, pick)
            })
            .collect();
        vec![
            self.select_row(None, strings.default_priority, (*priority_name).into(), Menu::DefaultPriority, priority_options, cx),
            self.select_row(
                None,
                strings.default_tag,
                defaults.tag.clone().unwrap_or_else(|| strings.none_option.to_owned()).into(),
                Menu::DefaultTag,
                tag_options,
                cx,
            ),
        ]
    }

    fn drag_area(&self, id: &'static str, window: &Window, cx: &Context<Self>) -> Stateful<Div> {
        let linux = cfg!(any(target_os = "linux", target_os = "freebsd"));
        let mac = cfg!(target_os = "macos");
        let windows = cfg!(target_os = "windows");
        let client_side = matches!(window.window_decorations(), gpui::Decorations::Client { .. });
        div()
            .id(id)
            .window_control_area(WindowControlArea::Drag)
            .when(!windows, |bar| {
                bar.on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = true))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
                    .on_mouse_down_out(cx.listener(|this, _, _, _| this.should_move = false))
                    .on_mouse_move(cx.listener(|this, _, window, _| {
                        if this.should_move {
                            this.should_move = false;
                            window.start_window_move();
                        }
                    }))
                    .on_click(move |event, window, _| {
                        if event.click_count() != 2 {
                            return;
                        }
                        if mac {
                            window.titlebar_double_click();
                        } else if window.window_controls().maximize && window.is_resizable() {
                            window.zoom_window();
                        }
                    })
            })
            .when(linux && client_side && window.window_controls().window_menu, |bar| {
                bar.on_mouse_down(MouseButton::Right, |event, window, _| window.show_window_menu(event.position))
            })
    }

    fn render_nav(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let item = |id: &'static str, icon: IconName, label: &'static str, selected: bool| {
            ListItem::new(id)
                .height(px(37.))
                .toggle_state(selected)
                .start_slot(Icon::new(icon).color(Color::Icon))
                .child(Label::new(label).truncate())
        };
        let page = |id, icon, index: usize, target: Page| {
            item(id, icon, strings.settings_pages[index], self.page == target)
                .on_click(cx.listener(move |this, _, _, cx| this.set_page(target, cx)))
        };
        v_flex()
            .flex_none()
            .w(px(240.))
            .h_full()
            .bg(cx.theme().colors().panel_background)
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .child(
                self.drag_area("settings-nav-drag", window, cx)
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(if cfg!(target_os = "macos") { 56. } else { 44. }))
                    .when(cfg!(target_os = "macos"), |el| el.pt(px(20.)))
                    .px_4()
                    .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(strings.settings)),
            )
            .child(
                v_scroll(
                    "settings-navigation-scroll",
                    v_flex()
                        .id("settings-navigation")
                        .pl_2()
                        .pr(px(12.))
                        .pb_2()
                        .gap_0p5()
                        .child(
                            item("settings-features", IconName::LayoutGrid, strings.features, self.page == Page::Features)
                                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Features, cx))),
                        )
                        .child(page("settings-smart-list", IconName::Funnel, 0, Page::SmartList))
                        .child(
                            item("settings-notifications", IconName::Bell, strings.notifications_page, self.page == Page::Notifications)
                                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Notifications, cx))),
                        )
                        .child(page("settings-date-time", IconName::Clock, 1, Page::DateTime))
                        .child(page("settings-appearance", IconName::PaintRoller, 2, Page::Appearance))
                        .child(page("settings-more", IconName::ToggleRight, 3, Page::More))
                        .child(
                            item("settings-sticky", IconName::FileText, strings.sticky_notes, self.page == Page::Sticky)
                                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Sticky, cx))),
                        )
                        .child(
                            item("settings-sync", IconName::RefreshCw, strings.data_and_sync, self.page == Page::Sync)
                                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Sync, cx))),
                        )
                        .child(
                            item("settings-integrations", IconName::CalendarDays, strings.integrations, self.page == Page::Integrations)
                                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Integrations, cx))),
                        )
                        .child(div().h(px(12.)))
                        .child(
                            div()
                                .debug_selector(|| "settings-shortcuts".to_owned())
                                .child(page("settings-shortcuts", IconName::Keyboard, 4, Page::Shortcuts)),
                        )
                        .child(div().h(px(12.)))
                        .child(page("settings-about", IconName::Info, 5, Page::About)),
                )
                .flex_1()
                .min_h_0(),
            )
    }

    /// A row whose value opens a dropdown of `options` (label, selected, what picking it does).
    fn select_row(
        &self,
        icon: Option<IconName>,
        label: &'static str,
        value: SharedString,
        menu: Menu,
        options: Vec<SelectOption>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.menu == Some(menu);
        let dropdown = open.then(|| {
            let options = Rc::new(options);
            let items = (0..options.len()).map(|i| {
                let (label, selected, _) = &options[i];
                let options = options.clone();
                ListItem::new(SharedString::from(format!("option-{i}")))
                    .child(Label::new(label.clone()))
                    .when(*selected, |item| item.end_slot(Icon::new(IconName::Check).color(Color::Accent).size(IconSize::Small)))
                    .on_click(cx.listener(move |this, _, window, cx| (options[i].2)(this, window, cx)))
            });
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(180.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .children(items),
            )
        });
        let hover_bg = cx.theme().colors().element_hover;
        h_flex()
            .debug_selector(|| format!("settings-row-{menu:?}"))
            .h(px(ROW_HEIGHT))
            .px_4()
            .gap_2()
            .children(icon.map(|icon| Icon::new(icon).color(Color::Muted)))
            .child(div().flex_1().child(Label::new(label)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("settings-value-{label}")))
                    .relative()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(open, |el| el.bg(hover_bg))
                    .hover(move |s| s.bg(hover_bg))
                    .child(Label::new(value).color(Color::Muted))
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
                    .children(dropdown),
            )
            .into_any_element()
    }

    /// About → Updates: the automatic checks, how the last one went and what to do next.
    fn render_updates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::updater::Status;
        let updater = crate::updater::updater(cx).filter(|updater| updater.read(cx).supported())?;
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let (status, automatic) = {
            let updater = updater.read(cx);
            (updater.status().clone(), updater.automatic())
        };
        let version = |text: &str, version: &str| text.replace("{version}", version);
        let (message, action) = match &status {
            Status::Idle => (None, Some(strings.check_for_updates.to_owned())),
            Status::Checking => (Some(strings.update_checking.to_owned()), None),
            Status::UpToDate => (Some(strings.update_up_to_date.to_owned()), Some(strings.check_for_updates.to_owned())),
            Status::Downloading(v) => (Some(version(strings.update_downloading, v)), None),
            Status::Ready(notes) => (Some(version(strings.update_ready, &notes.version)), Some(strings.restart_to_update.to_owned())),
            Status::Available(notes) => (Some(version(strings.update_available, &notes.version)), Some(version(strings.update_to, &notes.version))),
            Status::Failed(error) => (Some(strings.update_failed.replace("{error}", error)), Some(strings.check_for_updates.to_owned())),
        };
        let notes = match status {
            Status::Ready(notes) | Status::Available(notes) => Some(notes),
            _ => None,
        };
        let install = notes.is_some();
        let switch_updater = updater.clone();
        Some(
            v_flex()
                .child(div().pb_3().child(Label::new(strings.updates).weight(FontWeight::MEDIUM)))
                .child(
                    v_flex()
                        .py_1p5()
                        .rounded(px(8.))
                        .bg(colors.surface_background)
                        .child(
                            h_flex()
                                .id("auto-update")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.check_updates_automatically)))
                                .child(Switch::new().toggle_state(automatic))
                                .on_click(move |_, _, cx| switch_updater.update(cx, |updater, cx| updater.set_automatic(!automatic, cx))),
                        )
                        .child(
                            h_flex()
                                .min_h(px(ROW_HEIGHT))
                                .py_2()
                                .px_4()
                                .gap_x_3()
                                .gap_y_1()
                                .flex_wrap()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(110.))
                                        .text_size(px(13.))
                                        .text_color(if install { colors.text } else { colors.text_muted })
                                        .child(message.unwrap_or_else(|| format!("Fern {}", env!("CARGO_PKG_VERSION")))),
                                )
                                .child(h_flex().gap_2().flex_none().flex_wrap()
                                    .children(notes.map(|notes| {
                                        action_button("update-notes", cx)
                                            .debug_selector(|| "update-notes".to_owned())
                                            .border_color(colors.border)
                                            .child(Label::new(strings.whats_new).size(LabelSize::Small))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.show_release_notes(notes.clone(), window, cx);
                                            }))
                                    }))
                                    .children(action.map(|action| {
                                        action_button("update-action", cx)
                                            .debug_selector(|| "update-action".to_owned())
                                            .when(install, |el| el.bg(colors.accent.opacity(0.12)).border_color(colors.accent.opacity(0.35)))
                                            .child(Label::new(action).size(LabelSize::Small).weight(FontWeight::MEDIUM))
                                            .on_click(move |_, _, cx| {
                                                updater.update(cx, |updater, cx| {
                                                    if install {
                                                        updater.restart_to_update(cx)
                                                    } else {
                                                        updater.check(cx)
                                                    }
                                                });
                                            })
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Opens the subscription form, empty for a new calendar or filled from `id`'s.
    fn open_subscription_form(&mut self, id: Option<fern_core::Id>, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let current = id.and_then(|id| {
            let feeds = crate::subscriptions::feeds(cx)?;
            let feeds = feeds.read(cx);
            feeds.calendars().iter().find(|c| c.subscription.id == id).map(|c| c.subscription.clone())
        });
        let (name, url, color, kind, provider, username, password, writable) = match current {
            Some(s) => (s.name, s.url, s.color, s.kind, s.provider, s.username, s.password, s.writable),
            None => (
                String::new(),
                String::new(),
                None,
                fern_core::ical::CalendarKind::Ics,
                fern_core::ical::CalendarProvider::Url,
                String::new(),
                String::new(),
                true,
            ),
        };
        self.subscription_kind = kind;
        self.subscription_provider = provider;
        self.subscription_writable = writable;
        self.subscription_discovering = false;
        self.subscription_discovered.clear();
        self.subscription_discover_error = None;
        self.subscription_name.update(cx, |input, cx| input.set_text(name, cx));
        self.subscription_url.update(cx, |input, cx| input.set_text(url, cx));
        self.subscription_username.update(cx, |input, cx| input.set_text(username, cx));
        self.subscription_password.update(cx, |input, cx| input.set_text(password, cx));
        self.subscription_form = Some((id, color));
        window.focus(&self.subscription_url.focus_handle(cx), cx);
        cx.notify();
    }

    fn save_subscription_form(&mut self, cx: &mut Context<Self>) {
        let url = self.subscription_url.read(cx).text().to_owned();
        if url.trim().is_empty() {
            return;
        }
        let Some((id, color)) = self.subscription_form.take() else { return };
        let name = self.subscription_name.read(cx).text().to_owned();
        let kind = self.subscription_kind;
        let provider = self.subscription_provider;
        let username = self.subscription_username.read(cx).text().to_owned();
        let password = self.subscription_password.read(cx).text().to_owned();
        let writable = self.subscription_writable;
        if let Some(feeds) = crate::subscriptions::feeds(cx) {
            feeds.update(cx, |feeds, cx| match (id, kind) {
                (Some(id), fern_core::ical::CalendarKind::CalDav) => {
                    let hidden = feeds.calendars().iter().find(|c| c.subscription.id == id).is_some_and(|c| c.subscription.hidden);
                    feeds.update_caldav(id, &name, &url, color.as_deref(), hidden, provider, &username, &password, writable, cx);
                }
                (None, fern_core::ical::CalendarKind::CalDav) => {
                    feeds.add_caldav(&name, &url, color.as_deref(), provider, &username, &password, writable, cx);
                }
                (Some(id), fern_core::ical::CalendarKind::Ics) => {
                    let hidden = feeds.calendars().iter().find(|c| c.subscription.id == id).is_some_and(|c| c.subscription.hidden);
                    feeds.update_subscription(id, &name, &url, color.as_deref(), hidden, cx);
                }
                (None, fern_core::ical::CalendarKind::Ics) => feeds.add(&name, &url, color.as_deref(), cx),
            });
        }
        cx.notify();
    }

    fn discover_caldav(&mut self, cx: &mut Context<Self>) {
        if self.subscription_discovering {
            return;
        }
        let url = self.subscription_url.read(cx).text().trim().to_owned();
        let username = self.subscription_username.read(cx).text().to_owned();
        let password = self.subscription_password.read(cx).text().to_owned();
        if url.is_empty() {
            return;
        }
        self.subscription_discovering = true;
        self.subscription_discover_error = None;
        self.subscription_discovered.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::subscriptions::discover_caldav_calendars(&url, &username, &password) })
                .await;
            this.update(cx, |this, cx| {
                this.subscription_discovering = false;
                match result {
                    Ok(cals) => {
                        this.subscription_discovered = cals;
                    }
                    Err(err) => {
                        this.subscription_discover_error = Some(format!("{err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_china_holiday_source(
        &self,
        current: Option<(fern_core::Id, ChinaHolidaySource)>,
        enabled: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let hover = cx.theme().colors().ghost_element_hover;
        let subscription = current.map(|(id, _)| id);
        let selected = current.map(|(_, source)| source);
        let menu = Menu::ChinaHolidays(subscription);
        let open = enabled && self.menu == Some(menu);
        let dropdown = open.then(|| {
            PopoverAttachment::new(
                Placement::BelowLeft,
                popover(cx)
                    .min_w(px(156.))
                    .debug_selector(|| "china-holiday-source-menu".to_owned())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .children(ChinaHolidaySource::ALL.map(|source| {
                        h_flex()
                            .id(source.label())
                            .debug_selector(move || source.label().to_owned())
                            .tab_index(0)
                            .h(px(32.))
                            .gap_2()
                            .px_2()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(hover))
                            .focus_visible(|style| style.bg(hover))
                            .child(div().flex_1().child(Label::new(source.label())))
                            .when(selected == Some(source), |el| {
                                el.child(Icon::new(IconName::Check).color(Color::Accent).size(IconSize::Small))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_menu(None, cx);
                                cx.stop_propagation();
                                if selected != Some(source)
                                    && let Some(feeds) = crate::subscriptions::feeds(cx)
                                {
                                    feeds.update(cx, |feeds, cx| {
                                        feeds.select_china_holiday_source(subscription, source, strings.holidays_name, cx);
                                    });
                                }
                            }))
                    })),
            )
        });
        let color = if enabled { Color::Accent } else { Color::Muted };
        h_flex()
            .id(match subscription {
                Some(id) => SharedString::from(format!("china-holiday-source-{id}")),
                None => SharedString::from("subscription-holidays"),
            })
            .debug_selector(|| "china-holiday-source".to_owned())
            .relative()
            .flex_none()
            .gap_1()
            .child(
                Label::new(selected.map_or(strings.add_holidays, ChinaHolidaySource::label))
                    .color(color)
                    .when(selected.is_some(), |label| label.size(LabelSize::Small)),
            )
            .when(enabled, |el| {
                el.cursor_pointer()
                    .tab_index(0)
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(color))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
                    .when(selected.is_some(), |el| el.tooltip(Tooltip::text(strings.subscription_change_source)))
            })
            .children(dropdown)
            .into_any_element()
    }

    /// Integrations, as the original's URL calendars: each subscribed calendar with its color,
    /// whether it is read and when, show or hide, change and unsubscribe; and the form that adds
    /// or changes one (name, address, color).
    fn render_integrations(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let Some(feeds) = crate::subscriptions::feeds(cx) else { return Vec::new() };
        let feeds = feeds.read(cx);
        let chinese = i18n::locale(cx) == Locale::ZhCn;
        let china_status = feeds.china_holiday_status();
        let china_sources = ChinaHolidaySource::ALL.map(|source| {
            feeds.calendars().iter().any(|calendar| {
                ChinaHolidaySource::from_url(&calendar.subscription.url) == Some(source)
            })
        });
        let has_china_holidays = china_sources.into_iter().any(|installed| installed);
        let can_choose_source = china_status != ChinaHolidayStatus::Reading && !china_sources.into_iter().all(|installed| installed);
        let rows: Vec<AnyElement> = feeds
            .calendars()
            .iter()
            .map(|calendar| {
                let subscription = &calendar.subscription;
                let id = subscription.id;
                let hidden = subscription.hidden;
                let china_source = ChinaHolidaySource::from_url(&subscription.url);
                let status = if feeds.is_reading(id) {
                    (strings.subscription_reading.to_owned(), Color::Muted)
                } else if let Some(error) = &subscription.error {
                    (error.clone(), Color::Error)
                } else {
                    (strings.subscription_events.replace("{n}", &calendar.events.len().to_string()), Color::Muted)
                };
                let dot = crate::lists_panel::parse_color(subscription.color.as_deref()).unwrap_or(colors.accent);
                h_flex()
                    .id(SharedString::from(format!("subscription-{id}")))
                    .when_some(subscription.error.as_ref(), |el, error| el.tooltip(Tooltip::text(error.clone())))
                    .min_h(px(ROW_HEIGHT))
                    .px_4()
                    .gap_3()
                    .child(div().size(px(10.)).flex_none().rounded_full().bg(dot))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(Label::new(subscription.title().to_owned()).truncate().color(if hidden { Color::Muted } else { Color::Default }))
                                    .when(subscription.is_caldav(), |el| {
                                        el.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(colors.accent.opacity(0.14))
                                                .child(Label::new(strings.cal_badge_caldav).size(LabelSize::XSmall).color(Color::Accent)),
                                        )
                                    }),
                            )
                            .child(Label::new(status.0).size(LabelSize::XSmall).color(status.1).truncate()),
                    )
                    .children(china_source.map(|source| self.render_china_holiday_source(Some((id, source)), can_choose_source, cx)))
                    .child(
                        div()
                            .debug_selector(move || format!("subscription-show-{id}"))
                            .child(
                                IconButton::new(SharedString::from(format!("subscription-show-{id}")), if hidden { IconName::EyeOff } else { IconName::Eye })
                                    .icon_color(Color::Muted)
                                    .tooltip(Tooltip::text(if hidden { strings.subscription_show } else { strings.subscription_hide }))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                            feeds.update(cx, |feeds, cx| {
                                                if let Some(s) = feeds.calendars().iter().find(|c| c.subscription.id == id).map(|c| c.subscription.clone()) {
                                                    if s.is_caldav() {
                                                        feeds.update_caldav(id, &s.name, &s.url, s.color.as_deref(), !hidden, s.provider, &s.username, &s.password, s.writable, cx);
                                                    } else {
                                                        feeds.update_subscription(id, &s.name, &s.url, s.color.as_deref(), !hidden, cx);
                                                    }
                                                }
                                            });
                                        }
                                    })),
                            ),
                    )
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-read-{id}")), IconName::RefreshCw)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.subscription_read_now))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                    feeds.update(cx, |feeds, cx| feeds.read(id, cx));
                                }
                            })),
                    )
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-edit-{id}")), IconName::Pencil)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.subscription_edit))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_subscription_form(Some(id), window, cx))),
                    )
                    .child(
                        IconButton::new(SharedString::from(format!("subscription-delete-{id}")), IconName::Trash)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text(strings.subscription_delete))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                    feeds.update(cx, |feeds, cx| feeds.delete(id, cx));
                                }
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let field = |label: &'static str, input: Entity<crate::text_input::TextInput>| {
            h_flex()
                .h(px(40.))
                .px_4()
                .gap_4()
                .child(div().w(px(80.)).child(Label::new(label)))
                .child(div().flex_1().min_w_0().overflow_hidden().h(px(28.)).px_2().flex().items_center().rounded(px(6.)).bg(colors.background).child(input))
        };
        let form = self.subscription_form.as_ref().map(|(_, color)| {
            let has_url = !self.subscription_url.read(cx).text().trim().is_empty();
            let swatches = h_flex().gap_1p5().children(std::iter::once(None).chain(crate::lists_panel::LIST_COLORS.iter().map(|c| Some(*c))).map(|swatch| {
                let selected = color.as_deref() == swatch;
                let fill = crate::lists_panel::parse_color(swatch).unwrap_or(colors.accent);
                div()
                    .id(SharedString::from(format!("subscription-color-{}", swatch.unwrap_or("none"))))
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(fill.opacity(if swatch.is_none() { 0.35 } else { 1. }))
                    .when(selected, |el| el.border_2().border_color(colors.text))
                    .when(!selected, |el| el.hover(|s| s.border_2().border_color(colors.border)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some((_, color)) = &mut this.subscription_form {
                            *color = swatch.map(str::to_owned);
                        }
                        cx.notify();
                    }))
            }));
            let is_caldav = self.subscription_kind == fern_core::ical::CalendarKind::CalDav;
            let mode_btn = |id: &'static str, label: &'static str, kind: fern_core::ical::CalendarKind| {
                let active = self.subscription_kind == kind;
                div()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .px_2p5()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(active, |el| el.bg(colors.element_selected))
                    .when(!active, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                    .child(Label::new(label).size(LabelSize::Small).color(if active { Color::Accent } else { Color::Default }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.subscription_kind = kind;
                        if kind == fern_core::ical::CalendarKind::CalDav && this.subscription_provider == fern_core::ical::CalendarProvider::Url {
                            this.subscription_provider = fern_core::ical::CalendarProvider::CalDav;
                        }
                        cx.notify();
                    }))
            };
            let provider_label = |p: fern_core::ical::CalendarProvider| match p {
                fern_core::ical::CalendarProvider::Url => strings.cal_provider_url,
                fern_core::ical::CalendarProvider::CalDav => strings.cal_provider_caldav,
                fern_core::ical::CalendarProvider::ICloud => strings.cal_provider_icloud,
                fern_core::ical::CalendarProvider::Google => strings.cal_provider_google,
                fern_core::ical::CalendarProvider::Outlook => strings.cal_provider_outlook,
                fern_core::ical::CalendarProvider::Nextcloud => strings.cal_provider_nextcloud,
                fern_core::ical::CalendarProvider::Fastmail => strings.cal_provider_fastmail,
            };
            group()
                .child(
                    h_flex()
                        .px_4()
                        .py_2()
                        .gap_1()
                        .child(mode_btn("subscription-kind-ics", strings.cal_source_ics, fern_core::ical::CalendarKind::Ics))
                        .child(mode_btn("subscription-kind-caldav", strings.cal_source_caldav, fern_core::ical::CalendarKind::CalDav)),
                )
                .when(is_caldav, |el| {
                    el.child(
                        h_flex()
                            .min_h(px(36.))
                            .px_4()
                            .gap_4()
                            .child(div().w(px(80.)).child(Label::new(strings.cal_provider)))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .flex_wrap()
                                    .gap_1()
                                    .children(fern_core::ical::CalendarProvider::CALDAV_PRESETS.into_iter().map(|preset| {
                                        let active = self.subscription_provider == preset;
                                        let label = provider_label(preset);
                                        div()
                                            .id(SharedString::from(format!("caldav-provider-{}", preset.code())))
                                            .debug_selector(move || format!("caldav-provider-{}", preset.code()))
                                            .px_2()
                                            .h(px(24.))
                                            .flex()
                                            .items_center()
                                            .rounded(px(5.))
                                            .border_1()
                                            .border_color(if active { colors.accent } else { colors.border })
                                            .when(active, |el| el.bg(colors.accent.opacity(0.14)))
                                            .when(!active, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                                            .cursor_pointer()
                                            .child(Label::new(label).size(LabelSize::XSmall).color(if active { Color::Accent } else { Color::Default }))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.subscription_provider = preset;
                                                let suggested = preset.default_discovery_url();
                                                if !suggested.is_empty() {
                                                    let cur = this.subscription_url.read(cx).text().trim().to_owned();
                                                    let is_preset_url = cur.is_empty()
                                                        || fern_core::ical::CalendarProvider::CALDAV_PRESETS
                                                            .iter()
                                                            .any(|p| p.default_discovery_url() == cur);
                                                    if is_preset_url {
                                                        this.subscription_url.update(cx, |input, cx| input.set_text(suggested, cx));
                                                    }
                                                }
                                                cx.notify();
                                            }))
                                    })),
                            ),
                    )
                })
                .child(field(strings.subscription_name, self.subscription_name.clone()))
                .child(field("URL", self.subscription_url.clone()))
                .when(is_caldav, |el| {
                    el.child(field(strings.cal_username, self.subscription_username.clone()))
                        .child(field(strings.cal_password, self.subscription_password.clone()))
                        .child(
                            h_flex()
                                .id("caldav-writable-toggle")
                                .h(px(36.))
                                .px_4()
                                .gap_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.cal_writable).size(LabelSize::Small)))
                                .child(ui::Switch::new().toggle_state(self.subscription_writable))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.subscription_writable = !this.subscription_writable;
                                    cx.notify();
                                })),
                        )
                        .child(
                            h_flex()
                                .px_4()
                                .py_1()
                                .gap_2()
                                .child(
                                    form_button(
                                        "caldav-discover",
                                        if self.subscription_discovering { "…" } else { strings.cal_discover },
                                        false,
                                        &colors,
                                    )
                                    .when(!has_url || self.subscription_discovering, |el| el.opacity(0.45).cursor_default())
                                    .on_click(cx.listener(|this, _, _, cx| this.discover_caldav(cx))),
                                )
                                .when_some(self.subscription_discover_error.as_ref(), |el, err| {
                                    el.child(div().flex_1().min_w_0().child(Label::new(err.clone()).size(LabelSize::XSmall).color(Color::Error).truncate()))
                                }),
                        )
                        .when(!self.subscription_discovered.is_empty(), |el| {
                            el.child(
                                h_flex()
                                    .px_4()
                                    .py_1()
                                    .flex_wrap()
                                    .gap_1p5()
                                    .children(self.subscription_discovered.iter().enumerate().map(|(i, cal)| {
                                        let cal = cal.clone();
                                        let selected = self.subscription_url.read(cx).text().trim() == cal.url;
                                        div()
                                            .id(SharedString::from(format!("caldav-discovered-{i}")))
                                            .px_2p5()
                                            .h(px(26.))
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(if selected { colors.accent } else { colors.border })
                                            .when(selected, |el| el.bg(colors.accent.opacity(0.12)))
                                            .when(!selected, |el| el.hover(|s| s.bg(colors.ghost_element_hover)))
                                            .cursor_pointer()
                                            .child(Icon::new(IconName::CalendarPlus).size(IconSize::XSmall).color(Color::Muted))
                                            .child(Label::new(cal.name.clone()).size(LabelSize::XSmall))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let name = cal.name.clone();
                                                let url = cal.url.clone();
                                                this.subscription_name.update(cx, |input, cx| input.set_text(name, cx));
                                                this.subscription_url.update(cx, |input, cx| input.set_text(url, cx));
                                                this.subscription_writable = cal.writable;
                                                cx.notify();
                                            }))
                                    })),
                            )
                        })
                })
                .child(h_flex().h(px(40.)).px_4().gap_4().child(div().w(px(80.)).child(Label::new(strings.color))).child(swatches))
                .child(
                    h_flex()
                        .px_4()
                        .py_2()
                        .gap_2()
                        .justify_end()
                        .child(form_button("subscription-cancel", strings.cancel, false, &colors).on_click(cx.listener(|this, _, _, cx| {
                            this.subscription_form = None;
                            cx.notify();
                        })))
                        .child(
                            form_button("subscription-save", strings.ok, true, &colors)
                                .when(!has_url, |el| el.opacity(0.45).cursor_default())
                                .on_click(cx.listener(|this, _, _, cx| this.save_subscription_form(cx))),
                        ),
                )
                .into_any_element()
        });
        let mut page = vec![
            div().pb_3().child(Label::new(strings.subscribed_calendars).weight(FontWeight::MEDIUM)).into_any_element(),
            div().pb_2().px_1().text_size(px(12.)).text_color(colors.text_muted).child(strings.subscriptions_hint).into_any_element(),
        ];
        if !rows.is_empty() {
            page.push(group().children(rows).into_any_element());
        }
        if chinese || has_china_holidays {
            let message = match china_status {
                ChinaHolidayStatus::Idle => None,
                ChinaHolidayStatus::Reading => Some((strings.subscription_reading, Color::Muted)),
                ChinaHolidayStatus::Failed => Some((strings.holiday_source_failed, Color::Error)),
            };
            if let Some((text, color)) = message {
                page.push(div().px_1().child(Label::new(text).size(LabelSize::Small).color(color)).into_any_element());
            }
        }
        match form {
            Some(form) => page.push(form),
            None => {
                // The original's Holidays: the public holidays of the language's country, a
                // calendar like any other once added.
                let has_holidays = if chinese {
                    has_china_holidays
                } else {
                    feeds.calendars().iter().any(|c| c.subscription.url == strings.holidays_url)
                };
                page.push(
                    h_flex()
                        .px_1()
                        .gap_5()
                        .child(
                            div()
                                .id("subscription-add")
                                .cursor_pointer()
                                .child(Label::new(strings.add_calendar).color(Color::Accent))
                                .on_click(cx.listener(|this, _, window, cx| this.open_subscription_form(None, window, cx))),
                        )
                        .when(!has_holidays, |el| {
                            if chinese {
                                return el.child(self.render_china_holiday_source(None, can_choose_source, cx));
                            }
                            el.child(
                                div()
                                    .id("subscription-holidays")
                                    .cursor_pointer()
                                    .child(Label::new(strings.add_holidays).color(Color::Accent))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        let strings = i18n::t(cx);
                                        if let Some(feeds) = crate::subscriptions::feeds(cx) {
                                            feeds.update(cx, |feeds, cx| feeds.add(strings.holidays_name, strings.holidays_url, Some("#22C55E"), cx));
                                        }
                                    })),
                            )
                        })
                        .into_any_element(),
                )
            }
        }
        page
    }

    fn render_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let group = || v_flex().py_1p5().rounded(px(8.)).bg(colors.surface_background);
        let title = |text: &'static str| div().pb_3().child(Label::new(text).weight(FontWeight::MEDIUM));
        let body = v_flex()
            .id("settings-page")
            .debug_selector(|| "settings-page".to_owned())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .pt_1()
            .px_6()
            .pb_6()
            .gap_4();
        let page = match self.page {
            Page::SmartList => {
                let row = |this: &Self, list: SmartList, cx: &mut Context<Self>| {
                    let (icon, label) = smart_list_label(list, strings);
                    let current = this.preferences.visibility(list);
                    let options = list
                        .options()
                        .iter()
                        .map(|&visibility| {
                            let pick: SelectPick = Box::new(
                                move |this, window, cx| this.update(window, cx, |p| p.set_visibility(list, visibility)),
                            );
                            (visibility_label(list, visibility, strings).into(), visibility == current, pick)
                        })
                        .collect();
                    this.select_row(
                        Some(icon),
                        label,
                        visibility_label(list, current, strings).into(),
                        Menu::Visibility(list),
                        options,
                        cx,
                    )
                };
                let [all, today, tomorrow, next7, inbox, summary, tags, filters, completed, wont_do, trash] = SmartList::ALL;
                body.child(title(strings.settings_pages[0]))
                    .child(group().children([all, today, tomorrow, next7, inbox, summary].map(|l| row(self, l, cx))))
                    .child(group().children([tags, filters].map(|l| row(self, l, cx))))
                    .child(group().children([completed, wont_do, trash].map(|l| row(self, l, cx))))
                    .into_any_element()
            }
            Page::DateTime => {
                let week_start = self.preferences.week_start;
                let options = WEEK_STARTS
                    .into_iter()
                    .zip(strings.week_start_names)
                    .map(|(day, name)| {
                        let pick: SelectPick =
                            Box::new(move |this, window, cx| this.update(window, cx, |p| p.week_start = day));
                        (SharedString::from(name), day == week_start, pick)
                    })
                    .collect();
                let current = WEEK_STARTS.iter().position(|d| *d == week_start).unwrap_or(0);
                let hour12 = self.preferences.hour12;
                let week_numbers = self.preferences.week_numbers;
                let countdown_dates = self.preferences.countdown_dates;
                // The original shows each format on the year's last day.
                let sample = chrono::NaiveDate::from_ymd_opt(chrono::Local::now().year(), 12, 31).expect("December 31 exists");
                let current_format = self.preferences.date_format;
                let date_formats = DateFormat::ALL
                    .into_iter()
                    .map(|format| {
                        let pick: SelectPick =
                            Box::new(move |this, window, cx| this.update(window, cx, |p| p.date_format = Some(format)));
                        let label = calendar::format_date_as(sample, Some(format), false, true);
                        let selected = current_format == Some(format)
                            || (current_format.is_none() && label == calendar::format_date(sample, strings));
                        (SharedString::from(label), selected, pick)
                    })
                    .collect();
                let formats = [false, true]
                    .into_iter()
                    .map(|twelve| {
                        let pick: SelectPick =
                            Box::new(move |this, window, cx| this.update(window, cx, |p| p.hour12 = twelve));
                        (SharedString::from(strings.time_formats[twelve as usize]), twelve == hour12, pick)
                    })
                    .collect();
                body.child(
                    v_flex()
                        .child(title(strings.settings_pages[1]))
                        .child(group().child(self.select_row(
                            None,
                            strings.time_format,
                            strings.time_formats[hour12 as usize].into(),
                            Menu::TimeFormat,
                            formats,
                            cx,
                        ))
                        .child(self.select_row(
                            None,
                            strings.date_format,
                            calendar::format_date(sample, strings).into(),
                            Menu::DateFormat,
                            date_formats,
                            cx,
                        ))
                        .child(self.select_row(
                            None,
                            strings.start_week_on,
                            strings.week_start_names[current].into(),
                            Menu::WeekStart,
                            options,
                            cx,
                        ))),
                )
                .child(
                    group()
                        .child(
                            h_flex()
                                .id("week-numbers")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.show_week_numbers)))
                                .child(Switch::new().toggle_state(week_numbers))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.update(window, cx, |p| p.week_numbers = !week_numbers)
                                })),
                        )
                        .child(
                            h_flex()
                                .id("countdown-dates")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.show_countdown_mode)))
                                .child(Switch::new().toggle_state(countdown_dates))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.update(window, cx, |p| p.countdown_dates = !countdown_dates)
                                })),
                        ),
                )
                .into_any_element()
            }
            Page::Appearance => body.child(self.render_appearance(cx)).into_any_element(),
            Page::More => {
                let current = i18n::locale(cx);
                let languages = Locale::available()
                    .map(|locale| {
                        let pick: SelectPick =
                            Box::new(move |this, _, cx| this.set_locale(locale, cx));
                        (SharedString::from(locale.native_name()), locale == current, pick)
                    })
                    .collect();
                let lists = &self.cached_lists;
                let default_list = self.preferences.default_list;
                let list_name = default_list
                    .and_then(|id| lists.iter().find(|l| l.id == id))
                    .map(|l| SharedString::from(l.name.clone()))
                    .unwrap_or(strings.inbox.into());
                let mut list_options: Vec<SelectOption> =
                    vec![(
                        strings.inbox.into(),
                        default_list.is_none(),
                        Box::new(|this, window, cx| this.update(window, cx, |p| p.default_list = None)),
                    )];
                list_options.extend(lists.iter().filter(|l| !l.archived).map(|list| {
                    let id = list.id;
                    let pick: SelectPick =
                        Box::new(move |this, window, cx| this.update(window, cx, |p| p.default_list = Some(id)));
                    (SharedString::from(list.name.clone()), default_list == Some(id), pick)
                }));
                let position_row = |label, menu, bottom, set: fn(&mut fern_core::TaskDefaults, bool), cx: &mut Context<Self>| {
                    let top_label = strings.add_to_top;
                    let bottom_label = strings.add_to_bottom;
                    let options = [(false, top_label), (true, bottom_label)]
                        .into_iter()
                        .map(|(choice, name)| {
                            let pick: SelectPick =
                                Box::new(move |this, window, cx| this.update(window, cx, |p| set(&mut p.task_defaults, choice)));
                            (name.into(), choice == bottom, pick)
                        })
                        .collect();
                    let value = if bottom { bottom_label } else { top_label };
                    self.select_row(None, label, value.into(), menu, options, cx)
                };
                let defaults = &self.preferences.task_defaults;
                body.child(group().debug_selector(|| "settings-language".to_owned()).child(self.select_row(
                    None,
                    strings.language,
                    current.native_name().into(),
                    Menu::Language,
                    languages,
                    cx,
                )))
                .child({
                    let r = self.preferences.recognition;
                    type RecognitionCheck = (&'static str, bool, fn(&mut fern_core::Recognition));
                    // A switch row with a hint, and the checkbox under it while the switch is on.
                    let section = |id: &'static str,
                                   label: &'static str,
                                   hint: &'static str,
                                   on: bool,
                                   toggle: fn(&mut fern_core::Recognition),
                                   check: Option<RecognitionCheck>,
                                   cx: &mut Context<Self>| {
                        v_flex()
                            .px_4()
                            .py_1p5()
                            .gap_1()
                            .child(
                                h_flex()
                                    .id(id)
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(label)))
                                    .child(Switch::new().toggle_state(on))
                                    .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| toggle(&mut p.recognition)))),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.text_muted).child(hint))
                            .when_some(check.filter(|_| on), |el, (text, checked, flip)| {
                                el.child(
                                    h_flex()
                                        .id(SharedString::from(format!("{id}-remove")))
                                        .gap_2()
                                        .pt_0p5()
                                        .cursor_pointer()
                                        .child(Checkbox::new(SharedString::from(format!("{id}-check"))).accent().toggle_state(checked))
                                        .child(Label::new(text).size(LabelSize::Small))
                                        .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| flip(&mut p.recognition)))),
                                )
                            })
                    };
                    v_flex().child(title(strings.smart_recognition)).child(
                        group()
                            .child(section(
                                "recognize-dates",
                                strings.date_recognition,
                                strings.date_recognition_hint,
                                r.dates,
                                |r| r.dates = !r.dates,
                                Some((strings.remove_date_text, r.remove_date_text, |r| r.remove_date_text = !r.remove_date_text)),
                                cx,
                            ))
                            .child(section(
                                "recognize-tags",
                                strings.tag_recognition,
                                strings.tag_recognition_hint,
                                r.tags,
                                |r| r.tags = !r.tags,
                                Some((strings.remove_tag_text, r.remove_tag_text, |r| r.remove_tag_text = !r.remove_tag_text)),
                                cx,
                            )),
                    )
                })
                .child({
                    let defaults_modified = self.preferences.task_defaults != fern_core::TaskDefaults::default()
                        || self.preferences.default_list.is_some();
                    v_flex()
                        .child(
                            h_flex()
                                .items_center()
                                .child(div().flex_1().child(title(strings.task_default)))
                                .when(defaults_modified, |el| {
                                    el.child(
                                        div()
                                            .id("reset-task-defaults")
                                            .px_2()
                                            .py_0p5()
                                            .rounded(px(4.))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(colors.element_hover))
                                            .child(Label::new(strings.habit_reset).size(LabelSize::XSmall).color(Color::Accent))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.update(window, cx, |p| {
                                                    p.task_defaults = fern_core::TaskDefaults::default();
                                                    p.default_list = None;
                                                });
                                            })),
                                    )
                                }),
                        )
                        .child(self.render_task_defaults(cx))
                        .child(div().h_3())
                        .child(group().children(self.render_priority_and_tag(cx)).child(self.select_row(
                            None,
                            strings.default_list,
                            list_name,
                            Menu::DefaultList,
                            list_options,
                            cx,
                        )))
                        .child(
                            div()
                                .w_full()
                                .pt_1p5()
                                .px_1()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .child(strings.default_list_hint),
                        )
                        .child(div().h_3())
                        .child(group().children([
                            position_row(
                                strings.default_add_to,
                                Menu::AddTo,
                                defaults.add_to_bottom,
                                |defaults, bottom| defaults.add_to_bottom = bottom,
                                cx,
                            ),
                            position_row(
                                strings.overdue_section_at,
                                Menu::OverdueAt,
                                defaults.overdue_last,
                                |defaults, bottom| defaults.overdue_last = bottom,
                                cx,
                            ),
                        ]))
                })
                .child({
                    let on = self.preferences.mini_calendar;
                    v_flex().child(title(strings.mini_calendar)).child(
                        group().child(
                            h_flex()
                                .id("mini-calendar")
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .cursor_pointer()
                                .child(div().flex_1().child(Label::new(strings.show_mini_calendar)))
                                .child(Switch::new().toggle_state(on))
                                .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| p.mini_calendar = !on))),
                        ),
                    )
                })
                .into_any_element()
            }
            Page::Notifications => {
                let n = self.preferences.notifications;
                let values = [n.daily.is_some(), n.desktop, n.constant];
                let toggles: [fn(&mut fern_core::Notifications); 3] = [
                    |n| n.daily = if n.daily.is_some() { None } else { chrono::NaiveTime::from_hms_opt(9, 0, 0) },
                    |n| n.desktop = !n.desktop,
                    |n| n.constant = !n.constant,
                ];
                let times: Vec<SelectOption> = (5..=23u32)
                    .filter_map(|h| chrono::NaiveTime::from_hms_opt(h, 0, 0))
                    .map(|time| {
                        let option: SelectOption = (
                            calendar::format_time(time, strings).into(),
                            n.daily == Some(time),
                            Box::new(move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                                this.update(window, cx, |p| p.notifications.daily = Some(time))
                            }),
                        );
                        option
                    })
                    .collect();
                let mut daily_time = n.daily.map(|t| self.select_row(None, strings.daily_time, calendar::format_time(t, strings).into(), Menu::DailyTime, times, cx));
                let body = body.child(title(strings.notifications_page)).child(group().children(
                    strings.notification_items.into_iter().zip(values).zip(toggles).enumerate().map(|(i, (((name, hint), on), toggle))| {
                        v_flex()
                            .px_4()
                            .py_1p5()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("notification-{i}")))
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(name)))
                                    .child(Switch::new().toggle_state(on))
                                    .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| toggle(&mut p.notifications)))),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.text_muted).child(hint))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .enumerate()
                    .flat_map(|(i, row)| {
                        let extra = if i == 0 { daily_time.take() } else { None };
                        std::iter::once(row).chain(extra)
                    })
                    .collect::<Vec<_>>(),
                ));
                let sound_enabled = self.sound_enabled;
                let body = body.child(group().child(
                    h_flex()
                        .id("completion-sound")
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .cursor_pointer()
                        .child(div().flex_1().child(Label::new(strings.completion_sound)))
                        .child(Switch::new().toggle_state(sound_enabled))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let next = !this.sound_enabled;
                            if let Err(err) = desktop_common::sound::set_enabled(&this.store, next) {
                                eprintln!("failed to save the completion sound setting: {err:#}");
                            } else {
                                this.sound_enabled = next;
                                if next {
                                    desktop_common::sound::play_complete(&this.store);
                                }
                            }
                            cx.notify();
                        })),
                ));
                #[cfg(windows)]
                let body = {
                    let enabled = self.minimize_to_tray;
                    let topmost = self.always_on_top;
                    body.child(
                        group()
                            .child(
                                h_flex()
                                    .id("minimize-to-tray")
                                    .h(px(ROW_HEIGHT))
                                    .px_4()
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(strings.minimize_to_tray)))
                                    .child(Switch::new().toggle_state(enabled))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let next = !this.minimize_to_tray;
                                        if let Err(err) = crate::tray::set_enabled(&this.store, next, cx) {
                                            eprintln!("failed to save the notification area setting: {err:#}");
                                        } else {
                                            this.minimize_to_tray = next;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                h_flex()
                                    .id("always-on-top")
                                    .h(px(ROW_HEIGHT))
                                    .px_4()
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(strings.tray_always_on_top)))
                                    .child(Switch::new().toggle_state(topmost))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let next = !this.always_on_top;
                                        if let Err(err) = crate::tray::set_always_on_top(&this.store, next, cx) {
                                            eprintln!("failed to save always_on_top setting: {err:#}");
                                        } else {
                                            this.always_on_top = next;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                };
                body.into_any_element()
            }
            Page::Features => {
                let features = self.preferences.features;
                let values = [features.calendar, features.matrix, features.habits, features.focus, features.countdown];
                let toggles: [fn(&mut fern_core::Features); 5] = [
                    |f| f.calendar = !f.calendar,
                    |f| f.matrix = !f.matrix,
                    |f| f.habits = !f.habits,
                    |f| f.focus = !f.focus,
                    |f| f.countdown = !f.countdown,
                ];
                body.child(title(strings.features)).child(group().children(
                    strings.feature_items.into_iter().zip(values).zip(toggles).enumerate().map(|(i, (((name, hint), on), toggle))| {
                        v_flex()
                            .px_4()
                            .py_1p5()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("feature-{i}")))
                                    .debug_selector(move || format!("settings-feature-{i}"))
                                    .cursor_pointer()
                                    .child(div().flex_1().child(Label::new(name)))
                                    .child(Switch::new().toggle_state(on))
                                    .on_click(cx.listener(move |this, _, window, cx| this.update(window, cx, |p| toggle(&mut p.features)))),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.text_muted).child(hint))
                    }),
                ))
                .into_any_element()
            }
            Page::Sync => {
                let field = |label: &'static str, input: Entity<crate::text_input::TextInput>| {
                    h_flex()
                        .h(px(40.))
                        .px_4()
                        .gap_4()
                        .child(div().w(px(80.)).child(Label::new(label)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .h(px(28.))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(colors.background)
                                .child(input),
                        )
                };
                let folder = crate::data_dir().map(|dir| dir.display().to_string()).unwrap_or_default();
                body.child(title(strings.data_on_device))
                    .child(
                        group().child(
                            h_flex()
                                .h(px(ROW_HEIGHT))
                                .px_4()
                                .gap_4()
                                .child(Label::new(strings.data_folder))
                                .child(
                                    div()
                                        .id("data-folder-path")
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .tooltip(Tooltip::text(folder.clone()))
                                        .child(Label::new(folder).color(Color::Muted).truncate()),
                                )
                                .child(
                                    div()
                                        .id("open-data-folder")
                                        .px_2()
                                        .py_1()
                                        .rounded(px(4.))
                                        .hover(|s| s.bg(colors.element_hover))
                                        .cursor_pointer()
                                        .child(Label::new(strings.open_folder).color(Color::Accent))
                                        .on_click(|_, _, cx| {
                                            if let Ok(dir) = crate::data_dir() {
                                                cx.open_with_system(&dir);
                                            }
                                        }),
                                ),
                        ),
                    )
                    .child(div().pt_2().pb_4().px_1().text_size(px(12.)).text_color(colors.text_muted).child(strings.data_hint))
                    .child(title(strings.sync_optional))
                    .child(
                        group()
                            .child(field(strings.sync_server, self.sync_server.clone()))
                            .child(field(strings.sync_token, self.sync_token.clone())),
                    )
                    .child(div().pt_2().px_1().text_size(px(12.)).text_color(colors.text_muted).child(strings.sync_hint))
                    .into_any_element()
            }
            Page::Integrations => body.children(self.render_integrations(cx)).into_any_element(),
            Page::Sticky => body.child(self.render_sticky(cx)).into_any_element(),
            Page::Shortcuts => return self.render_shortcuts_page(cx),
            Page::About => {
                let row = |label: &'static str, value: &'static str| {
                    h_flex()
                        .h(px(ROW_HEIGHT))
                        .px_4()
                        .child(div().flex_1().child(Label::new(label)))
                        .child(Label::new(value).color(Color::Muted))
                };
                body.child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .py_4()
                        .child(Label::new("Fern").size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                        .child(Label::new(strings.about_tagline).color(Color::Muted)),
                )
                .child(
                    group()
                        .child(row(strings.version, release_notes::CURRENT_VERSION))
                        .children(self.installed_notes.as_ref().map(|notes| {
                            let title = strings.whats_new_in_version.replace("{version}", &notes.version);
                            let notes = notes.clone();
                            action_button("installed-release-notes", cx)
                                .mx_2()
                                .min_h(px(ROW_HEIGHT))
                                .child(div().flex_1().child(Label::new(title)))
                                .child(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(Color::Muted))
                                .on_click(cx.listener(move |this, _, window, cx| this.show_release_notes(notes.clone(), window, cx)))
                        }))
                        .child(row(strings.license, "AGPL-3.0-or-later")),
                )
                .children(self.render_updates(cx))
                .child(
                    v_flex()
                        .child(title(strings.backup_restore))
                        .child(
                            group().child(
                                h_flex()
                                    .h(px(ROW_HEIGHT))
                                    .px_4()
                                    .gap_4()
                                    .child(div().flex_1().child(Label::new(strings.backup_restore)))
                                    .child(
                                        div()
                                            .id("generate-backup")
                                            .px_2()
                                            .py_1()
                                            .rounded(px(4.))
                                            .hover(|s| s.bg(colors.element_hover))
                                            .cursor_pointer()
                                            .child(Label::new(strings.generate_backup).color(Color::Accent))
                                            .on_click(cx.listener(|this, _, _, cx| this.generate_backup(cx))),
                                    )
                                    .child(
                                        div()
                                            .id("import-backup")
                                            .px_2()
                                            .py_1()
                                            .rounded(px(4.))
                                            .hover(|s| s.bg(colors.element_hover))
                                            .cursor_pointer()
                                            .child(Label::new(strings.import_backups).color(Color::Accent))
                                            .on_click(cx.listener(|this, _, _, cx| this.import_backup(cx))),
                                    ),
                            ),
                        )
                        .children(self.backup_message.clone().map(|message| {
                            div().pt_1p5().px_1().text_size(px(11.)).text_color(colors.text_muted).child(message)
                        })),
                )
                .into_any_element()
            }
        };
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(page)
            .child(VerticalScrollbar::for_handle("settings-page-scroll", self.page_scroll.clone()))
            .into_any_element()
    }

    fn render_release_notes(&self, notes: &ReleaseNotes, cx: &App) -> AnyElement {
        let strings = i18n::t(cx);
        let title = strings.whats_new_in_version.replace("{version}", &notes.version);
        let url = notes.url.clone();
        v_scroll(
            SharedString::from(format!("release-notes-scroll-{}", notes.version)),
            v_flex()
                .id(SharedString::from(format!("release-notes-page-{}", notes.version)))
                .debug_selector(|| "release-notes-page".to_owned())
                .px_6()
                .pt_2()
                .pb_6()
                .gap_4()
                .child(Label::new(title).size(LabelSize::Large).weight(FontWeight::SEMIBOLD))
                .child(if notes.markdown.is_empty() {
                    div().text_size(px(14.)).child(strings.release_notes_missing).into_any_element()
                } else {
                    crate::task_detail::render_release_markdown(&notes.markdown, cx)
                })
                .child(
                    action_button("release-notes-github", cx)
                        .self_start()
                        .border_color(cx.theme().colors().border)
                        .child(Label::new(strings.release_on_github).size(LabelSize::Small))
                        .child(Icon::new(IconName::SquareArrowOutUpRight).size(IconSize::Small).color(Color::Muted))
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                ),
        )
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors().clone();
        let strings = i18n::t(cx);
        let notes_open = self.release_notes.is_some();
        let about = strings.settings_pages[5];
        let page = match &self.release_notes {
            Some(notes) => self.render_release_notes(notes, cx),
            None => self.render_page(cx),
        };
        let root = h_flex()
            .id("settings-window")
            .track_focus(&self.focus_handle)
            .tab_group()
            .key_context("Settings")
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.handle_shortcut_key_down(event, cx);
            }))
            .on_action(cx.listener(|this, _: &DismissSettings, window, cx| {
                this.dismiss(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.close(window, cx);
            }))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(colors.background)
            .text_color(colors.text)
            .child(self.render_nav(window, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(colors.background)
                    .child(
                        h_flex()
                            .debug_selector(|| "settings-header".to_owned())
                            .h(px(44.))
                            .flex_none()
                            .px_3()
                            .items_center()
                            .when(notes_open, |el| {
                                el.child(
                                    action_button("release-notes-back", cx)
                                        .child(Icon::new(IconName::ChevronLeft).size(IconSize::Small).color(Color::Muted))
                                        .child(Label::new(about).size(LabelSize::Small))
                                        .on_click(cx.listener(|this, _, window, cx| this.back_to_about(window, cx))),
                                )
                            })
                            .child(self.drag_area("settings-header-drag", window, cx).flex_1().min_w_0().h_full())
                            .when(!cfg!(target_os = "macos"), |el| {
                                el.child(
                                    IconButton::new("close-settings", IconName::X)
                                        .icon_color(Color::Muted)
                                        .tooltip(Tooltip::text(strings.close))
                                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                                )
                            }),
                    )
                    .child(page),
            )
            .children(self.render_reset_shortcuts_confirm(cx));
        crate::title_bar::window_frame(root, window, cx)
    }
}

fn smart_list_label(list: SmartList, strings: &i18n::Strings) -> (IconName, &'static str) {
    match list {
        SmartList::All => (IconName::Layers, strings.all),
        SmartList::Today => (IconName::CalendarCheck, strings.today),
        SmartList::Tomorrow => (IconName::Sunrise, strings.tomorrow),
        SmartList::Next7Days => (IconName::CalendarRange, strings.next_7_days_short),
        SmartList::Inbox => (IconName::Inbox, strings.inbox),
        SmartList::Summary => (IconName::FileText, strings.summary),
        SmartList::Tags => (IconName::Tag, strings.tags),
        SmartList::Filters => (IconName::Funnel, strings.filters),
        SmartList::Completed => (IconName::SquareCheckBig, strings.completed),
        SmartList::WontDo => (IconName::SquareX, strings.wont_do),
        SmartList::Trash => (IconName::Trash, strings.trash),
    }
}

fn visibility_label(list: SmartList, visibility: Visibility, strings: &i18n::Strings) -> &'static str {
    match visibility {
        Visibility::Show => strings.visibility_names[0],
        Visibility::IfNotEmpty if list == SmartList::Tags => strings.visibility_names[3],
        Visibility::IfNotEmpty => strings.visibility_names[1],
        Visibility::Hide => strings.visibility_names[2],
    }
}

/// A small dialog button: the accent for the one that confirms, a frame for the other.
fn form_button(id: &'static str, label: &'static str, primary: bool, colors: &theme::ThemeColors) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(80.))
        .h(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .when(primary, |el| el.bg(colors.accent).hover(|s| s.opacity(0.9)))
        .when(!primary, |el| el.border_1().border_color(colors.border).hover(|s| s.bg(colors.element_hover)))
        .child(Label::new(label).size(LabelSize::Small).when(primary, |label| label.color(Color::OnAccent)))
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[gpui::test]
    fn task_positions_share_a_painted_settings_surface(cx: &mut gpui::TestAppContext) {
        cx.update(theme::init);
        let store = Rc::new(Store::open_in_memory().unwrap());
        let (settings, cx) = cx.add_window_view(|_, cx| {
            let mut settings = SettingsWindow::new(store, cx);
            settings.set_page(Page::More, cx);
            settings
        });
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        for theme in [theme::Theme::light(), theme::Theme::dark()] {
            let surface = theme.colors().surface_background;
            cx.update(|_, cx| {
                theme::set(theme, cx);
                settings.update(cx, |settings, cx| {
                    settings.page_scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            let add_to = cx.debug_bounds("settings-row-AddTo").unwrap();
            let overdue = cx.debug_bounds("settings-row-OverdueAt").unwrap();
            cx.update(|window, _| {
                let scale = window.scale_factor();
                let positions = [add_to.center().scale(scale), overdue.center().scale(scale)];
                assert!(window.painted_quads().iter().any(|quad| {
                    quad.background.as_solid() == Some(surface)
                        && positions.iter().all(|point| {
                            quad.bounds.contains(point) && quad.content_mask.bounds.contains(point)
                        })
                }), "both task position rows must be painted on the same settings surface");
            });
        }
    }

    #[gpui::test]
    fn china_holidays_offer_both_sources_in_settings(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
            cx.set_global(CurrentLocale(Locale::ZhCn));
            crate::subscriptions::init(store.clone(), cx);
        });
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let mut settings = SettingsWindow::new(store, cx);
            settings.set_page(Page::Integrations, cx);
            window.focus(&settings.focus_handle, cx);
            settings
        });
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        let trigger = cx.debug_bounds("china-holiday-source").unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        for source in ChinaHolidaySource::ALL {
            assert!(cx.debug_bounds(source.label()).is_some());
        }
        cx.simulate_keystrokes("escape");
        assert!(cx.debug_bounds("china-holiday-source-menu").is_none());
        cx.update(|_, cx| assert_eq!(settings.read(cx).page, Page::Integrations));
    }

    #[gpui::test]
    fn caldav_form_saves_and_preserves_credentials_on_visibility_toggle(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
            cx.set_global(CurrentLocale(Locale::En));
            crate::subscriptions::init(store.clone(), cx);
        });
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let mut settings = SettingsWindow::new(store.clone(), cx);
            settings.set_page(Page::Integrations, cx);
            window.focus(&settings.focus_handle, cx);
            settings
        });
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        // Open add form, switch to CalDAV + Fastmail preset, fill credentials, save.
        cx.update(|window, cx| {
            settings.update(cx, |s, cx| s.open_subscription_form(None, window, cx));
        });
        let caldav_btn = cx.debug_bounds("subscription-kind-caldav").unwrap().center();
        cx.simulate_click(caldav_btn, gpui::Modifiers::default());
        let fastmail_btn = cx.debug_bounds("caldav-provider-fastmail").unwrap().center();
        cx.simulate_click(fastmail_btn, gpui::Modifiers::default());
        settings.update(cx, |s, cx| {
            s.subscription_name.update(cx, |i, cx| i.set_text("Fastmail Work", cx));
            s.subscription_username.update(cx, |i, cx| i.set_text("alice@fastmail.com", cx));
            s.subscription_password.update(cx, |i, cx| i.set_text("app-secret", cx));
            s.save_subscription_form(cx);
        });
        cx.run_until_parked();

        let subs = store.subscriptions().unwrap();
        assert_eq!(subs.len(), 1);
        let id = subs[0].id;
        assert_eq!(subs[0].kind, fern_core::ical::CalendarKind::CalDav);
        assert_eq!(subs[0].provider, fern_core::ical::CalendarProvider::Fastmail);
        assert_eq!(subs[0].username, "alice@fastmail.com");
        assert_eq!(subs[0].password, "app-secret");
        assert!(subs[0].writable);

        // Toggle visibility -> CalDAV credentials/provider remain intact.
        let show_sel: &'static str = Box::leak(format!("subscription-show-{id}").into_boxed_str());
        let show_btn = cx.debug_bounds(show_sel).unwrap().center();
        cx.simulate_click(show_btn, gpui::Modifiers::default());
        cx.run_until_parked();
        let after = store.subscriptions().unwrap();
        assert!(after[0].hidden);
        assert_eq!(after[0].kind, fern_core::ical::CalendarKind::CalDav);
        assert_eq!(after[0].provider, fern_core::ical::CalendarProvider::Fastmail);
        assert_eq!(after[0].password, "app-secret");
    }

    #[gpui::test]
    fn shortcuts_page_records_cancels_with_escape_and_confirms_reset(cx: &mut gpui::TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
            cx.set_global(CurrentLocale(Locale::En));
        });
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let mut settings = SettingsWindow::new(store, cx);
            settings.set_page(Page::Shortcuts, cx);
            window.focus(&settings.focus_handle, cx);
            settings
        });
        cx.simulate_resize(gpui::size(px(900.), px(720.)));

        // Shortcuts page and sticky Reset button are rendered.
        assert!(cx.debug_bounds("settings-shortcuts-page").is_some());
        let first_pill = cx.debug_bounds("shortcut-action-0").unwrap();
        let reset_btn = cx.debug_bounds("shortcuts-reset-all").unwrap();

        // Clicking an action pill enters recording mode and shows the rule hint bar.
        cx.simulate_click(first_pill.center(), gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert_eq!(
                settings.read(cx).recording_shortcut,
                Some(shortcuts_settings_view::RecordingTarget::Action(0))
            );
            assert!(!settings.read(cx).recording_rejected);
        });
        assert!(cx.debug_bounds("shortcuts-recording-hint").is_some());

        // Pressing a bare letter on a Workspace-scoped shortcut flags rejection without closing.
        cx.simulate_keystrokes("a");
        cx.update(|_, cx| {
            assert_eq!(
                settings.read(cx).recording_shortcut,
                Some(shortcuts_settings_view::RecordingTarget::Action(0))
            );
            assert!(settings.read(cx).recording_rejected);
        });

        // Escape cancels recording and clears the rejection state without closing Settings.
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            assert_eq!(settings.read(cx).recording_shortcut, None);
            assert!(!settings.read(cx).recording_rejected);
            assert!(!settings.read(cx).closing);
        });
        assert!(cx.debug_bounds("shortcuts-recording-hint").is_none());

        // Clicking Reset opens the confirmation modal; Escape dismisses it without closing Settings.
        cx.simulate_click(reset_btn.center(), gpui::Modifiers::default());
        cx.update(|_, cx| assert!(settings.read(cx).confirm_reset_shortcuts));
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| {
            assert!(!settings.read(cx).confirm_reset_shortcuts);
            assert!(!settings.read(cx).closing);
        });

        // Keymap parse errors surface cleanly in the banner at the top of Page::Shortcuts.
        cx.update(|_, cx| {
            crate::keymap::set_errors(vec!["keymap.json: test error".to_owned()], cx);
            settings.update(cx, |_, cx| cx.notify());
        });
        assert!(cx.debug_bounds("shortcuts-keymap-errors").is_some());
    }
}
