//! macOS global hotkeys using Carbon HIToolbox (`RegisterEventHotKey`).
//!
//! Unlike `CGEventTap`, `RegisterEventHotKey` works system-wide without requiring Accessibility
//! or Input Monitoring permissions and dispatches directly on the main `NSApplication` run loop.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};

use futures::channel::mpsc::UnboundedSender;

use super::{COUNT, Event, Hotkey, Shared};

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const FERN_SIGNATURE: u32 = u32::from_be_bytes(*b"FERN");
const EVENT_NOT_HANDLED_ERR: OSStatus = -9874;

const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandlerUPP = Option<
    unsafe extern "C" fn(
        next_handler: EventHandlerCallRef,
        event: EventRef,
        user_data: *mut c_void,
    ) -> OSStatus,
>;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerUPP,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RemoveEventHandler(handler: EventHandlerRef) -> OSStatus;
    fn RegisterEventHotKey(
        hotkey_code: u32,
        hotkey_modifiers: u32,
        hotkey_id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hotkey: EventHotKeyRef) -> OSStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
}

struct MacState {
    handler: EventHandlerRef,
    refs: [Option<EventHotKeyRef>; COUNT],
    tx: UnboundedSender<Event>,
}

thread_local! {
    static MAC_STATE: RefCell<Option<MacState>> = const { RefCell::new(None) };
}

unsafe extern "C" fn hotkey_callback(
    _next_handler: EventHandlerCallRef,
    event: EventRef,
    _user_data: *mut c_void,
) -> OSStatus {
    let mut hk_id = EventHotKeyID::default();
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            ptr::null_mut(),
            (&raw mut hk_id).cast(),
        )
    };
    if status == 0
        && hk_id.signature == FERN_SIGNATURE
        && let Some(hotkey) = Hotkey::from_id(hk_id.id as usize)
    {
        let dispatched = MAC_STATE.with(|slot| {
            if let Ok(borrow) = slot.try_borrow()
                && let Some(state) = borrow.as_ref()
            {
                state.tx.unbounded_send(Event::Pressed(hotkey)).is_ok()
            } else {
                false
            }
        });
        if dispatched {
            return 0;
        }
    }
    EVENT_NOT_HANDLED_ERR
}

pub(super) fn start(shared: Arc<Mutex<Shared>>, _stopped_cv: Arc<Condvar>, tx: UnboundedSender<Event>) {
    let target = unsafe { GetApplicationEventTarget() };
    if target.is_null() {
        return;
    }
    let spec = EventTypeSpec {
        event_class: K_EVENT_CLASS_KEYBOARD,
        event_kind: K_EVENT_HOT_KEY_PRESSED,
    };
    let mut handler: EventHandlerRef = ptr::null_mut();
    let status = unsafe {
        InstallEventHandler(
            target,
            Some(hotkey_callback),
            1,
            &spec,
            ptr::null_mut(),
            &mut handler,
        )
    };
    if status != 0 || handler.is_null() {
        desktop_common::logging::log_error(
            "hotkeys",
            format!("failed to install macOS global hotkey handler: OSStatus {status}"),
        );
        return;
    }
    MAC_STATE.with(|slot| {
        *slot.borrow_mut() = Some(MacState {
            handler,
            refs: [None; COUNT],
            tx,
        });
    });
    reload(&shared);
}

pub(super) fn reload(shared: &Arc<Mutex<Shared>>) {
    let target = unsafe { GetApplicationEventTarget() };
    if target.is_null() {
        return;
    }
    MAC_STATE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };
        // Pass 1: unregister all existing bindings first so swapping chords or
        // resetting defaults never self-conflicts with `eventHotKeyExistsErr`.
        for hk_ref in &mut state.refs {
            if let Some(handle) = hk_ref.take() {
                unsafe { UnregisterEventHotKey(handle) };
            }
        }
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        let shared = &mut *guard;
        // Pass 2: register all active bindings from the updated table.
        for hotkey in Hotkey::ALL {
            let idx = hotkey.index();
            let Some(binding) = shared.bindings[idx].as_ref() else {
                shared.taken[idx] = false;
                continue;
            };
            let Some(code) = binding.mac_key_code() else {
                shared.taken[idx] = false;
                continue;
            };
            let mut mods = 0u32;
            if binding.ctrl {
                mods |= CONTROL_KEY;
            }
            if binding.alt {
                mods |= OPTION_KEY;
            }
            if binding.shift {
                mods |= SHIFT_KEY;
            }
            if binding.win {
                mods |= CMD_KEY;
            }
            let hk_id = EventHotKeyID {
                signature: FERN_SIGNATURE,
                id: hotkey.id(),
            };
            let mut hk_ref: EventHotKeyRef = ptr::null_mut();
            let status = unsafe { RegisterEventHotKey(code, mods, hk_id, target, 0, &mut hk_ref) };
            if status == 0 && !hk_ref.is_null() {
                state.refs[idx] = Some(hk_ref);
                shared.taken[idx] = false;
            } else {
                shared.taken[idx] = true;
            }
        }
        let _ = state.tx.unbounded_send(Event::Registered);
    });
}

pub(super) fn shutdown(shared: &Arc<Mutex<Shared>>, stopped_cv: &Arc<Condvar>) {
    let state = MAC_STATE.with(|slot| slot.borrow_mut().take());
    if let Some(mut state) = state {
        for hk_ref in &mut state.refs {
            if let Some(handle) = hk_ref.take() {
                unsafe { UnregisterEventHotKey(handle) };
            }
        }
        if !state.handler.is_null() {
            unsafe { RemoveEventHandler(state.handler) };
        }
    }
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    guard.stopped = true;
    stopped_cv.notify_all();
}
