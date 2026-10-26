//! System-wide shortcuts, as the original's desktop app has them: they work while another app
//! is in front. Windows only, where `RegisterHotKey` makes them; GPUI has no such shortcuts.
//!
//! - Ctrl+Shift+A: Quick Add, a small field over every window (see `quick_add_window`).
//! - Ctrl+Shift+E: shows the main window, or hides it (minimized) when it is in front.

use gpui::{App, WindowHandle};

use crate::app_view::AppView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hotkey {
    QuickAdd,
    ShowHide,
}

/// Registers the shortcuts for the main window; a shortcut another app holds is left out.
#[cfg(windows)]
pub fn init(window: WindowHandle<AppView>, cx: &mut App) {
    use futures::StreamExt as _;

    // Debug builds: `FERN_DEV_STATE=quick-add-window` opens Quick Add, as its shortcut would.
    #[cfg(debug_assertions)]
    if std::env::var("FERN_DEV_STATE").is_ok_and(|state| state.contains("quick-add-window")) {
        cx.defer(move |cx| crate::quick_add_window::open(window, cx));
    }
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || listen(tx))
        .ok();
    cx.spawn(async move |cx| {
        while let Some(hotkey) = rx.next().await {
            let handled = match hotkey {
                Hotkey::QuickAdd => {
                    cx.update(|cx| crate::quick_add_window::open(window, cx));
                    Ok(())
                }
                Hotkey::ShowHide => window
                    .update(cx, |_, window, _| {
                        if window.is_window_active() { window.minimize_window() } else { window.activate_window() }
                    })
                    .map_err(|_| ()),
            };
            if handled.is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(not(windows))]
pub fn init(_window: WindowHandle<AppView>, _cx: &mut App) {}

/// The shortcuts' thread: registers them and passes each press on until the app ends.
#[cfg(windows)]
fn listen(tx: futures::channel::mpsc::UnboundedSender<Hotkey>) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey};
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    const KEYS: [(Hotkey, u32); 2] = [(Hotkey::QuickAdd, b'A' as u32), (Hotkey::ShowHide, b'E' as u32)];
    for (id, (_, key)) in KEYS.iter().enumerate() {
        // Thread-wide (no window): WM_HOTKEY comes to this thread's queue.
        if let Err(err) = unsafe { RegisterHotKey(None, id as i32 + 1, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, *key) } {
            eprintln!("cannot register Ctrl+Shift+{}: {err}", char::from_u32(*key).unwrap_or('?'));
        }
    }
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        if msg.message == WM_HOTKEY
            && let Some((hotkey, _)) = KEYS.get((msg.wParam.0 as usize).wrapping_sub(1))
            && tx.unbounded_send(*hotkey).is_err()
        {
            break;
        }
    }
}
