//! Linux global hotkeys using X11 passive root-window key grabs (`x11rb`).
//!
//! Runs on a dedicated background thread and handles CapsLock/NumLock modifier variants cleanly.
//! On pure Wayland sessions without an X11 display, exits gracefully without panicking.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{
    ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, KeyButMask,
    Mapping, ModMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::{Binding, COUNT, Event, Hotkey, Shared};

const CMD_RELOAD: u32 = 1;
const CMD_QUIT: u32 = 2;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

fn lock_variants() -> [ModMask; 4] {
    [
        ModMask::from(0u16),
        ModMask::LOCK,
        ModMask::M2,
        ModMask::LOCK | ModMask::M2,
    ]
}

pub(super) fn start(shared: Arc<Mutex<Shared>>, stopped_cv: Arc<Condvar>, tx: UnboundedSender<Event>) {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    let shared_thread = Arc::clone(&shared);
    let stopped_thread = Arc::clone(&stopped_cv);
    if std::thread::Builder::new()
        .name("hotkeys".into())
        .spawn(move || listen(shared_thread, stopped_thread, tx))
        .is_err()
    {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.stopped = true;
        stopped_cv.notify_all();
    }
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
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    if guard.stopped {
        return;
    }
    let win = guard.wake_window;
    drop(guard);
    if let Some(win) = win {
        send_wake(Some(win), CMD_QUIT);
    }
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

struct KeyboardMapping {
    min_keycode: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
}

fn fetch_keyboard_mapping(conn: &RustConnection) -> Option<KeyboardMapping> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode.saturating_sub(min).saturating_add(1);
    let reply = conn.get_keyboard_mapping(min, count).ok()?.reply().ok()?;
    let per = usize::from(reply.keysyms_per_keycode);
    if per == 0 {
        return None;
    }
    Some(KeyboardMapping {
        min_keycode: min,
        keysyms_per_keycode: per,
        keysyms: reply.keysyms,
    })
}

fn keysym_to_keycode(mapping: &KeyboardMapping, keysym: u32) -> Option<u8> {
    // Prefer Group 0 (primary layout, columns 0..2).
    for (idx, syms) in mapping.keysyms.chunks_exact(mapping.keysyms_per_keycode).enumerate() {
        if syms.iter().take(2).any(|&s| s == keysym) {
            let offset = u8::try_from(idx).ok()?;
            return mapping.min_keycode.checked_add(offset);
        }
    }
    // Fall back to all columns so non-Latin primary XKB layouts (e.g. Russian) still
    // resolve Latin hotkey keysyms in secondary groups.
    for (idx, syms) in mapping.keysyms.chunks_exact(mapping.keysyms_per_keycode).enumerate() {
        if syms.contains(&keysym) {
            let offset = u8::try_from(idx).ok()?;
            return mapping.min_keycode.checked_add(offset);
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
    if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.stopped = true;
        stopped_cv.notify_all();
        return;
    }
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
    let should_stop = {
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            true
        } else {
            guard.wake_window = Some(wake_win);
            false
        }
    };

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
        let bindings = {
            let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            guard.bindings.clone()
        };
        let mapping = fetch_keyboard_mapping(conn);
        let mut taken = [false; COUNT];
        for hotkey in Hotkey::ALL {
            let idx = hotkey.index();
            let Some(binding) = &bindings[idx] else {
                continue;
            };
            let Some(keysym) = binding.x11_keysym() else {
                continue;
            };
            let Some(keycode) = mapping.as_ref().and_then(|m| keysym_to_keycode(m, keysym)) else {
                continue;
            };
            let mods = binding_mod_mask(binding);
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
                taken[idx] = true;
            } else {
                active[idx] = Some((keycode, mods));
            }
        }
        let _ = conn.flush();
        shared.lock().unwrap_or_else(|e| e.into_inner()).taken = taken;
    };

    if !should_stop && !SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        register(&conn, &shared, &mut active);
        if !SHUTDOWN_REQUESTED.load(Ordering::SeqCst) && tx.unbounded_send(Event::Registered).is_ok() {
            while !SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                let Ok(ev) = conn.wait_for_event() else { break };
                match ev {
                    XEvent::KeyPress(key_ev) => {
                        let mods = event_clean_mods(key_ev.state);
                        let matched = Hotkey::ALL.into_iter().find(|&hk| {
                            active[hk.index()].is_some_and(|(code, mask)| {
                                code == key_ev.detail && u16::from(mask) == mods
                            })
                        });
                        if let Some(hotkey) = matched
                            && tx.unbounded_send(Event::Pressed(hotkey)).is_err()
                        {
                            break;
                        }
                    }
                    XEvent::MappingNotify(m) if m.request == Mapping::KEYBOARD => {
                        register(&conn, &shared, &mut active);
                        if tx.unbounded_send(Event::Registered).is_err() {
                            break;
                        }
                    }
                    XEvent::ClientMessage(msg)
                        if msg.window == wake_win && msg.type_ == wake_atom.atom =>
                    {
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
    }

    ungrab_all(&conn, &mut active);
    let _ = conn.destroy_window(wake_win);
    let _ = conn.flush();
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    guard.stopped = true;
    guard.wake_window = None;
    stopped_cv.notify_all();
}
