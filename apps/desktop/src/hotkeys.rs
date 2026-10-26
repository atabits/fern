//! System-wide shortcuts, as the original's desktop app has them: they work while another app
//! is in front. Windows only, where `RegisterHotKey` makes them; GPUI has no such shortcuts.
//!
//! - Quick Add (Ctrl+Shift+A at first): a small field over every window (see `quick_add_window`).
//! - Show / Hide (Ctrl+Shift+E at first): shows the main window, or hides it (minimized) when it
//!   is in front.
//!
//! Each can be changed or turned off from the Shortcuts panel, as in the original's Settings;
//! a shortcut another app holds is shown as taken.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use fern_core::Store;
use gpui::{App, Global, Keystroke, WindowHandle};

use crate::app_view::AppView;

/// The shortcuts, in the order the panel lists them.
pub const COUNT: usize = 2;
const DEFAULTS: [&str; COUNT] = ["ctrl-shift-a", "ctrl-shift-e"];
const SETTINGS: [&str; COUNT] = ["hotkey_quick_add", "hotkey_show_hide"];

/// A key with its modifiers, kept as GPUI writes keystrokes ("ctrl-shift-a").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: String,
}

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
#[derive(Default)]
struct Shared {
    bindings: [Option<Binding>; COUNT],
    taken: [bool; COUNT],
    /// The thread to wake when the bindings change.
    thread: Option<u32>,
}

struct GlobalHotkeys {
    store: Rc<Store>,
    shared: Arc<Mutex<Shared>>,
}

impl Global for GlobalHotkeys {}

/// Each shortcut as set (`None` when off) and whether another app holds it.
pub fn bindings(cx: &App) -> [(Option<Binding>, bool); COUNT] {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return Default::default() };
    let shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
    std::array::from_fn(|i| (shared.bindings[i].clone(), shared.taken[i]))
}

/// Changes a shortcut (`None` turns it off), keeps it in the store and registers it anew.
pub fn set(index: usize, binding: Option<Binding>, cx: &mut App) {
    let Some(global) = cx.try_global::<GlobalHotkeys>() else { return };
    let value = binding.as_ref().map_or_else(|| "off".to_owned(), Binding::to_setting);
    if let Err(err) = global.store.set_setting(SETTINGS[index], &value) {
        eprintln!("failed to save the shortcut: {err:#}");
    }
    let thread = {
        let mut shared = global.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.bindings[index] = binding;
        shared.taken[index] = false;
        shared.thread
    };
    #[cfg(windows)]
    if let Some(thread) = thread {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};
        unsafe { PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0)) }.ok();
    }
    #[cfg(not(windows))]
    let _ = thread;
    cx.refresh_windows();
}

/// Registers the shortcuts for the main window.
pub fn init(window: WindowHandle<AppView>, store: Rc<Store>, cx: &mut App) {
    let bindings = std::array::from_fn(|i| match store.setting(SETTINGS[i]).ok().flatten() {
        Some(value) if value == "off" => None,
        Some(value) => Binding::parse(&value),
        None => Binding::parse(DEFAULTS[i]),
    });
    let shared = Arc::new(Mutex::new(Shared { bindings, ..Default::default() }));
    cx.set_global(GlobalHotkeys { store, shared: shared.clone() });
    // Debug builds: `FERN_DEV_STATE=quick-add-window` opens Quick Add, as its shortcut would.
    #[cfg(debug_assertions)]
    if std::env::var("FERN_DEV_STATE").is_ok_and(|state| state.contains("quick-add-window")) {
        cx.defer(move |cx| crate::quick_add_window::open(window, cx));
    }
    #[cfg(windows)]
    start(window, shared, cx);
    #[cfg(not(windows))]
    let _ = (window, shared);
}

#[cfg(windows)]
fn start(window: WindowHandle<AppView>, shared: Arc<Mutex<Shared>>, cx: &mut App) {
    use futures::StreamExt as _;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new().name("hotkeys".into()).spawn(move || listen(shared, tx)).ok();
    cx.spawn(async move |cx| {
        while let Some(event) = rx.next().await {
            let handled = match event {
                Event::Pressed(0) => {
                    cx.update(|cx| crate::quick_add_window::open(window, cx));
                    Ok(())
                }
                Event::Pressed(_) => window
                    .update(cx, |_, window, _| {
                        if window.is_window_active() { window.minimize_window() } else { window.activate_window() }
                    })
                    .map_err(|_| ()),
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
    Pressed(usize),
    Registered,
}

/// The shortcuts' thread: registers them, passes each press on, and registers them anew when
/// they change, until the app ends.
#[cfg(windows)]
fn listen(shared: Arc<Mutex<Shared>>, tx: futures::channel::mpsc::UnboundedSender<Event>) {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, WM_APP, WM_HOTKEY, WM_USER};

    let register = |shared: &Mutex<Shared>| {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..COUNT {
            unsafe { UnregisterHotKey(None, i as i32 + 1) }.ok();
            let Some(binding) = shared.bindings[i].clone() else { continue };
            let Some(key) = binding.virtual_key() else { continue };
            let mut modifiers = MOD_NOREPEAT;
            for (on, flag) in [(binding.ctrl, MOD_CONTROL), (binding.alt, MOD_ALT), (binding.shift, MOD_SHIFT), (binding.win, MOD_WIN)] {
                if on {
                    modifiers |= flag;
                }
            }
            // Thread-wide (no window): WM_HOTKEY comes to this thread's queue.
            shared.taken[i] = unsafe { RegisterHotKey(None, i as i32 + 1, HOT_KEY_MODIFIERS(modifiers.0), key) }.is_err();
        }
    };
    // The queue exists once the thread has asked for a message; only then can it be woken.
    let mut msg = MSG::default();
    unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) }.ok();
    shared.lock().unwrap_or_else(|e| e.into_inner()).thread = Some(unsafe { GetCurrentThreadId() });
    register(&shared);
    if tx.unbounded_send(Event::Registered).is_err() {
        return;
    }
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        let event = match msg.message {
            WM_HOTKEY => Event::Pressed((msg.wParam.0 as usize).wrapping_sub(1)),
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
