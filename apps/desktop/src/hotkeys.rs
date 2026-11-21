//! System-wide shortcuts, as the original's desktop app has them: they work while another app
//! is in front. Windows only, where `RegisterHotKey` makes them; GPUI has no such shortcuts.
//!
//! - Quick Add (Ctrl+Shift+A at first): a small field over every window (see `quick_add_window`).
//! - Show / Hide (Ctrl+Shift+E at first): shows the main window, or hides it (to tray when enabled,
//!   otherwise minimized) when it is in front.
//! - Focus Mini & Pin Window: off by default, configurable in the Shortcuts panel.
//!
//! Each can be changed or turned off from the Shortcuts panel, as in the original's Settings;
//! a shortcut another app holds is shown as taken.

use std::rc::Rc;
#[cfg(windows)]
use std::sync::{Arc, Condvar, Mutex};

use fern_core::Store;
use gpui::{App, WindowHandle};
#[cfg(windows)]
use gpui::{Global, Keystroke};

use crate::app_view::AppView;

/// System-wide shortcuts, in the order the Shortcuts panel lists them.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hotkey {
    QuickAdd = 0,
    ShowHide = 1,
    FocusMini = 2,
    PinWindow = 3,
}

#[cfg(any(windows, test))]
pub const COUNT: usize = 4;

#[cfg(any(windows, test))]
impl Hotkey {
    pub const ALL: [Self; COUNT] = [
        Self::QuickAdd,
        Self::ShowHide,
        Self::FocusMini,
        Self::PinWindow,
    ];

    #[cfg(windows)]
    pub const fn index(self) -> usize {
        self as usize
    }

    const fn win32_id(self) -> i32 {
        (self as i32) + 1
    }

    fn from_win32_id(id: usize) -> Option<Self> {
        Self::ALL.get(id.checked_sub(1)?).copied()
    }

    #[cfg(windows)]
    const fn setting_key(self) -> &'static str {
        match self {
            Self::QuickAdd => "hotkey_quick_add",
            Self::ShowHide => "hotkey_show_hide",
            Self::FocusMini => "hotkey_focus_mini",
            Self::PinWindow => "hotkey_pin_window",
        }
    }

    #[cfg(windows)]
    const fn default_setting(self) -> &'static str {
        match self {
            Self::QuickAdd => "ctrl-shift-a",
            Self::ShowHide => "ctrl-shift-e",
            Self::FocusMini => "off",
            Self::PinWindow => "off",
        }
    }

    #[cfg(windows)]
    pub fn label(self, strings: &'static crate::i18n::Strings) -> &'static str {
        match self {
            Self::QuickAdd => strings.global_quick_add,
            Self::ShowHide => strings.global_show_hide,
            Self::FocusMini => strings.global_focus_mini,
            Self::PinWindow => strings.global_pin_window,
        }
    }
}

/// A key with its modifiers, kept as GPUI writes keystrokes ("ctrl-shift-a").
#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: String,
}

#[cfg(any(windows, test))]
impl Binding {
    fn parse(text: &str) -> Option<Self> {
        let mut binding = Binding { ctrl: false, alt: false, shift: false, win: false, key: String::new() };
        for part in text.split('-') {
            match part {
                "ctrl" => binding.ctrl = true,
                "alt" => binding.alt = true,
                "shift" => binding.shift = true,
                "win" | "cmd" | "super" => binding.win = true,
                key => binding.key = key.to_owned(),
            }
        }
        binding.virtual_key()?;
        Some(binding)
    }

    /// A pressed keystroke, when it can be a system-wide shortcut: a letter, a digit, a function
    /// key or Space, with Ctrl, Alt or Win (Shift alone would take typing away).
    #[cfg(windows)]
    pub fn from_keystroke(keystroke: &Keystroke) -> Option<Self> {
        let m = &keystroke.modifiers;
        let binding = Binding { ctrl: m.control, alt: m.alt, shift: m.shift, win: m.platform, key: keystroke.key.to_lowercase() };
        (binding.ctrl || binding.alt || binding.win).then_some(())?;
        binding.virtual_key()?;
        Some(binding)
    }

    fn to_setting(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for (on, name) in [(self.ctrl, "ctrl"), (self.alt, "alt"), (self.shift, "shift"), (self.win, "win")] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("-")
    }

    /// "Ctrl+Shift+A", as the panel shows it.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (on, name) in [(self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift"), (self.win, "Win")] {
            if on {
                parts.push(name.to_owned());
            }
        }
        let mut key = self.key.clone();
        if let Some(first) = key.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        parts.push(key);
        parts.join("+")
    }

    /// Windows' virtual-key code of the key.
    fn virtual_key(&self) -> Option<u32> {
        let key = self.key.as_str();
        match key.len() {
            1 => {
                let c = key.chars().next()?.to_ascii_uppercase();
                c.is_ascii_alphanumeric().then_some(c as u32)
            }
            _ if key == "space" => Some(0x20),
            _ => key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)).map(|n| 0x6F + n),
        }
    }
}

/// What the shortcuts' thread registers, and what it could not.
#[cfg(windows)]
#[derive(Default)]
struct Shared {
    bindings: [Option<Binding>; COUNT],
    taken: [bool; COUNT],
    /// The thread to wake when the bindings change or when shutting down.
    thread: Option<u32>,
    /// Set once the listener thread has unregistered all hotkeys and exited.
    stopped: bool,
}

#[cfg(windows)]
struct GlobalHotkeys {
    store: Rc<Store>,
    shared: Arc<Mutex<Shared>>,
    stopped_cv: Arc<Condvar>,
}

#[cfg(windows)]
impl Global for GlobalHotkeys {}

/// Each shortcut as set (`None` when off) and whether another app holds it.
#[cfg(windows)]
pub fn bindings(cx: &App) -> [(Hotkey, Option<Binding>, bool); COUNT] {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else {
        return std::array::from_fn(|i| (Hotkey::ALL[i], None, false));
    };
    let shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
    std::array::from_fn(|i| (Hotkey::ALL[i], shared.bindings[i].clone(), shared.taken[i]))
}

#[cfg(windows)]
fn wake_listener(thread: Option<u32>) {
    if let Some(thread) = thread {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};
        unsafe { PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0)) }.ok();
    }
}

/// Changes a shortcut (`None` turns it off), keeps it in the store and registers it anew.
#[cfg(windows)]
pub fn set(hotkey: Hotkey, binding: Option<Binding>, cx: &mut App) {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return };
    let value = binding.as_ref().map_or_else(|| "off".to_owned(), Binding::to_setting);
    if let Err(err) = global.store.set_setting(hotkey.setting_key(), &value) {
        eprintln!("failed to save the shortcut: {err:#}");
    }
    let thread = {
        let mut shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
        let idx = hotkey.index();
        shared.bindings[idx] = binding;
        shared.taken[idx] = false;
        shared.thread
    };
    wake_listener(thread);
    cx.refresh_windows();
}

/// Restores all system-wide shortcuts to their built-in defaults in a single batched update.
#[cfg(windows)]
pub fn reset_defaults(cx: &mut App) {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return };
    let defaults: [Option<Binding>; COUNT] = std::array::from_fn(|i| {
        let hotkey = Hotkey::ALL[i];
        let default = hotkey.default_setting();
        if let Err(err) = global.store.set_setting(hotkey.setting_key(), default) {
            eprintln!("failed to save the shortcut: {err:#}");
        }
        (default != "off").then(|| Binding::parse(default)).flatten()
    });
    let thread = {
        let mut shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.bindings = defaults;
        shared.taken = [false; COUNT];
        shared.thread
    };
    wake_listener(thread);
    cx.refresh_windows();
}

#[cfg(not(windows))]
pub fn reset_defaults(_cx: &mut App) {}

/// Unregisters all global hotkeys synchronously (up to a short timeout) before spawning a
/// replacement instance on update so the new process does not hit `taken = true`.
#[cfg(windows)]
pub fn shutdown(cx: &App) {
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};

    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return };
    let mut shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(thread) = shared.thread else { return };
    if shared.stopped {
        return;
    }
    let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    let deadline = Instant::now() + Duration::from_millis(500);
    while !shared.stopped {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { break };
        let (guard, timeout) = global
            .stopped_cv
            .wait_timeout(shared, remaining)
            .unwrap_or_else(|e| e.into_inner());
        shared = guard;
        if timeout.timed_out() {
            break;
        }
    }
}

#[cfg(not(windows))]
pub fn shutdown(_cx: &App) {}

/// Registers the shortcuts for the main window.
#[cfg(windows)]
pub fn init(window: WindowHandle<AppView>, store: Rc<Store>, cx: &mut App) {
    let bindings = std::array::from_fn(|i| {
        let hotkey = Hotkey::ALL[i];
        match store.setting(hotkey.setting_key()).ok().flatten() {
            Some(value) if value == "off" => None,
            Some(value) => Binding::parse(&value),
            None => Binding::parse(hotkey.default_setting()),
        }
    });
    let shared = Arc::new(Mutex::new(Shared { bindings, ..Default::default() }));
    let stopped_cv = Arc::new(Condvar::new());
    cx.set_global(GlobalHotkeys {
        store,
        shared: shared.clone(),
        stopped_cv: stopped_cv.clone(),
    });
    // Debug builds: `FERN_DEV_STATE=quick-add-window` opens Quick Add, as its shortcut would.
    #[cfg(debug_assertions)]
    if std::env::var("FERN_DEV_STATE").is_ok_and(|state| state.contains("quick-add-window")) {
        cx.defer(move |cx| crate::quick_add_window::open(window, cx));
    }
    start(window, shared, stopped_cv, cx);
}

#[cfg(not(windows))]
pub fn init(window: WindowHandle<AppView>, _store: Rc<Store>, cx: &mut App) {
    #[cfg(debug_assertions)]
    if std::env::var("FERN_DEV_STATE").is_ok_and(|state| state.contains("quick-add-window")) {
        cx.defer(move |cx| crate::quick_add_window::open(window, cx));
    }
    #[cfg(not(debug_assertions))]
    let _ = (window, cx);
}

#[cfg(windows)]
fn start(
    window: WindowHandle<AppView>,
    shared: Arc<Mutex<Shared>>,
    stopped_cv: Arc<Condvar>,
    cx: &mut App,
) {
    use futures::StreamExt as _;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || listen(shared, stopped_cv, tx))
        .ok();
    cx.spawn(async move |cx| {
        while let Some(event) = rx.next().await {
            let handled = match event {
                Event::Pressed(Hotkey::QuickAdd) => {
                    cx.update(|cx| crate::quick_add_window::open(window, cx));
                    Ok(())
                }
                Event::Pressed(Hotkey::ShowHide) => window
                    .update(cx, |_, window, _| {
                        if window.is_window_active() {
                            if !crate::tray::hide_to_tray_if_enabled() {
                                window.minimize_window();
                            }
                        } else {
                            crate::tray::restore_main_window();
                            window.activate_window();
                        }
                    })
                    .map_err(|_| ()),
                Event::Pressed(Hotkey::FocusMini) => window
                    .update(cx, |view, _, cx| view.toggle_focus_mini(cx))
                    .map_err(|_| ()),
                Event::Pressed(Hotkey::PinWindow) => {
                    cx.update(|cx| {
                        if let Some(store) = cx.try_global::<GlobalHotkeys>().map(|g| g.store.clone()) {
                            let next = !crate::tray::TrayPrefs::current(&store, cx).always_on_top;
                            if let Err(err) = crate::tray::set_always_on_top(&store, next, cx) {
                                eprintln!("failed to toggle always_on_top: {err:#}");
                            }
                        }
                    });
                    Ok(())
                }
                // A shortcut found taken: the panel shows it.
                Event::Registered => {
                    cx.update(|cx| cx.refresh_windows());
                    Ok(())
                }
            };
            if handled.is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(windows)]
enum Event {
    Pressed(Hotkey),
    Registered,
}

/// The shortcuts' thread: registers them, passes each press on, and registers them anew when
/// they change, until `WM_QUIT` or the app ends.
#[cfg(windows)]
fn listen(
    shared: Arc<Mutex<Shared>>,
    stopped_cv: Arc<Condvar>,
    tx: futures::channel::mpsc::UnboundedSender<Event>,
) {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
        UnregisterHotKey,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, WM_APP, WM_HOTKEY, WM_USER,
    };

    let unregister_all = || {
        for hotkey in Hotkey::ALL {
            unsafe { UnregisterHotKey(None, hotkey.win32_id()) }.ok();
        }
    };
    let register = |shared: &Mutex<Shared>| {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        for hotkey in Hotkey::ALL {
            let idx = hotkey.index();
            unsafe { UnregisterHotKey(None, hotkey.win32_id()) }.ok();
            let Some(binding) = shared.bindings[idx].clone() else {
                shared.taken[idx] = false;
                continue;
            };
            let Some(key) = binding.virtual_key() else {
                shared.taken[idx] = false;
                continue;
            };
            let mut modifiers = MOD_NOREPEAT;
            for (on, flag) in [
                (binding.ctrl, MOD_CONTROL),
                (binding.alt, MOD_ALT),
                (binding.shift, MOD_SHIFT),
                (binding.win, MOD_WIN),
            ] {
                if on {
                    modifiers |= flag;
                }
            }
            // Thread-wide (no window): WM_HOTKEY comes to this thread's queue.
            shared.taken[idx] =
                unsafe { RegisterHotKey(None, hotkey.win32_id(), HOT_KEY_MODIFIERS(modifiers.0), key) }
                    .is_err();
        }
    };

    // The queue exists once the thread has asked for a message; only then can it be woken.
    let mut msg = MSG::default();
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
    shared.lock().unwrap_or_else(|e| e.into_inner()).thread = Some(unsafe { GetCurrentThreadId() });
    register(&shared);
    if tx.unbounded_send(Event::Registered).is_ok() {
        // `GetMessageW` returns > 0 for messages, 0 for WM_QUIT, and -1 on error.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            let event = match msg.message {
                WM_HOTKEY => {
                    let Some(hotkey) = Hotkey::from_win32_id(msg.wParam.0) else { continue };
                    Event::Pressed(hotkey)
                }
                WM_APP => {
                    register(&shared);
                    Event::Registered
                }
                _ => continue,
            };
            if tx.unbounded_send(event).is_err() {
                break;
            }
        }
    }
    unregister_all();
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    guard.stopped = true;
    guard.thread = None;
    stopped_cv.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_win32_ids_roundtrip_and_reject_out_of_bounds() {
        assert_eq!(Hotkey::from_win32_id(0), None);
        for hotkey in Hotkey::ALL {
            assert_eq!(Hotkey::from_win32_id(hotkey.win32_id() as usize), Some(hotkey));
        }
        assert_eq!(Hotkey::from_win32_id(COUNT + 1), None);
    }

    #[test]
    fn binding_parses_and_formats_labels() {
        let binding = Binding::parse("ctrl-shift-a").unwrap();
        assert_eq!(binding.label(), "Ctrl+Shift+A");
        assert_eq!(binding.to_setting(), "ctrl-shift-a");
        assert!(Binding::parse("off").is_none());
    }
}
