//! Windows global hotkey listener thread using `RegisterHotKey`.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_HOTKEY, WM_QUIT,
    WM_USER,
};

use super::{Event, Hotkey, Shared};

pub(super) fn start(shared: Arc<Mutex<Shared>>, stopped_cv: Arc<Condvar>, tx: UnboundedSender<Event>) {
    let _ = std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || listen(shared, stopped_cv, tx));
}

pub(super) fn reload(shared: &Arc<Mutex<Shared>>) {
    let thread = shared.lock().unwrap_or_else(|e| e.into_inner()).thread;
    if let Some(thread) = thread {
        unsafe { PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0)) }.ok();
    }
}

pub(super) fn shutdown(shared: &Arc<Mutex<Shared>>, stopped_cv: &Arc<Condvar>) {
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(thread) = guard.thread else { return };
    if guard.stopped {
        return;
    }
    let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    let deadline = Instant::now() + Duration::from_millis(500);
    while !guard.stopped {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { break };
        let (next, timeout) = stopped_cv
            .wait_timeout(guard, remaining)
            .unwrap_or_else(|e| e.into_inner());
        guard = next;
        if timeout.timed_out() {
            break;
        }
    }
}

fn listen(shared: Arc<Mutex<Shared>>, stopped_cv: Arc<Condvar>, tx: UnboundedSender<Event>) {
    let unregister_all = || {
        for hotkey in Hotkey::ALL {
            unsafe { UnregisterHotKey(None, hotkey.id() as i32) }.ok();
        }
    };
    let register = |shared: &Mutex<Shared>| {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        for hotkey in Hotkey::ALL {
            let idx = hotkey.index();
            unsafe { UnregisterHotKey(None, hotkey.id() as i32) }.ok();
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
            shared.taken[idx] = unsafe {
                RegisterHotKey(None, hotkey.id() as i32, HOT_KEY_MODIFIERS(modifiers.0), key)
            }
            .is_err();
        }
    };

    let mut msg = MSG::default();
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE) };
    shared.lock().unwrap_or_else(|e| e.into_inner()).thread = Some(unsafe { GetCurrentThreadId() });
    register(&shared);
    if tx.unbounded_send(Event::Registered).is_ok() {
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            let event = match msg.message {
                WM_HOTKEY => {
                    let Some(hotkey) = Hotkey::from_id(msg.wParam.0) else { continue };
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
