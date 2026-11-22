//! macOS menu bar (`NSStatusBar` / `NSStatusItem`) backend.

use std::cell::RefCell;

use dispatch2::DispatchQueue;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSFloatingWindowLevel, NSImage,
    NSMenu, NSMenuItem, NSNormalWindowLevel, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    NSWindow, NSWindowButton, NSWindowDidMiniaturizeNotification,
};
use objc2_foundation::{
    NSData, NSInteger, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSSize,
    NSString,
};

use super::{TrayAction, TrayLabels, TrayPrefs};

const TAG_ACTIVATE: NSInteger = 1000;
const TAG_START_POMO: NSInteger = 1001;
const TAG_START_STOPWATCH: NSInteger = 1002;
const TAG_ALWAYS_ON_TOP: NSInteger = 1003;
const TAG_QUIT: NSInteger = 1004;

static TRAY_ICON_PNG: &[u8] = include_bytes!("../../../../assets/icon/icon-64.png");

define_class!(
    // SAFETY: NSObject has no subclassing invariants; FernTrayTarget has no custom Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct FernTrayTarget;

    unsafe impl NSObjectProtocol for FernTrayTarget {}

    impl FernTrayTarget {
        #[unsafe(method(onTrayMenuItem:))]
        fn on_tray_menu_item(&self, sender: Option<&NSMenuItem>) {
            let Some(item) = sender else { return };
            let action = match item.tag() {
                TAG_ACTIVATE => Some(TrayAction::Activate),
                TAG_START_POMO => Some(TrayAction::StartPomo),
                TAG_START_STOPWATCH => Some(TrayAction::StartStopwatch),
                TAG_ALWAYS_ON_TOP => Some(TrayAction::ToggleAlwaysOnTop),
                TAG_QUIT => Some(TrayAction::Quit),
                _ => None,
            };
            if let Some(action) = action {
                SENDER.with(|slot| {
                    if let Ok(borrow) = slot.try_borrow()
                        && let Some(tx) = borrow.as_ref()
                    {
                        let _ = tx.unbounded_send(action);
                    }
                });
            }
        }

        #[unsafe(method(onMinimizeButton:))]
        fn on_minimize_button(&self, _: Option<&AnyObject>) {
            let action = STATE.with(|slot| {
                let Ok(borrow) = slot.try_borrow() else { return None };
                let state = borrow.as_ref()?;
                Some((state.window.clone(), state.enabled && !state.quitting))
            });
            if let Some((window, hide_to_tray)) = action {
                if hide_to_tray {
                    window.orderOut(None);
                } else {
                    window.performMiniaturize(None);
                }
            }
        }

        #[unsafe(method(onWindowDidMiniaturize:))]
        fn on_window_did_miniaturize(&self, _: Option<&NSNotification>) {
            let should_defer = STATE.with(|slot| {
                let Ok(mut borrow) = slot.try_borrow_mut() else { return false };
                let Some(state) = borrow.as_mut() else { return false };
                if !state.enabled || state.quitting || state.hiding_from_minimize {
                    return false;
                }
                state.hiding_from_minimize = true;
                true
            });
            if should_defer {
                // Defer to the next main-queue turn so we never mutate window state
                // re-entrantly inside AppKit's notification delivery stack.
                DispatchQueue::main().exec_async(|| {
                    let window = STATE.with(|slot| {
                        let Ok(mut borrow) = slot.try_borrow_mut() else { return None };
                        let state = borrow.as_mut()?;
                        if !state.hiding_from_minimize {
                            return None;
                        }
                        if !state.enabled || state.quitting {
                            state.hiding_from_minimize = false;
                            return None;
                        }
                        Some(state.window.clone())
                    });
                    if let Some(window) = window {
                        if window.isMiniaturized() {
                            // Deminiaturize first so AppKit removes the Dock mini-tile
                            // instead of leaving an orphaned Dock entry after `orderOut`.
                            window.deminiaturize(None);
                        }
                        window.orderOut(None);
                        STATE.with(|slot| {
                            if let Ok(mut borrow) = slot.try_borrow_mut()
                                && let Some(state) = borrow.as_mut()
                            {
                                state.hiding_from_minimize = false;
                            }
                        });
                    }
                });
            }
        }
    }
);

struct MinimizeButtonBinding {
    target: Option<Retained<AnyObject>>,
    action: Option<Sel>,
}

struct State {
    window: Retained<NSWindow>,
    orig_minimize_binding: Option<MinimizeButtonBinding>,
    status_item: Retained<NSStatusItem>,
    // Declared before `target` so `NSMenu` (whose items weakly target `FernTrayTarget`)
    // always drops before `target` is released.
    menu: Retained<NSMenu>,
    target: Retained<FernTrayTarget>,
    enabled: bool,
    always_on_top: bool,
    quitting: bool,
    hiding_from_minimize: bool,
    labels: TrayLabels,
    last_tip: String,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static SENDER: RefCell<Option<UnboundedSender<TrayAction>>> = const { RefCell::new(None) };
}

fn sync_minimize_button(state: &State) {
    let Some(binding) = state.orig_minimize_binding.as_ref() else {
        return;
    };
    let Some(button) = state.window.standardWindowButton(NSWindowButton::MiniaturizeButton) else {
        return;
    };
    if state.enabled && !state.quitting {
        let target_obj: &AnyObject = &state.target;
        // SAFETY: `state.target` implements `onMinimizeButton:` and is detached
        // in `set_enabled(false)` and `prepare_quit()` before `target` drops.
        unsafe {
            button.setTarget(Some(target_obj));
            button.setAction(Some(sel!(onMinimizeButton:)));
        }
    } else {
        // SAFETY: Restores the button's original AppKit target and selector captured in `init()`.
        unsafe {
            button.setTarget(binding.target.as_deref());
            button.setAction(binding.action);
        }
    }
}

fn make_item(
    title: &str,
    tag: NSInteger,
    target: &FernTrayTarget,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(sel!(onTrayMenuItem:)),
            &NSString::from_str(""),
        )
    };
    item.setTag(tag);
    let target_obj: &AnyObject = target;
    unsafe { item.setTarget(Some(target_obj)) };
    item
}

fn rebuild_menu(
    menu: &NSMenu,
    target: &FernTrayTarget,
    labels: TrayLabels,
    always_on_top: bool,
    mtm: MainThreadMarker,
) {
    menu.removeAllItems();

    let show_item = make_item("Fern", TAG_ACTIVATE, target, mtm);
    menu.addItem(&show_item);
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let focus_root = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(labels.focus),
            None,
            &NSString::from_str(""),
        )
    };
    let focus_sub = NSMenu::new(mtm);
    focus_sub.addItem(&make_item(labels.start_pomo, TAG_START_POMO, target, mtm));
    focus_sub.addItem(&make_item(labels.stopwatch, TAG_START_STOPWATCH, target, mtm));
    focus_root.setSubmenu(Some(&focus_sub));
    menu.addItem(&focus_root);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let top_item = make_item(labels.always_on_top, TAG_ALWAYS_ON_TOP, target, mtm);
    top_item.setState(if always_on_top { NSControlStateValueOn } else { NSControlStateValueOff });
    menu.addItem(&top_item);

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&make_item(labels.quit, TAG_QUIT, target, mtm));
}

pub(super) fn prepare_channel() -> UnboundedReceiver<TrayAction> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    SENDER.with(|slot| *slot.borrow_mut() = Some(tx));
    rx
}

pub(super) fn init(window: &Window, prefs: TrayPrefs, labels: TrayLabels) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(ns_window) = desktop_common::sticky_windows::macos_ns_window(window) else { return };

    if prefs.always_on_top {
        ns_window.setLevel(NSFloatingWindowLevel);
    }

    let status_bar = NSStatusBar::systemStatusBar();
    let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status_item.button(mtm) {
        let data = NSData::with_bytes(TRAY_ICON_PNG);
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            image.setSize(NSSize::new(18.0, 18.0));
            button.setImage(Some(&image));
        } else {
            button.setTitle(&NSString::from_str("Fern"));
        }
        button.setToolTip(Some(&NSString::from_str("Fern")));
    }

    let target: Retained<FernTrayTarget> = {
        let this = FernTrayTarget::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
    let menu = NSMenu::new(mtm);
    rebuild_menu(&menu, &target, labels, prefs.always_on_top, mtm);
    status_item.setMenu(Some(&menu));

    let orig_minimize_binding = ns_window
        .standardWindowButton(NSWindowButton::MiniaturizeButton)
        .map(|button| MinimizeButtonBinding {
            target: button.target(),
            action: button.action(),
        });

    let target_obj: &AnyObject = &target;
    let window_obj: &AnyObject = &ns_window;
    // SAFETY: `target` implements `onWindowDidMiniaturize:` and is unregistered in
    // `prepare_quit()` before dropping.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            target_obj,
            sel!(onWindowDidMiniaturize:),
            Some(NSWindowDidMiniaturizeNotification),
            Some(window_obj),
        );
    }

    let state = State {
        window: ns_window,
        orig_minimize_binding,
        status_item,
        menu,
        target,
        enabled: prefs.minimize_to_tray,
        always_on_top: prefs.always_on_top,
        quitting: false,
        hiding_from_minimize: false,
        labels,
        last_tip: "Fern".to_owned(),
    };
    if state.enabled {
        sync_minimize_button(&state);
    }

    STATE.with(|slot| {
        *slot.borrow_mut() = Some(state);
    });
}

pub(super) fn set_tooltip(tip: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    STATE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };
        if state.last_tip == tip {
            return;
        }
        state.last_tip.clear();
        state.last_tip.push_str(tip);
        if let Some(button) = state.status_item.button(mtm) {
            button.setToolTip(Some(&NSString::from_str(tip)));
            // Show compact running clock beside the menu bar icon when Focus is active.
            let title = tip.strip_prefix("Fern — ").unwrap_or("");
            if title.is_empty() {
                button.setTitle(&NSString::from_str(""));
            } else {
                button.setTitle(&NSString::from_str(&format!(" {title}")));
            }
        }
    });
}

pub(super) fn set_enabled(is_enabled: bool) {
    STATE.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.enabled = is_enabled;
            if !is_enabled {
                state.hiding_from_minimize = false;
            }
            sync_minimize_button(state);
        }
    });
}

pub(super) fn set_always_on_top(topmost: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    STATE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };
        state.always_on_top = topmost;
        state.window.setLevel(if topmost { NSFloatingWindowLevel } else { NSNormalWindowLevel });
        rebuild_menu(&state.menu, &state.target, state.labels, topmost, mtm);
    });
}

pub(super) fn update_labels(labels: TrayLabels) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    STATE.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(state) = borrow.as_mut() else { return };
        state.labels = labels;
        rebuild_menu(&state.menu, &state.target, labels, state.always_on_top, mtm);
    });
}

pub(super) fn hide_to_tray() -> bool {
    let window = STATE.with(|slot| {
        let Ok(borrow) = slot.try_borrow() else { return None };
        let state = borrow.as_ref()?;
        (!state.quitting).then(|| state.window.clone())
    });
    let Some(window) = window else { return false };
    window.orderOut(None);
    true
}

pub(super) fn hide_to_tray_if_enabled() -> bool {
    let window = STATE.with(|slot| {
        let Ok(borrow) = slot.try_borrow() else { return None };
        let state = borrow.as_ref()?;
        (state.enabled && !state.quitting).then(|| state.window.clone())
    });
    let Some(window) = window else { return false };
    window.orderOut(None);
    true
}

pub(super) fn restore_main_window() -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let window = STATE.with(|slot| {
        let Ok(mut borrow) = slot.try_borrow_mut() else { return None };
        let state = borrow.as_mut()?;
        state.hiding_from_minimize = false;
        Some(state.window.clone())
    });
    let Some(window) = window else { return false };
    let app = NSApplication::sharedApplication(mtm);
    app.unhide(None);
    // `activateIgnoringOtherApps(true)` works on macOS 12+ (`activate` is 14+ only)
    // and reliably brings the window to front from global hotkeys.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    if window.isMiniaturized() {
        window.deminiaturize(None);
    }
    window.makeKeyAndOrderFront(None);
    true
}

pub(super) fn prepare_quit() {
    let state = STATE.with(|slot| slot.borrow_mut().take());
    if let Some(mut state) = state {
        let was_enabled = state.enabled;
        state.quitting = true;
        if was_enabled {
            sync_minimize_button(&state);
        }
        let target_obj: &AnyObject = &state.target;
        // SAFETY: `state.target` was registered as an observer in `init()`.
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(target_obj);
        }
        state.status_item.setMenu(None);
        state.menu.removeAllItems();
        NSStatusBar::systemStatusBar().removeStatusItem(&state.status_item);
    }
    SENDER.with(|slot| *slot.borrow_mut() = None);
}
