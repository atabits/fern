//! Cross-platform system tray / menu bar icon, context menu, always-on-top, and minimize-to-tray.
//!
//! - **Windows**: Win32 `Shell_NotifyIconW` subclass (`tray/windows.rs`).
//! - **macOS**: AppKit `NSStatusBar` / `NSStatusItem` menu bar item (`tray/macos.rs`).
//! - **Linux**: freedesktop / KDE `StatusNotifierItem` over D-Bus via `ksni` (`tray/linux.rs`).

use std::rc::Rc;

use fern_core::Store;
use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{App, Global, Window, WindowHandle};

use crate::app_view::AppView;
use crate::i18n::{self, CurrentLocale, Strings};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod platform {
    use futures::channel::mpsc::UnboundedReceiver;
    use gpui::Window;

    use super::{TrayAction, TrayLabels, TrayPrefs};

    pub(super) fn set_tooltip(_tip: &str) {}
    pub(super) fn set_enabled(_is_enabled: bool) {}
    pub(super) fn set_always_on_top(_topmost: bool) {}
    pub(super) fn hide_to_tray() -> bool {
        false
    }
    pub(super) fn hide_to_tray_if_enabled() -> bool {
        false
    }
    pub(super) fn prepare_quit() {}
    pub(super) fn restore_main_window() -> bool {
        false
    }
    pub(super) fn update_labels(_labels: TrayLabels) {}
    pub(super) fn prepare_channel() -> UnboundedReceiver<TrayAction> {
        futures::channel::mpsc::unbounded().1
    }
    pub(super) fn init(_window: &Window, _prefs: TrayPrefs, _labels: TrayLabels) {}
}

const SETTING: &str = "minimize_to_tray";
const TOPMOST_SETTING: &str = "always_on_top";

/// Cached tray preferences stored as a GPUI Global so `render()` never queries SQLite
/// and tray/hotkey toggles keep Settings and Account Menu in sync.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrayPrefs {
    pub minimize_to_tray: bool,
    pub always_on_top: bool,
}

impl Global for TrayPrefs {}

impl TrayPrefs {
    pub fn load(store: &Store) -> Self {
        Self {
            minimize_to_tray: enabled(store),
            always_on_top: always_on_top(store),
        }
    }

    pub fn current(store: &Store, cx: &mut App) -> Self {
        if let Some(prefs) = cx.try_global::<Self>() {
            *prefs
        } else {
            let prefs = Self::load(store);
            cx.set_global(prefs);
            prefs
        }
    }
}

#[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TrayLabels {
    pub focus: &'static str,
    pub start_pomo: &'static str,
    pub stopwatch: &'static str,
    pub always_on_top: &'static str,
    pub quit: &'static str,
}

impl TrayLabels {
    pub(super) fn from_strings(strings: &'static Strings) -> Self {
        Self {
            focus: strings.tray_focus,
            start_pomo: strings.tray_start_pomo,
            stopwatch: strings.tray_stopwatch,
            always_on_top: strings.tray_always_on_top,
            quit: strings.tray_quit,
        }
    }
}

#[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
pub enum TrayAction {
    StartPomo,
    StartStopwatch,
    ToggleAlwaysOnTop,
    Activate,
    Quit,
}

/// Updates the tray icon hover tooltip (e.g. to show a running Focus timer).
pub fn set_tooltip(tip: &str) {
    platform::set_tooltip(tip);
}

pub fn enabled(store: &Store) -> bool {
    store.setting(SETTING).ok().flatten().as_deref() == Some("1")
}

pub fn set_enabled(store: &Store, is_enabled: bool, cx: &mut App) -> anyhow::Result<()> {
    store.set_setting(SETTING, if is_enabled { "1" } else { "0" })?;
    platform::set_enabled(is_enabled);
    let mut prefs = TrayPrefs::current(store, cx);
    prefs.minimize_to_tray = is_enabled;
    cx.set_global(prefs);
    cx.refresh_windows();
    Ok(())
}

pub fn always_on_top(store: &Store) -> bool {
    store.setting(TOPMOST_SETTING).ok().flatten().as_deref() == Some("1")
}

pub fn set_always_on_top(store: &Store, topmost: bool, cx: &mut App) -> anyhow::Result<()> {
    store.set_setting(TOPMOST_SETTING, if topmost { "1" } else { "0" })?;
    platform::set_always_on_top(topmost);
    let mut prefs = TrayPrefs::current(store, cx);
    prefs.always_on_top = topmost;
    cx.set_global(prefs);
    cx.refresh_windows();
    Ok(())
}

/// Hides the main window into the system tray instead of closing it.
/// Returns `false` when the tray is unavailable (e.g. in tests or unsupported Wayland WMs)
/// or when quitting.
pub fn hide_to_tray() -> bool {
    platform::hide_to_tray()
}

/// Hides the main window into the system tray only if "Minimize to tray" is enabled.
/// Returns `false` when disabled, unavailable, or quitting so callers can minimize normally.
pub fn hide_to_tray_if_enabled() -> bool {
    platform::hide_to_tray_if_enabled()
}

/// Marks the app as quitting and removes the tray icon so the window can close cleanly.
pub fn prepare_quit() {
    platform::prepare_quit();
}

/// Restores the main window from the system tray (or minimized state) and brings it to front.
pub fn restore_main_window() -> bool {
    platform::restore_main_window()
}

pub fn update_labels(strings: &'static Strings) {
    platform::update_labels(TrayLabels::from_strings(strings));
}

/// Creates and registers the tray action channel BEFORE `init()` adds the tray icon,
/// so early tray clicks during startup are queued rather than dropped.
pub fn prepare_channel() -> UnboundedReceiver<TrayAction> {
    platform::prepare_channel()
}

pub fn init(window: &Window, store: &Store, cx: &mut App) {
    let labels = TrayLabels::from_strings(i18n::t(cx));
    let prefs = TrayPrefs::load(store);
    cx.set_global(prefs);
    platform::init(window, prefs, labels);
}

pub fn connect(
    mut rx: UnboundedReceiver<TrayAction>,
    main_window: WindowHandle<AppView>,
    store: Rc<Store>,
    cx: &mut App,
) {
    cx.observe_global::<CurrentLocale>(|cx| update_labels(i18n::t(cx))).detach();
    cx.spawn(async move |cx| {
        while let Some(action) = rx.next().await {
            match action {
                TrayAction::StartPomo => {
                    let _ = main_window.update(cx, |view, _, cx| view.start_focus_from_tray(false, cx));
                }
                TrayAction::StartStopwatch => {
                    let _ = main_window.update(cx, |view, _, cx| view.start_focus_from_tray(true, cx));
                }
                TrayAction::ToggleAlwaysOnTop => {
                    cx.update(|cx| {
                        let next = !TrayPrefs::current(&store, cx).always_on_top;
                        if let Err(err) = set_always_on_top(&store, next, cx) {
                            eprintln!("failed to save always_on_top setting: {err:#}");
                        }
                    });
                }
                TrayAction::Activate => {
                    #[cfg(windows)]
                    let link = crate::data_dir()
                        .ok()
                        .and_then(|dir| crate::single_instance::take_pending_link(&dir));
                    #[cfg(not(windows))]
                    let link: Option<fern_core::Id> = None;
                    let _ = main_window.update(cx, |view, window, cx| {
                        restore_main_window();
                        window.activate_window();
                        if let Some(task_id) = link {
                            view.open_task(task_id, window, cx);
                        }
                    });
                }
                TrayAction::Quit => {
                    if cx.update(|cx| AppView::try_quit(main_window, cx)) {
                        break;
                    }
                }
            }
        }
    })
    .detach();
}
