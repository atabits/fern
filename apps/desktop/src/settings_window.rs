//! The Settings window from the account menu, modeled on the original's separate desktop window:
//! pages on the left, grouped rows on the right, values picked from small dropdowns.

use std::rc::Rc;

use fern_core::{Preferences, SmartList, Store};
use gpui::{
    Bounds, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, MouseButton, ScrollHandle,
    TitlebarOptions, WindowBounds, WindowControlArea, WindowDecorations, WindowHandle, WindowOptions,
    point, size,
};
use ui::prelude::*;
use ui::{VerticalScrollbar, v_scroll};

use crate::actions::{CloseWindow, DismissSettings};
use crate::calendar;
use crate::i18n::{self, CurrentLocale, Locale};
use crate::release_notes::{self, ReleaseNotes, action_button};
#[path = "appearance_view.rs"]
mod appearance_view;
mod backups;
mod backups_diagnostics;
mod integrations;
mod pages;
#[path = "shortcuts_settings_view.rs"]
mod shortcuts_settings_view;
#[path = "sticky_settings_view.rs"]
mod sticky_settings_view;
mod updates;

/// A row of a Settings page, and of the sections column, as the original's.
const ROW_HEIGHT: f32 = 40.;
pub const WIDTH: f32 = 840.;
pub const HEIGHT: f32 = 680.;
const MIN_WIDTH: f32 = 640.;
const MIN_HEIGHT: f32 = 400.;
const SYNC_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

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
    Plugins,
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
    /// Automatic daily backups discovered off the UI thread (cached so `render()` never does fs I/O).
    auto_backups: Vec<fern_core::store::BackupEntry>,
    auto_backups_task: Option<gpui::Task<()>>,
    /// Cached `PRAGMA user_version` for the Database Integrity status row.
    schema_version: usize,
    /// Backup waiting for confirmation before staging `restore.db` for the next launch.
    pending_restore: Option<std::path::PathBuf>,
    /// Outcome of the last "Save Diagnostic Log" action (`(message, is_error)`).
    diagnostic_message: Option<(String, bool)>,
    #[cfg(test)]
    backups_dir_override: Option<std::path::PathBuf>,
    #[cfg(test)]
    restore_target_override: Option<std::path::PathBuf>,
    /// Error saving Sticky Note preferences (kept separate from backup/import messages).
    sticky_error: Option<String>,
    /// Cached lookups so `render()` never runs synchronous SQLite queries per frame.
    cached_tags: Vec<fern_core::Tag>,
    cached_lists: Vec<fern_core::List>,
    sound_enabled: bool,
    minimize_to_tray: bool,
    always_on_top: bool,
    installed_notes: Option<Rc<ReleaseNotes>>,
    release_notes: Option<Rc<ReleaseNotes>>,
    /// Settings → Sync: server address, token, and optional E2EE passphrase,
    /// debounced on edit and flushed on submit/close.
    sync_server: Entity<crate::text_input::TextInput>,
    sync_token: Entity<crate::text_input::TextInput>,
    sync_passphrase: Entity<crate::text_input::TextInput>,
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
    pending_theme_switch: Option<std::sync::Arc<theme::Theme>>,
    theme_save_error: Option<String>,
    pending_delete_subscription: Option<(fern_core::Id, String)>,
    plugin_settings: Entity<plugin_ui::PluginSettingsView>,
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
    let appearance = if cx.theme().is_dark() { "dark" } else { "light" };
    plugin_ui::PluginHost::set_appearance(None, Some(appearance.to_owned()), cx);
    calendar::set_week_start(preferences.week_start);
    calendar::set_hour12(preferences.hour12);
    calendar::set_date_format(preferences.date_format);
    calendar::set_week_numbers(preferences.week_numbers);
    crate::app_view::set_horizons_enabled(preferences.features.horizons);
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
        let schema_version = store.schema_version().unwrap_or(fern_core::SCHEMA_VERSION);
        let sound_enabled = desktop_common::sound::enabled(&store);
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
        let sync_passphrase = field(i18n::t(cx).sync_passphrase_placeholder, config.passphrase, true, cx);
        let updates = crate::updater::updater(cx).map(|updater| cx.observe(&updater, |_, _, cx| cx.notify()));
        let feeds = crate::subscriptions::feeds(cx).map(|feeds| cx.observe(&feeds, |_, _, cx| cx.notify()));
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            this.refresh_cached_collections(cx);
        })
        .detach();
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
        let plugin_settings = cx.new(|cx| plugin_ui::PluginSettingsView::new(store.clone(), cx));
        let mut this = Self {
            store,
            preferences,
            page: Page::default(),
            page_scroll: ScrollHandle::new(),
            menu: None,
            focus_handle: cx.focus_handle(),
            closing: false,
            should_move: false,
            backup_message: None,
            auto_backups: Vec::new(),
            auto_backups_task: None,
            schema_version,
            pending_restore: None,
            diagnostic_message: None,
            #[cfg(test)]
            backups_dir_override: None,
            #[cfg(test)]
            restore_target_override: None,
            sticky_error: None,
            cached_tags,
            cached_lists,
            sound_enabled,
            minimize_to_tray: tray_prefs.minimize_to_tray,
            always_on_top: tray_prefs.always_on_top,
            installed_notes,
            release_notes: None,
            sync_server,
            sync_token,
            sync_passphrase,
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
            pending_theme_switch: None,
            theme_save_error: None,
            pending_delete_subscription: None,
            plugin_settings,
            _updates: updates,
            _feeds: feeds,
        };
        this.refresh_auto_backups(cx);
        this
    }

    /// Switches to Settings → Plugins and focuses the Logs tab for `plugin_id`.
    pub fn open_plugin_logs(&mut self, plugin_id: &str, cx: &mut Context<Self>) {
        self.set_page(Page::Plugins, cx);
        let plugin_id = plugin_id.to_owned();
        self.plugin_settings.update(cx, |view, cx| {
            view.open_logs_for(&plugin_id, cx);
        });
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
                this.plugin_settings.update(cx, |view, cx| view.close_plugin_page(cx));
                if !this.closing {
                    this.closing = true;
                    cx.emit(SettingsEvent::Dismissed);
                }
            })
            .ok();
            true
        });
        cx.observe_global_in::<CurrentLocale>(window, |this, window, cx| {
            let strings = i18n::t(cx);
            window.set_window_title(strings.settings);
            this.sync_passphrase.update(cx, |input, cx| {
                input.set_placeholder(strings.sync_passphrase_placeholder, cx);
            });
            this.subscription_name.update(cx, |input, cx| {
                input.set_placeholder(strings.subscription_name_placeholder, cx);
            });
        })
        .detach();
    }

    pub(crate) fn is_closing(&self) -> bool {
        self.closing
    }

    pub(crate) fn close_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_sync_save(cx);
        self.plugin_settings.update(cx, |view, cx| view.close_plugin_page(cx));
        self.closing = true;
        window.remove_window();
    }

    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_sync_save(cx);
        self.plugin_settings.update(cx, |view, cx| view.close_plugin_page(cx));
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
            passphrase: self.sync_passphrase.read(cx).text().to_owned(),
        };
        match self.store.set_sync_config(&config) {
            Ok(()) => cx.emit(SettingsEvent::Changed),
            Err(err) => eprintln!("failed to save the sync settings: {err:#}"),
        }
    }

    pub fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        let changed = self.page != page;
        if changed && self.page == Page::Sync {
            self.flush_sync_save(cx);
        }
        if changed && self.page == Page::Plugins {
            self.plugin_settings.update(cx, |view, cx| view.close_plugin_page(cx));
        }
        if changed {
            self.page_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.page = page;
        self.menu = None;
        self.release_notes = None;
        self.pending_restore = None;
        self.recording_shortcut = None;
        self.recording_rejected = false;
        self.confirm_reset_shortcuts = false;
        self.pending_theme_switch = None;
        self.theme_save_error = None;
        self.pending_delete_subscription = None;
        if page == Page::About {
            self.refresh_auto_backups(cx);
        }
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

    fn dismiss_plugin_settings_escape(&mut self, cx: &mut Context<Self>) -> bool {
        self.plugin_settings.update(cx, |view, cx| view.handle_escape(cx))
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::app_view::dismiss_plugin_modal(cx) {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else if self.pending_theme_switch.take().is_some()
            || self.theme_save_error.take().is_some()
            || self.pending_delete_subscription.take().is_some()
            || self.pending_restore.take().is_some()
        {
            cx.notify();
        } else if self.confirm_reset_shortcuts {
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
        } else if self.page == Page::Plugins && self.dismiss_plugin_settings_escape(cx) {
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

    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting("locale", locale.code()) {
            eprintln!("failed to save the language: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        plugin_ui::PluginHost::set_appearance(Some(locale.code().to_owned()), None, cx);
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
            self.backup_message = Some(backups::format_ticktick_import(
                self.store.import_ticktick(&text),
                strings,
            ));
            crate::sticky_note::data_changed(cx.entity_id(), cx);
        }
        let page = match page {
            "smart-list" => Page::SmartList,
            "appearance" => Page::Appearance,
            "more" => Page::More,
            "about" => Page::About,
            "sync" => Page::Sync,
            "integrations" => Page::Integrations,
            "plugins" => Page::Plugins,
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
                        .child(
                            item("settings-plugins", IconName::Code, strings.plugins, self.page == Page::Plugins)
                                .end_slot(Chip::new(strings.beta_badge, cx.theme().status().warning))
                                .on_click(cx.listener(|this, _, _, cx| this.set_page(Page::Plugins, cx))),
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
            .children(self.render_reset_shortcuts_confirm(cx))
            .children(self.render_restore_backup_confirm(cx))
            .children(self.render_delete_subscription_confirm(cx))
            .children(self.render_theme_confirm(cx))
            .child(plugin_ui::render_plugin_overlays(window, cx));
        crate::title_bar::window_frame(root, window, cx)
    }
}

#[cfg(all(test, debug_assertions))]
mod tests;
