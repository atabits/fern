//! System-wide shortcuts that work while another application is in front.
//!
//! - **Windows**: Win32 `RegisterHotKey` message thread (`hotkeys/windows.rs`).
//! - **macOS**: Carbon HIToolbox `RegisterEventHotKey` (`hotkeys/macos.rs`, no Accessibility permission required).
//! - **Linux**: X11 root-window passive grabs via `x11rb` (`hotkeys/linux.rs`).
//!
//! Each shortcut can be changed or turned off from Settings → Shortcuts; a shortcut held by
//! another application is reported as taken.

use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};

use fern_core::Store;
use futures::StreamExt as _;
use gpui::{App, Global, Keystroke, WindowHandle};

use crate::app_view::AppView;

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
    use std::sync::{Arc, Condvar, Mutex};

    use futures::channel::mpsc::UnboundedSender;

    use super::{Event, Shared};

    pub(super) fn start(
        shared: Arc<Mutex<Shared>>,
        stopped_cv: Arc<Condvar>,
        _tx: UnboundedSender<Event>,
    ) {
        shared.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        stopped_cv.notify_all();
    }

    pub(super) fn reload(_shared: &Arc<Mutex<Shared>>) {}

    pub(super) fn shutdown(_shared: &Arc<Mutex<Shared>>, _stopped_cv: &Arc<Condvar>) {}
}

/// System-wide shortcuts, in the order Settings → Shortcuts lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hotkey {
    QuickAdd = 0,
    ShowHide = 1,
    FocusMini = 2,
    PinWindow = 3,
}

pub const COUNT: usize = 4;

impl Hotkey {
    pub const ALL: [Self; COUNT] = [
        Self::QuickAdd,
        Self::ShowHide,
        Self::FocusMini,
        Self::PinWindow,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    #[cfg(any(windows, target_os = "macos", test))]
    pub(super) const fn id(self) -> u32 {
        (self as u32) + 1
    }

    #[cfg(any(windows, target_os = "macos", test))]
    pub(super) fn from_id(id: usize) -> Option<Self> {
        Self::ALL.get(id.checked_sub(1)?).copied()
    }

    const fn setting_key(self) -> &'static str {
        match self {
            Self::QuickAdd => "hotkey_quick_add",
            Self::ShowHide => "hotkey_show_hide",
            Self::FocusMini => "hotkey_focus_mini",
            Self::PinWindow => "hotkey_pin_window",
        }
    }

    const fn default_setting(self) -> &'static str {
        match self {
            Self::QuickAdd => "ctrl-shift-a",
            Self::ShowHide => "ctrl-shift-e",
            Self::FocusMini => "off",
            Self::PinWindow => "off",
        }
    }

    pub fn label(self, strings: &'static crate::i18n::Strings) -> &'static str {
        match self {
            Self::QuickAdd => strings.global_quick_add,
            Self::ShowHide => strings.global_show_hide,
            Self::FocusMini => strings.global_focus_mini,
            Self::PinWindow => strings.global_pin_window,
        }
    }
}

/// A key with its modifiers, stored as GPUI writes keystrokes ("ctrl-shift-a" / "cmd-shift-a").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub(super) ctrl: bool,
    pub(super) alt: bool,
    pub(super) shift: bool,
    pub(super) win: bool,
    pub(super) key: String,
}

impl Binding {
    fn parse(text: &str) -> Option<Self> {
        let mut binding = Binding { ctrl: false, alt: false, shift: false, win: false, key: String::new() };
        for part in text.split('-') {
            match part {
                "ctrl" => binding.ctrl = true,
                "alt" => binding.alt = true,
                "shift" => binding.shift = true,
                "win" | "cmd" | "super" | "meta" => binding.win = true,
                "" => return None,
                key => {
                    if !binding.key.is_empty() {
                        return None;
                    }
                    binding.key = key.to_owned();
                }
            }
        }
        (binding.ctrl || binding.alt || binding.win).then_some(())?;
        binding.virtual_key()?;
        Some(binding)
    }

    /// A pressed keystroke, when it can be a system-wide shortcut: a letter, a digit, a function
    /// key or Space, with Ctrl, Alt or Cmd/Win/Super (Shift alone would interfere with typing).
    pub fn from_keystroke(keystroke: &Keystroke) -> Option<Self> {
        let m = &keystroke.modifiers;
        let binding = Binding {
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            win: m.platform,
            key: keystroke.key.to_lowercase(),
        };
        (binding.ctrl || binding.alt || binding.win).then_some(())?;
        binding.virtual_key()?;
        Some(binding)
    }

    fn to_setting(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let platform_key = if cfg!(target_os = "macos") { "cmd" } else { "win" };
        for (on, name) in [
            (self.ctrl, "ctrl"),
            (self.alt, "alt"),
            (self.win, platform_key),
            (self.shift, "shift"),
        ] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("-")
    }

    /// Formats the binding for display in the Shortcuts page (e.g. "Ctrl+Shift+A" or "Cmd+Shift+A").
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let platform_label = if cfg!(target_os = "macos") {
            "Cmd"
        } else if cfg!(target_os = "linux") {
            "Super"
        } else {
            "Win"
        };
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.win, platform_label),
            (self.shift, "Shift"),
        ] {
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

    /// Windows virtual-key code (also used to validate supported keys cross-platform).
    pub(super) fn virtual_key(&self) -> Option<u32> {
        let key = self.key.as_str();
        match key.len() {
            1 => {
                let c = key.chars().next()?.to_ascii_uppercase();
                c.is_ascii_alphanumeric().then_some(c as u32)
            }
            _ if key == "space" => Some(0x20),
            _ => key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=12).contains(n))
                .map(|n| 0x6F + n),
        }
    }

    /// macOS Carbon HIToolbox ANSI virtual keycode (`kVK_*`).
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub(super) fn mac_key_code(&self) -> Option<u32> {
        match self.key.as_str() {
            "a" => Some(0x00),
            "s" => Some(0x01),
            "d" => Some(0x02),
            "f" => Some(0x03),
            "h" => Some(0x04),
            "g" => Some(0x05),
            "z" => Some(0x06),
            "x" => Some(0x07),
            "c" => Some(0x08),
            "v" => Some(0x09),
            "b" => Some(0x0B),
            "q" => Some(0x0C),
            "w" => Some(0x0D),
            "e" => Some(0x0E),
            "r" => Some(0x0F),
            "y" => Some(0x10),
            "t" => Some(0x11),
            "1" => Some(0x12),
            "2" => Some(0x13),
            "3" => Some(0x14),
            "4" => Some(0x15),
            "6" => Some(0x16),
            "5" => Some(0x17),
            "9" => Some(0x19),
            "7" => Some(0x1A),
            "8" => Some(0x1C),
            "0" => Some(0x1D),
            "o" => Some(0x1F),
            "u" => Some(0x20),
            "i" => Some(0x22),
            "p" => Some(0x23),
            "l" => Some(0x25),
            "j" => Some(0x26),
            "k" => Some(0x28),
            "n" => Some(0x2D),
            "m" => Some(0x2E),
            "space" => Some(0x31),
            "f1" => Some(0x7A),
            "f2" => Some(0x78),
            "f3" => Some(0x63),
            "f4" => Some(0x76),
            "f5" => Some(0x60),
            "f6" => Some(0x61),
            "f7" => Some(0x62),
            "f8" => Some(0x64),
            "f9" => Some(0x65),
            "f10" => Some(0x6D),
            "f11" => Some(0x67),
            "f12" => Some(0x6F),
            _ => None,
        }
    }

    /// X11 keysym (`XK_*`) for Linux passive root-window key grabs.
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    pub(super) fn x11_keysym(&self) -> Option<u32> {
        let key = self.key.as_str();
        match key.len() {
            1 => {
                let c = key.chars().next()?.to_ascii_lowercase();
                c.is_ascii_alphanumeric().then_some(c as u32)
            }
            _ if key == "space" => Some(0x0020),
            _ => key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=12).contains(n))
                .map(|n| 0xFFBE + (n - 1)),
        }
    }
}

#[derive(Default)]
pub(super) struct Shared {
    pub bindings: [Option<Binding>; COUNT],
    pub taken: [bool; COUNT],
    #[cfg(windows)]
    pub thread: Option<u32>,
    #[cfg(target_os = "linux")]
    pub wake_window: Option<u32>,
    #[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
    pub stopped: bool,
}

#[cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(super) enum Event {
    Pressed(Hotkey),
    Registered,
}

struct GlobalHotkeys {
    store: Rc<Store>,
    shared: Arc<Mutex<Shared>>,
    stopped_cv: Arc<Condvar>,
}

impl Global for GlobalHotkeys {}

/// Each shortcut as set (`None` when off) and whether another app holds it.
pub fn bindings(cx: &App) -> [(Hotkey, Option<Binding>, bool); COUNT] {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else {
        return std::array::from_fn(|i| (Hotkey::ALL[i], None, false));
    };
    let shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
    std::array::from_fn(|i| (Hotkey::ALL[i], shared.bindings[i].clone(), shared.taken[i]))
}

/// Changes a shortcut (`None` turns it off), keeps it in the store, and registers it anew.
pub fn set(hotkey: Hotkey, binding: Option<Binding>, cx: &mut App) {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return };
    let value = binding.as_ref().map_or_else(|| "off".to_owned(), Binding::to_setting);
    if let Err(err) = global.store.set_setting(hotkey.setting_key(), &value) {
        eprintln!("failed to save the shortcut: {err:#}");
    }
    {
        let mut shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
        let idx = hotkey.index();
        shared.bindings[idx] = binding;
        shared.taken[idx] = false;
    }
    platform::reload(&global.shared);
    cx.refresh_windows();
}

/// Restores all system-wide shortcuts to their built-in defaults in a single batched update.
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
    {
        let mut shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.bindings = defaults;
        shared.taken = [false; COUNT];
    }
    platform::reload(&global.shared);
    cx.refresh_windows();
}

/// Unregisters all global hotkeys synchronously (up to a short timeout) before spawning a
/// replacement instance on update or quitting so another instance does not hit `taken = true`.
pub fn shutdown(cx: &App) {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return };
    platform::shutdown(&global.shared, &global.stopped_cv);
}

/// Registers the shortcuts for the main window.
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

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    platform::start(shared, stopped_cv, tx);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_ids_roundtrip_and_reject_out_of_bounds() {
        assert_eq!(Hotkey::from_id(0), None);
        for hotkey in Hotkey::ALL {
            assert_eq!(Hotkey::from_id(hotkey.id() as usize), Some(hotkey));
        }
        assert_eq!(Hotkey::from_id(COUNT + 1), None);
    }

    #[test]
    fn default_hotkeys_use_non_colliding_ctrl_shift_chords() {
        assert_eq!(Hotkey::QuickAdd.default_setting(), "ctrl-shift-a");
        assert_eq!(Hotkey::ShowHide.default_setting(), "ctrl-shift-e");
        assert_eq!(Hotkey::FocusMini.default_setting(), "off");
        assert_eq!(Hotkey::PinWindow.default_setting(), "off");
        assert!(Binding::parse(Hotkey::QuickAdd.default_setting()).is_some());
        assert!(Binding::parse(Hotkey::ShowHide.default_setting()).is_some());
    }

    #[test]
    fn binding_parses_formats_and_maps_all_platform_keycodes() {
        let binding = Binding::parse("ctrl-shift-a").unwrap();
        assert_eq!(binding.label(), "Ctrl+Shift+A");
        assert_eq!(binding.to_setting(), "ctrl-shift-a");
        assert!(Binding::parse("off").is_none());

        // Every supported key (a-z, 0-9, space, f1-f12) must map cleanly across Windows, macOS, and Linux.
        let mut keys: Vec<String> = ('a'..='z').chain('0'..='9').map(|c| c.to_string()).collect();
        keys.push("space".to_owned());
        keys.extend((1..=12).map(|n| format!("f{n}")));
        for key in keys {
            let parsed = Binding::parse(&format!("ctrl-{key}")).expect(&key);
            assert!(parsed.virtual_key().is_some(), "missing win32 vk for {key}");
            assert!(parsed.mac_key_code().is_some(), "missing macOS keycode for {key}");
            assert!(parsed.x11_keysym().is_some(), "missing X11 keysym for {key}");
        }
    }

    #[test]
    fn binding_parse_supports_platform_aliases_and_modifier_order() {
        let platform_label = if cfg!(target_os = "macos") {
            "Cmd"
        } else if cfg!(target_os = "linux") {
            "Super"
        } else {
            "Win"
        };
        let platform_key = if cfg!(target_os = "macos") { "cmd" } else { "win" };
        let expected_label = format!("{platform_label}+Shift+A");
        let expected_setting = format!("{platform_key}-shift-a");

        for alias in ["cmd", "super", "win", "meta"] {
            let parsed = Binding::parse(&format!("{alias}-shift-a")).expect(alias);
            assert!(parsed.win && parsed.shift && !parsed.ctrl && !parsed.alt);
            assert_eq!(parsed.key, "a");
            assert_eq!(parsed.label(), expected_label);
            assert_eq!(parsed.to_setting(), expected_setting);
            assert!(!parsed.label().starts_with("Shift+"));

            // Inverted input order still normalizes with platform modifier before Shift.
            let inverted = Binding::parse(&format!("shift-{alias}-a")).expect(alias);
            assert_eq!(inverted, parsed);
            assert_eq!(inverted.label(), expected_label);
            assert_eq!(inverted.to_setting(), expected_setting);
        }

        let full = Binding::parse("shift-win-alt-ctrl-f5").unwrap();
        assert_eq!(full.label(), format!("Ctrl+Alt+{platform_label}+Shift+F5"));
        assert_eq!(full.to_setting(), format!("ctrl-alt-{platform_key}-shift-f5"));
    }

    #[test]
    fn binding_parse_rejects_missing_primary_modifier_or_invalid_input() {
        for invalid in ["a", "shift-a", "off", "", "ctrl-", "ctrl-enter", "ctrl-f13", "ctrl-foo-a"] {
            assert_eq!(Binding::parse(invalid), None, "expected {invalid:?} to be rejected");
        }
    }

    #[test]
    fn binding_from_keystroke_accepts_valid_combos_and_rejects_unsafe_keys() {
        let valid = Keystroke::parse("ctrl-shift-space").unwrap();
        let parsed = Binding::from_keystroke(&valid).expect("ctrl-shift-space must be accepted");
        assert!(parsed.ctrl && parsed.shift && !parsed.alt && !parsed.win);
        assert_eq!(parsed.key, "space");
        assert_eq!(parsed.label(), "Ctrl+Shift+Space");

        let platform_combo = Keystroke {
            modifiers: gpui::Modifiers {
                platform: true,
                shift: true,
                ..Default::default()
            },
            key: "K".to_owned(),
            key_char: None,
        };
        let parsed = Binding::from_keystroke(&platform_combo).expect("platform+shift+K must be accepted");
        assert!(parsed.win && parsed.shift && !parsed.ctrl && !parsed.alt);
        assert_eq!(parsed.key, "k");
        assert!(!parsed.label().starts_with("Shift+"));

        for rejected in ["a", "shift-a", "space", "f1", "ctrl-enter", "ctrl-escape", "alt-tab"] {
            let ks = Keystroke::parse(rejected).unwrap();
            assert_eq!(
                Binding::from_keystroke(&ks),
                None,
                "expected keystroke {rejected:?} to be rejected"
            );
        }
    }
}
