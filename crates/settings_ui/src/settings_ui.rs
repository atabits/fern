//! Host-decoupled Settings window and preference management UI.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use desktop_common::actions::{CloseWindow, DismissTransient as DismissSettings};
use desktop_common::release_notes::{self, ReleaseNotes};
use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use desktop_common::user_themes;
use desktop_common::window_chrome::{CsdWindowOptions, WindowDragState, apply_drag_strip, csd_window_options};
use desktop_common::ActivePreferences;
use desktop_sync::subscriptions::{self, DiscoveredCalendar};
use fern_core::ical::{CalendarKind, CalendarProvider};
use fern_core::store::BackupEntry;
use fern_core::{Id, Preferences, SmartList, Store};
use fern_i18n::{self as i18n, CurrentLocale, Locale, Strings};
use gpui::{
    AnyView, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Keystroke, Subscription,
    Task, WindowHandle, point, size,
};
use theme::Theme;
use ui::prelude::*;
use ui::v_scroll;

mod appearance_view;
mod backups;
mod backups_diagnostics;
mod integrations;
mod pages;
mod shortcuts_settings_view;
mod sticky_settings_view;
mod updates;

const ROW_HEIGHT: f32 = 40.;
pub const WIDTH: f32 = 840.;
pub const HEIGHT: f32 = 680.;
const MIN_WIDTH: f32 = 640.;
const MIN_HEIGHT: f32 = 400.;
const SYNC_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    Features,
    Notifications,
    SmartList,
    #[default]
    DateTime,
    Appearance,
    More,
    Sync,
    Integrations,
    Plugins,
    Sticky,
    Shortcuts,
    About,
}

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
    ChinaHolidays(Option<Id>),
}

pub enum SettingsEvent {
    Changed,
    Dismissed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalShortcutEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub binding: Option<String>,
    pub taken: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdateUiStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    Ready(Rc<ReleaseNotes>),
    Available(Rc<ReleaseNotes>),
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateSnapshot {
    pub supported: bool,
    pub automatic: bool,
    pub status: UpdateUiStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrayState {
    pub minimize_to_tray: bool,
    pub always_on_top: bool,
}

pub type RenderOverlaysFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
pub type PluginLogOpenerFn = Rc<dyn Fn(&str, &mut App)>;
pub type AppearanceChangedFn = Rc<dyn Fn(Option<String>, Option<String>, &mut App)>;
pub type TrayStateFn = Rc<dyn Fn(&Store, &mut App) -> TrayState>;
pub type TrayToggleFn = Rc<dyn Fn(&Store, bool, &mut App) -> anyhow::Result<()>>;
pub type SettingsObserverFn = Rc<dyn Fn(&mut Context<SettingsWindow>) -> Option<Subscription>>;
pub type GlobalShortcutsFn = Rc<dyn Fn(&'static Strings, &App) -> Vec<GlobalShortcutEntry>>;
pub type SetGlobalHotkeyFn = Rc<dyn Fn(usize, Option<&Keystroke>, &mut App) -> bool>;
pub type AutoUpdateToggleFn = Rc<dyn Fn(bool, &mut App)>;

/// Host capabilities injected by `apps/desktop` so `settings_ui` has zero sibling `*_ui`
/// or OS tray/hotkey/updater dependencies.
#[derive(Clone)]
pub struct SettingsHost {
    pub plugin_settings_view: AnyView,
    pub close_plugin_page: Rc<dyn Fn(&mut App)>,
    pub handle_plugin_escape: Rc<dyn Fn(&mut App) -> bool>,
    pub dismiss_plugin_modal: Rc<dyn Fn(&mut App) -> bool>,
    pub render_plugin_overlays: RenderOverlaysFn,
    pub open_plugin_logs: PluginLogOpenerFn,
    pub on_appearance_changed: AppearanceChangedFn,
    pub tray_state: TrayStateFn,
    pub set_minimize_to_tray: TrayToggleFn,
    pub set_always_on_top: TrayToggleFn,
    pub observe_tray: SettingsObserverFn,
    pub global_shortcuts: GlobalShortcutsFn,
    pub set_global_hotkey: SetGlobalHotkeyFn,
    pub reset_global_hotkeys: Rc<dyn Fn(&mut App)>,
    pub updater_snapshot: Rc<dyn Fn(&App) -> UpdateSnapshot>,
    pub set_auto_update: AutoUpdateToggleFn,
    pub check_for_updates: Rc<dyn Fn(&mut App)>,
    pub restart_to_update: Rc<dyn Fn(&mut App)>,
    pub observe_updater: SettingsObserverFn,
}

struct EmptyHostView;

impl Render for EmptyHostView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl SettingsHost {
    /// Minimal no-op host suitable for standalone `settings_ui` tests.
    pub fn noop(cx: &mut App) -> Self {
        let plugin_settings_view: AnyView = cx.new(|_| EmptyHostView).into();
        Self {
            plugin_settings_view,
            close_plugin_page: Rc::new(|_| {}),
            handle_plugin_escape: Rc::new(|_| false),
            dismiss_plugin_modal: Rc::new(|_| false),
            render_plugin_overlays: Rc::new(|_, _| div().into_any_element()),
            open_plugin_logs: Rc::new(|_, _| {}),
            on_appearance_changed: Rc::new(|_, _, _| {}),
            tray_state: Rc::new(|_, _| TrayState::default()),
            set_minimize_to_tray: Rc::new(|_, _, _| Ok(())),
            set_always_on_top: Rc::new(|_, _, _| Ok(())),
            observe_tray: Rc::new(|_| None),
            global_shortcuts: Rc::new(|_, _| Vec::new()),
            set_global_hotkey: Rc::new(|_, _, _| false),
            reset_global_hotkeys: Rc::new(|_| {}),
            updater_snapshot: Rc::new(|_| UpdateSnapshot::default()),
            set_auto_update: Rc::new(|_, _| {}),
            check_for_updates: Rc::new(|_| {}),
            restart_to_update: Rc::new(|_| {}),
            observe_updater: Rc::new(|_| None),
        }
    }
}

enum ActiveConfirm {
    RestoreBackup(PathBuf),
    ResetShortcuts,
    SwitchTheme(Arc<Theme>),
    DeleteSubscription { id: Id, title: String },
}

struct SyncSettingsState {
    server: Entity<TextInput>,
    token: Entity<TextInput>,
    passphrase: Entity<TextInput>,
    save_task: Option<Task<()>>,
    dirty: bool,
}

struct SubscriptionSettingsState {
    form: Option<(Option<Id>, Option<String>)>,
    name: Entity<TextInput>,
    url: Entity<TextInput>,
    kind: CalendarKind,
    provider: CalendarProvider,
    username: Entity<TextInput>,
    password: Entity<TextInput>,
    writable: bool,
    discovering: bool,
    discovered: Vec<DiscoveredCalendar>,
    discover_error: Option<String>,
}

struct BackupSettingsState {
    message: Option<String>,
    auto_backups: Vec<BackupEntry>,
    auto_backups_task: Option<Task<()>>,
    schema_version: usize,
    diagnostic_message: Option<(String, bool)>,
    #[cfg(test)]
    backups_dir_override: Option<PathBuf>,
    #[cfg(test)]
    restore_target_override: Option<PathBuf>,
}

struct ShortcutSettingsState {
    recording: Option<shortcuts_settings_view::RecordingTarget>,
    rejected: bool,
}

struct ThemeSettingsState {
    save_error: Option<String>,
    sticky_error: Option<String>,
}

pub struct SettingsWindow {
    store: Rc<Store>,
    host: SettingsHost,
    preferences: Preferences,
    page: Page,
    page_scroll: gpui::ScrollHandle,
    menu: Option<Menu>,
    focus_handle: FocusHandle,
    closing: bool,
    drag: WindowDragState,
    confirm: Option<ActiveConfirm>,
    cached_tags: Vec<fern_core::Tag>,
    cached_lists: Vec<fern_core::List>,
    sound_enabled: bool,
    tray: TrayState,
    installed_notes: Option<Rc<ReleaseNotes>>,
    release_notes: Option<Rc<ReleaseNotes>>,
    sync: SyncSettingsState,
    subs: SubscriptionSettingsState,
    backups: BackupSettingsState,
    shortcuts: ShortcutSettingsState,
    themes: ThemeSettingsState,
    _updates: Option<Subscription>,
    _feeds: Option<Subscription>,
    _tray: Option<Subscription>,
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
    host: SettingsHost,
    notes: Option<Rc<ReleaseNotes>>,
    initial_page: Option<Page>,
    parent: &Window,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<SettingsWindow>> {
    let mut opts = CsdWindowOptions::new(size(px(WIDTH), px(HEIGHT)), cx);
    opts.title = Some(i18n::t(cx).settings.into());
    opts.min_size = Some(size(px(MIN_WIDTH), px(MIN_HEIGHT)));
    opts.traffic_light_position = cfg!(target_os = "macos").then_some(point(px(12.), px(14.)));
    opts.display_id = parent.display(cx).map(|display| display.id());
    let options = csd_window_options(opts);
    let handle = cx.open_window(options, |window, cx| {
        desktop_common::touch::init(window, cx);
        cx.new(|cx| {
            let mut view = SettingsWindow::new(store, host, cx);
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

/// Installs the theme and active preference snapshot from `preferences`.
pub fn apply(
    preferences: &Preferences,
    on_appearance_changed: &(impl Fn(Option<String>, Option<String>, &mut App) + ?Sized),
    window: &Window,
    cx: &mut App,
) {
    let refreshed = user_themes::apply_theme(&preferences.theme, window, cx);
    let appearance = if cx.theme().is_dark() { "dark" } else { "light" };
    on_appearance_changed(None, Some(appearance.to_owned()), cx);
    ActivePreferences::sync_from_preferences(
        preferences,
        ActivePreferences::get(cx).completion_sound,
        cx,
    );
    if !refreshed {
        cx.refresh_windows();
    }
}

impl SettingsWindow {
    pub fn new(store: Rc<Store>, host: SettingsHost, cx: &mut Context<Self>) -> Self {
        let preferences = store.preferences().unwrap_or_default();
        let config = store.sync_config().ok().flatten().unwrap_or_default();
        let cached_tags = store.tags().unwrap_or_default();
        let cached_lists = store.lists().unwrap_or_default();
        let schema_version = store.schema_version().unwrap_or(fern_core::SCHEMA_VERSION);
        let sound_enabled = desktop_common::sound::enabled(&store);
        let tray = (host.tray_state)(&store, cx);
        let field = |placeholder: &'static str, text: String, masked: bool, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, placeholder, cx);
                if masked {
                    input = input.masked();
                }
                input.set_text(text, cx);
                input
            });
            cx.subscribe(&input, |this: &mut Self, _, _: &Changed, cx| {
                this.schedule_save_sync(cx);
            })
            .detach();
            cx.subscribe(&input, |this: &mut Self, _, _: &Submitted, cx| {
                this.flush_sync_save(cx);
            })
            .detach();
            input
        };
        let sync_server = field("https://fern.example.org", config.server, false, cx);
        let sync_token = field("", config.token, true, cx);
        let sync_passphrase = field(i18n::t(cx).sync_passphrase_placeholder, config.passphrase, true, cx);
        let updates = (host.observe_updater)(cx);
        let feeds = subscriptions::feeds(cx).map(|feeds| cx.observe(&feeds, |_, _, cx| cx.notify()));
        let tray_sub = (host.observe_tray)(cx);
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            this.refresh_cached_collections(cx);
        })
        .detach();
        let plain = |placeholder: &'static str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| TextInput::new(Mode::SingleLine, placeholder, cx));
            cx.subscribe(&input, |this: &mut Self, _, _: &Submitted, cx| {
                this.save_subscription_form(cx);
            })
            .detach();
            input
        };
        let subscription_name = plain(i18n::t(cx).subscription_name_placeholder, cx);
        let subscription_url = plain("https://…/calendar.ics", cx);
        cx.subscribe(&subscription_url, |_, _, _: &Changed, cx| cx.notify()).detach();
        let subscription_username = plain("", cx);
        let subscription_password = cx.new(|cx| TextInput::new(Mode::SingleLine, "", cx).masked());
        cx.subscribe(&subscription_password, |this: &mut Self, _, _: &Submitted, cx| {
            this.save_subscription_form(cx);
        })
        .detach();
        let installed_notes = release_notes::installed(&store).map(Rc::new);
        let mut this = Self {
            store,
            host,
            preferences,
            page: Page::default(),
            page_scroll: gpui::ScrollHandle::new(),
            menu: None,
            focus_handle: cx.focus_handle(),
            closing: false,
            drag: WindowDragState::default(),
            confirm: None,
            cached_tags,
            cached_lists,
            sound_enabled,
            tray,
            installed_notes,
            release_notes: None,
            sync: SyncSettingsState {
                server: sync_server,
                token: sync_token,
                passphrase: sync_passphrase,
                save_task: None,
                dirty: false,
            },
            subs: SubscriptionSettingsState {
                form: None,
                name: subscription_name,
                url: subscription_url,
                kind: CalendarKind::Ics,
                provider: CalendarProvider::Url,
                username: subscription_username,
                password: subscription_password,
                writable: true,
                discovering: false,
                discovered: Vec::new(),
                discover_error: None,
            },
            backups: BackupSettingsState {
                message: None,
                auto_backups: Vec::new(),
                auto_backups_task: None,
                schema_version,
                diagnostic_message: None,
                #[cfg(test)]
                backups_dir_override: None,
                #[cfg(test)]
                restore_target_override: None,
            },
            shortcuts: ShortcutSettingsState {
                recording: None,
                rejected: false,
            },
            themes: ThemeSettingsState {
                save_error: None,
                sticky_error: None,
            },
            _updates: updates,
            _feeds: feeds,
            _tray: tray_sub,
        };
        this.refresh_auto_backups(cx);
        this
    }

    pub fn update_tray_state(&mut self, tray: TrayState, cx: &mut Context<Self>) {
        if self.tray != tray {
            self.tray = tray;
            cx.notify();
        }
    }

    pub fn host(&self) -> &SettingsHost {
        &self.host
    }

    pub fn open_plugin_logs(&mut self, plugin_id: &str, cx: &mut Context<Self>) {
        self.set_page(Page::Plugins, cx);
        (self.host.open_plugin_logs)(plugin_id, cx);
    }

    fn refresh_cached_collections(&mut self, cx: &mut Context<Self>) {
        self.cached_tags = self.store.tags().unwrap_or_default();
        self.cached_lists = self.store.lists().unwrap_or_default();
        cx.notify();
    }

    pub fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| {
                this.flush_sync_save(cx);
                (this.host.close_plugin_page)(cx);
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
            this.sync.passphrase.update(cx, |input, cx| {
                input.set_placeholder(strings.sync_passphrase_placeholder, cx);
            });
            this.subs.name.update(cx, |input, cx| {
                input.set_placeholder(strings.subscription_name_placeholder, cx);
            });
        })
        .detach();
    }

    pub fn is_closing(&self) -> bool {
        self.closing
    }

    pub fn close_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_sync_save(cx);
        (self.host.close_plugin_page)(cx);
        self.closing = true;
        window.remove_window();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_sync_save(cx);
        (self.host.close_plugin_page)(cx);
        if self.closing {
            return;
        }
        self.closing = true;
        cx.emit(SettingsEvent::Dismissed);
        window.defer(cx, |window, _| window.remove_window());
    }

    fn schedule_save_sync(&mut self, cx: &mut Context<Self>) {
        self.sync.dirty = true;
        self.sync.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SYNC_SAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| this.flush_sync_save(cx)).ok();
        }));
    }

    fn flush_sync_save(&mut self, cx: &mut Context<Self>) {
        self.sync.save_task = None;
        if !self.sync.dirty {
            return;
        }
        self.sync.dirty = false;
        let config = fern_core::sync::SyncConfig {
            server: self.sync.server.read(cx).text().to_owned(),
            token: self.sync.token.read(cx).text().to_owned(),
            passphrase: self.sync.passphrase.read(cx).text().to_owned(),
        };
        match self.store.set_sync_config(&config) {
            Ok(()) => cx.emit(SettingsEvent::Changed),
            Err(err) => desktop_common::logging::log_error(
                "settings",
                format!("failed to save the sync settings: {err:#}"),
            ),
        }
    }

    pub fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        let changed = self.page != page;
        if changed && self.page == Page::Sync {
            self.flush_sync_save(cx);
        }
        if changed && self.page == Page::Plugins {
            (self.host.close_plugin_page)(cx);
        }
        if changed {
            self.page_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.page = page;
        self.menu = None;
        self.release_notes = None;
        self.confirm = None;
        self.shortcuts.recording = None;
        self.shortcuts.rejected = false;
        self.themes.save_error = None;
        if page == Page::About {
            self.refresh_auto_backups(cx);
        }
        #[cfg(not(test))]
        if changed && page == Page::Shortcuts {
            self.reload_keymap(cx);
        }
        cx.notify();
    }

    pub fn current_page(&self) -> Page {
        self.page
    }

    pub fn show_release_notes(
        &mut self,
        notes: Rc<ReleaseNotes>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_page(Page::About, cx);
        self.release_notes = Some(notes);
        window.focus(&self.focus_handle, cx);
    }

    fn back_to_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_page(Page::About, cx);
        window.focus(&self.focus_handle, cx);
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if (self.host.dismiss_plugin_modal)(cx) {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else if self.confirm.take().is_some() || self.themes.save_error.take().is_some() {
            cx.notify();
        } else if self.shortcuts.recording.take().is_some() {
            self.shortcuts.rejected = false;
            cx.notify();
        } else if self.menu.is_some() {
            self.set_menu(None, cx);
        } else if self.release_notes.is_some() {
            self.back_to_about(window, cx);
        } else if self.subs.form.is_some() {
            self.subs.form = None;
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else if self.page == Page::Plugins && (self.host.handle_plugin_escape)(cx) {
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

    fn update(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Preferences),
    ) {
        self.menu = None;
        match self.store.update_preferences(change) {
            Ok(preferences) => {
                self.preferences = preferences;
                let on_appearance = self.host.on_appearance_changed.clone();
                apply(&self.preferences, on_appearance.as_ref(), window, cx);
                cx.emit(SettingsEvent::Changed);
            }
            Err(err) => {
                desktop_common::logging::log_error(
                    "settings",
                    format!("failed to save the settings: {err:#}"),
                );
                cx.notify();
            }
        }
    }

    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting("locale", locale.code()) {
            desktop_common::logging::log_error(
                "settings",
                format!("failed to save the language: {err:#}"),
            );
        }
        cx.set_global(CurrentLocale(locale));
        (self.host.on_appearance_changed)(Some(locale.code().to_owned()), None, cx);
        self.menu = None;
        cx.refresh_windows();
    }

    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, spec: &str, cx: &mut Context<Self>) {
        let (page, menu) = spec.split_once('>').unwrap_or((spec, ""));
        if let Some(path) = menu.strip_prefix("import:") {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let strings = i18n::t(cx);
            self.backups.message = Some(backups::format_ticktick_import(
                self.store.import_ticktick(&text),
                strings,
            ));
            desktop_common::data_version::data_changed(cx.entity_id(), cx);
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

    fn drag_area(&self, id: &'static str, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        apply_drag_strip(div().id(id), |this: &mut Self| &mut this.drag, window, cx)
    }

    fn render_close_button(&self, cx: &Context<Self>) -> impl IntoElement {
        IconButton::new("close-settings", IconName::X)
            .icon_color(Color::Muted)
            .tooltip(Tooltip::text(i18n::t(cx).close))
            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
    }

    fn render_nav(
        &self,
        chrome: SettingsChrome,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let strings = i18n::t(cx);
        let drag_strip = self
            .drag_area("settings-nav-drag", window, cx)
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(if cfg!(target_os = "macos") {
                56.
            } else if chrome.linux_ssd {
                40.
            } else {
                44.
            }))
            .when(cfg!(target_os = "macos"), |el| el.pt(px(20.)))
            .px_4()
            .when(chrome.close_on_left, |el| el.child(self.render_close_button(cx)))
            .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(strings.settings));
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
            .child(drag_strip)
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

    fn open_subscription_form(
        &mut self,
        id: Option<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let current = id.and_then(|id| {
            let feeds = subscriptions::feeds(cx)?;
            let feeds = feeds.read(cx);
            feeds
                .calendars()
                .iter()
                .find(|c| c.subscription.id == id)
                .map(|c| c.subscription.clone())
        });
        let (name, url, color, kind, provider, username, password, writable) = match current {
            Some(s) => (
                s.name,
                s.url,
                s.color,
                s.kind,
                s.provider,
                s.username,
                s.password,
                s.writable,
            ),
            None => (
                String::new(),
                String::new(),
                None,
                CalendarKind::Ics,
                CalendarProvider::Url,
                String::new(),
                String::new(),
                true,
            ),
        };
        self.subs.kind = kind;
        self.subs.provider = provider;
        self.subs.writable = writable;
        self.subs.discovering = false;
        self.subs.discovered.clear();
        self.subs.discover_error = None;
        self.subs.name.update(cx, |input, cx| input.set_text(name, cx));
        self.subs.url.update(cx, |input, cx| input.set_text(url, cx));
        self.subs.username.update(cx, |input, cx| input.set_text(username, cx));
        self.subs.password.update(cx, |input, cx| input.set_text(password, cx));
        self.subs.form = Some((id, color));
        window.focus(&self.subs.url.focus_handle(cx), cx);
        cx.notify();
    }

    fn save_subscription_form(&mut self, cx: &mut Context<Self>) {
        let url = self.subs.url.read(cx).text().to_owned();
        if url.trim().is_empty() {
            return;
        }
        let Some((id, color)) = self.subs.form.take() else {
            return;
        };
        let name = self.subs.name.read(cx).text().to_owned();
        let kind = self.subs.kind;
        let provider = self.subs.provider;
        let username = self.subs.username.read(cx).text().to_owned();
        let password = self.subs.password.read(cx).text().to_owned();
        let writable = self.subs.writable;
        if let Some(feeds) = subscriptions::feeds(cx) {
            feeds.update(cx, |feeds, cx| match (id, kind) {
                (Some(id), CalendarKind::CalDav) => {
                    let hidden = feeds
                        .calendars()
                        .iter()
                        .find(|c| c.subscription.id == id)
                        .is_some_and(|c| c.subscription.hidden);
                    feeds.update_caldav(
                        id,
                        &name,
                        &url,
                        color.as_deref(),
                        hidden,
                        provider,
                        &username,
                        &password,
                        writable,
                        cx,
                    );
                }
                (None, CalendarKind::CalDav) => {
                    feeds.add_caldav(
                        &name,
                        &url,
                        color.as_deref(),
                        provider,
                        &username,
                        &password,
                        writable,
                        cx,
                    );
                }
                (Some(id), CalendarKind::Ics) => {
                    let hidden = feeds
                        .calendars()
                        .iter()
                        .find(|c| c.subscription.id == id)
                        .is_some_and(|c| c.subscription.hidden);
                    feeds.update_subscription(id, &name, &url, color.as_deref(), hidden, cx);
                }
                (None, CalendarKind::Ics) => feeds.add(&name, &url, color.as_deref(), cx),
            });
        }
        cx.notify();
    }

    fn discover_caldav(&mut self, cx: &mut Context<Self>) {
        if self.subs.discovering {
            return;
        }
        let url = self.subs.url.read(cx).text().trim().to_owned();
        let username = self.subs.username.read(cx).text().to_owned();
        let password = self.subs.password.read(cx).text().to_owned();
        if url.is_empty() {
            return;
        }
        self.subs.discovering = true;
        self.subs.discover_error = None;
        self.subs.discovered.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { subscriptions::discover_caldav_calendars(&url, &username, &password) })
                .await;
            this.update(cx, |this, cx| {
                this.subs.discovering = false;
                match result {
                    Ok(cals) => {
                        this.subs.discovered = cals;
                    }
                    Err(err) => {
                        this.subs.discover_error = Some(format!("{err:#}"));
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
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let strings = i18n::t(cx);
        let notes_open = self.release_notes.is_some();
        let about = strings.settings_pages[5];
        let page = match &self.release_notes {
            Some(notes) => self.render_release_notes(notes, cx),
            None => self.render_page(cx),
        };
        let chrome = SettingsChrome::for_window(window, cx);
        let overlays = (self.host.render_plugin_overlays)(window, cx);
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
            .child(self.render_nav(chrome, window, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(colors.background)
                    .child(
                        h_flex()
                            .debug_selector(|| "settings-header".to_owned())
                            .h(px(if chrome.linux_ssd && !notes_open { 12. } else { 44. }))
                            .flex_none()
                            .px_3()
                            .items_center()
                            .when(notes_open, |el| {
                                el.child(
                                    ui::Button::new("release-notes-back", about)
                                        .variant(ui::ButtonVariant::Ghost)
                                        .size(ui::ButtonSize::Sm)
                                        .icon(IconName::ChevronLeft)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.back_to_about(window, cx)
                                        })),
                                )
                            })
                            .child(self.drag_area("settings-header-drag", window, cx).flex_1().min_w_0().h_full())
                            .when(chrome.show_right_close(), |el| el.child(self.render_close_button(cx))),
                    )
                    .child(page),
            )
            .children(self.render_active_confirm(cx))
            .children(self.render_theme_save_error(cx))
            .child(overlays);
        desktop_common::layout::linux_window_frame(
            root,
            desktop_common::layout::LINUX_WINDOW_ROUNDING,
            window,
            cx,
        )
    }
}

#[derive(Clone, Copy)]
struct SettingsChrome {
    linux_ssd: bool,
    close_on_left: bool,
}

impl SettingsChrome {
    fn for_window(window: &Window, cx: &App) -> Self {
        let linux_ssd = cfg!(any(target_os = "linux", target_os = "freebsd"))
            && matches!(window.window_decorations(), gpui::Decorations::Server);
        let close_on_left = !linux_ssd && has_left_close_button(cx);
        Self {
            linux_ssd,
            close_on_left,
        }
    }

    fn show_right_close(self) -> bool {
        !cfg!(target_os = "macos") && !self.linux_ssd && !self.close_on_left
    }
}

fn has_left_close_button(cx: &App) -> bool {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        cx.button_layout()
            .is_some_and(|layout| layout.left.contains(&Some(gpui::WindowButton::Close)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let _ = cx;
        false
    }
}

#[cfg(test)]
mod tests;
