//! Linux global hotkeys using X11 passive root-window key grabs (`x11rb`).
//!
//! Runs on a dedicated background thread and handles CapsLock/NumLock modifier variants cleanly.
//! On pure Wayland sessions without an X11 display, exits gracefully without panicking.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{
    ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, KeyButMask,
    ModMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::{Binding, COUNT, Event, Hotkey, Shared};

const CMD_RELOAD: u32 = 1;
const CMD_QUIT: u32 = 2;

fn lock_variants() -> [ModMask; 4] {
    [
        ModMask::from(0u16),
        ModMask::LOCK,
        ModMask::M2,
        ModMask::LOCK | ModMask::M2,
    ]
}

pub(super) fn start(shared: Arc<Mutex<Shared>>, stopped_cv: Arc<Condvar>, tx: UnboundedSender<Event>) {
    let _ = std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || listen(shared, stopped_cv, tx));
}

fn send_wake(wake_window: Option<u32>, cmd: u32) {
    let Some(win) = wake_window else { return };
    let Ok((conn, _)) = RustConnection::connect(None) else { return };
    let Some(atom) = conn.intern_atom(false, b"FERN_HOTKEY_WAKE").ok().and_then(|c| c.reply().ok()) else {
        return;
    };
    let event = ClientMessageEvent::new(32, win, atom.atom, [cmd, 0, 0, 0, 0]);
    let _ = conn.send_event(false, win, EventMask::NO_EVENT, event);
    let _ = conn.flush();
}

pub(super) fn reload(shared: &Arc<Mutex<Shared>>) {
    let win = shared.lock().unwrap_or_else(|e| e.into_inner()).wake_window;
    send_wake(win, CMD_RELOAD);
}

pub(super) fn shutdown(shared: &Arc<Mutex<Shared>>, stopped_cv: &Arc<Condvar>) {
    let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(win) = guard.wake_window else { return };
    if guard.stopped {
        return;
    }
    drop(guard);
    send_wake(Some(win), CMD_QUIT);
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
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

fn keysym_to_keycode(conn: &RustConnection, keysym: u32) -> Option<u8> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode.saturating_sub(min).saturating_add(1);
    let mapping = conn.get_keyboard_mapping(min, count).ok()?.reply().ok()?;
    let per = usize::from(mapping.keysyms_per_keycode);
    if per == 0 {
        return None;
    }
    for (idx, syms) in mapping.keysyms.chunks_exact(per).enumerate() {
        if syms.iter().take(2).any(|&s| s == keysym) {
            return min.checked_add(idx as u8);
        }
    }
    None
}

fn binding_mod_mask(binding: &Binding) -> ModMask {
    let mut mask = ModMask::from(0u16);
    if binding.ctrl {
        mask |= ModMask::CONTROL;
    }
    if binding.alt {
        mask |= ModMask::M1;
    }
    if binding.shift {
        mask |= ModMask::SHIFT;
    }
    if binding.win {
        mask |= ModMask::M4;
    }
    mask
}

fn event_clean_mods(state: KeyButMask) -> u16 {
    let relevant = u16::from(ModMask::CONTROL | ModMask::M1 | ModMask::SHIFT | ModMask::M4);
    u16::from(state) & relevant
}

fn listen(shared: Arc<Mutex<Shared>>, stopped_cv: Arc<Condvar>, tx: UnboundedSender<Event>) {
    let Ok((conn, screen_num)) = RustConnection::connect(None) else {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.stopped = true;
        stopped_cv.notify_all();
        return;
    };
    let root = conn.setup().roots[screen_num].root;
    let Some(wake_atom) = conn.intern_atom(false, b"FERN_HOTKEY_WAKE").ok().and_then(|c| c.reply().ok()) else {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.stopped = true;
        stopped_cv.notify_all();
        return;
    };
    let Ok(wake_win) = conn.generate_id() else {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.stopped = true;
        stopped_cv.notify_all();
        return;
    };
    let _ = conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        wake_win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new(),
    );
    let _ = conn.flush();
    shared.lock().unwrap_or_else(|e| e.into_inner()).wake_window = Some(wake_win);

    let mut active: [Option<(u8, ModMask)>; COUNT] = [None; COUNT];
    let ungrab_all = |conn: &RustConnection, active: &mut [Option<(u8, ModMask)>; COUNT]| {
        for slot in active.iter_mut() {
            if let Some((keycode, mods)) = slot.take() {
                for extra in lock_variants() {
                    let _ = conn.ungrab_key(keycode, root, mods | extra);
                }
            }
        }
        let _ = conn.flush();
    };
    let register = |conn: &RustConnection, shared: &Mutex<Shared>, active: &mut [Option<(u8, ModMask)>; COUNT]| {
        ungrab_all(conn, active);
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        for hotkey in Hotkey::ALL {
            let idx = hotkey.index();
            let Some(binding) = guard.bindings[idx].clone() else {
                guard.taken[idx] = false;
                continue;
            };
            let Some(keysym) = binding.x11_keysym() else {
                guard.taken[idx] = false;
                continue;
            };
            let Some(keycode) = keysym_to_keycode(conn, keysym) else {
                guard.taken[idx] = false;
                continue;
            };
            let mods = binding_mod_mask(&binding);
            let mut failed = false;
            for extra in lock_variants() {
                if conn
                    .grab_key(false, root, mods | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                    .ok()
                    .and_then(|c| c.check().ok())
                    .is_none()
                {
                    failed = true;
                }
            }
            if failed {
                for extra in lock_variants() {
                    let _ = conn.ungrab_key(keycode, root, mods | extra);
                }
                guard.taken[idx] = true;
            } else {
                active[idx] = Some((keycode, mods));
                guard.taken[idx] = false;
            }
        }
        let _ = conn.flush();
    };

    register(&conn, &shared, &mut active);
    if tx.unbounded_send(Event::Registered).is_ok() {
        while let Ok(ev) = conn.wait_for_event() {
            match ev {
                XEvent::KeyPress(key_ev) => {
                    let mods = event_clean_mods(key_ev.state);
                    let matched = Hotkey::ALL.into_iter().find(|&hk| {
                        active[hk.index()]
                            .is_some_and(|(code, mask)| code == key_ev.detail && u16::from(mask) == mods)
                    });
                    if let Some(hotkey) = matched
                        && tx.unbounded_send(Event::Pressed(hotkey)).is_err()
                    {
                        break;
                    }
                }
                XEvent::ClientMessage(msg) if msg.window == wake_win && msg.type_ == wake_atom.atom => {
                    match msg.data.as_data32()[0] {
                        CMD_RELOAD => {
                            register(&conn, &shared, &mut active);
                            if tx.unbounded_send(Event::Registered).is_err() {
                                break;
                            }
                        }
                        CMD_QUIT => break,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    ungrab_all(&conn, &mut active);
    let _ = conn.destroy_window(wake_win);
    let _ = conn.flush();
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    guard.stopped = true;
    guard.wake_window = None;
    stopped_cv.notify_all();
}
