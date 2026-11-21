//! macOS menu bar (`NSStatusBar` / `NSStatusItem`) backend.

use std::cell::RefCell;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui::Window;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSFloatingWindowLevel, NSImage,
    NSMenu, NSMenuItem, NSNormalWindowLevel, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    NSWindow,
};
use objc2_foundation::{NSData, NSInteger, NSObject, NSObjectProtocol, NSSize, NSString};

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
                    if let Some(tx) = slot.borrow().as_ref() {
                        let _ = tx.unbounded_send(action);
                    }
                });
            }
        }
    }
);

struct State {
    window: Retained<NSWindow>,
    status_item: Retained<NSStatusItem>,
    target: Retained<FernTrayTarget>,
    menu: Retained<NSMenu>,
    enabled: bool,
    always_on_top: bool,
    quitting: bool,
    labels: TrayLabels,
    last_tip: String,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static SENDER: RefCell<Option<UnboundedSender<TrayAction>>> = const { RefCell::new(None) };
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
    let target_obj: &AnyObject = unsafe { &*(target as *const FernTrayTarget as *const AnyObject) };
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

    STATE.with(|slot| {
        *slot.borrow_mut() = Some(State {
            window: ns_window,
            status_item,
            target,
            menu,
            enabled: prefs.minimize_to_tray,
            always_on_top: prefs.always_on_top,
            quitting: false,
            labels,
            last_tip: "Fern".to_owned(),
        });
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
    STATE.with(|slot| {
        let borrow = slot.borrow();
        let Some(state) = borrow.as_ref() else { return false };
        if state.quitting {
            return false;
        }
        state.window.orderOut(None);
        true
    })
}

pub(super) fn hide_to_tray_if_enabled() -> bool {
    STATE.with(|slot| {
        let borrow = slot.borrow();
        let Some(state) = borrow.as_ref() else { return false };
        if !state.enabled || state.quitting {
            return false;
        }
        state.window.orderOut(None);
        true
    })
}

pub(super) fn restore_main_window() -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    STATE.with(|slot| {
        let borrow = slot.borrow();
        let Some(state) = borrow.as_ref() else { return false };
        let app = NSApplication::sharedApplication(mtm);
        app.unhide(None);
        app.activate();
        if state.window.isMiniaturized() {
            state.window.deminiaturize(None);
        }
        state.window.makeKeyAndOrderFront(None);
        true
    })
}

pub(super) fn prepare_quit() {
    STATE.with(|slot| {
        if let Some(mut state) = slot.borrow_mut().take() {
            state.quitting = true;
            NSStatusBar::systemStatusBar().removeStatusItem(&state.status_item);
        }
    });
    SENDER.with(|slot| *slot.borrow_mut() = None);
}
