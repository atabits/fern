//! Linux system tray backend using freedesktop / KDE `StatusNotifierItem` (`ksni` over `zbus`).

use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui::Window;
use ksni::blocking::{Handle, TrayMethods as _};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::{Icon, ToolTip, Tray};

use super::{TrayAction, TrayLabels, TrayPrefs};
use crate::i18n;

static TRAY_ICON_PNG: &[u8] = include_bytes!("../../../../assets/icon/icon-64.png");

struct FernTray {
    tx: UnboundedSender<TrayAction>,
    labels: TrayLabels,
    always_on_top: bool,
    tooltip: String,
    icon: Vec<Icon>,
}

impl Tray for FernTray {
    fn id(&self) -> String {
        "fern-tasks".into()
    }

    fn title(&self) -> String {
        self.tooltip.clone()
    }

    fn icon_name(&self) -> String {
        if self.icon.is_empty() {
            "fern-tasks".into()
        } else {
            String::new()
        }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.tooltip.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.unbounded_send(TrayAction::Activate);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Fern".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.unbounded_send(TrayAction::Activate);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            SubMenu {
                label: self.labels.focus.into(),
                submenu: vec![
                    StandardItem {
                        label: self.labels.start_pomo.into(),
                        activate: Box::new(|tray: &mut Self| {
                            let _ = tray.tx.unbounded_send(TrayAction::StartPomo);
                        }),
                        ..Default::default()
                    }
                    .into(),
                    StandardItem {
                        label: self.labels.stopwatch.into(),
                        activate: Box::new(|tray: &mut Self| {
                            let _ = tray.tx.unbounded_send(TrayAction::StartStopwatch);
                        }),
                        ..Default::default()
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: self.labels.always_on_top.into(),
                checked: self.always_on_top,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.unbounded_send(TrayAction::ToggleAlwaysOnTop);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: self.labels.quit.into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.unbounded_send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

enum TrayUpdate {
    SetTooltip(String),
    SetAlwaysOnTop(bool),
    UpdateLabels(TrayLabels),
}

struct Shared {
    sender: Option<UnboundedSender<TrayAction>>,
    handle: Option<Handle<FernTray>>,
    update_tx: Option<Sender<TrayUpdate>>,
    x11_window: Option<u32>,
    enabled: bool,
    always_on_top: bool,
    quitting: bool,
    labels: TrayLabels,
    last_tip: String,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            sender: None,
            handle: None,
            update_tx: None,
            x11_window: None,
            enabled: false,
            always_on_top: false,
            quitting: false,
            labels: TrayLabels::from_strings(i18n::Locale::En.strings()),
            last_tip: "Fern".to_owned(),
        }
    }
}

fn state() -> &'static Arc<Mutex<Shared>> {
    static STATE: OnceLock<Arc<Mutex<Shared>>> = OnceLock::new();
    STATE.get_or_init(|| Arc::new(Mutex::new(Shared::default())))
}

fn decode_tray_icon() -> Vec<Icon> {
    let Ok(pixmap) = resvg::tiny_skia::Pixmap::decode_png(TRAY_ICON_PNG) else {
        return Vec::new();
    };
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for px in pixmap.pixels() {
        let color = px.demultiply();
        // StatusNotifierItem expects ARGB32 in network byte order (A, R, G, B).
        data.push(color.alpha());
        data.push(color.red());
        data.push(color.green());
        data.push(color.blue());
    }
    vec![Icon { width, height, data }]
}

pub(super) fn prepare_channel() -> UnboundedReceiver<TrayAction> {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    state().lock().unwrap_or_else(|e| e.into_inner()).sender = Some(tx);
    rx
}

pub(super) fn init(window: &Window, prefs: TrayPrefs, labels: TrayLabels) {
    let x11_window = desktop_common::sticky_windows::x11_window_id(window);
    if prefs.always_on_top
        && let Some(id) = x11_window
    {
        desktop_common::sticky_windows::x11_set_topmost(id, true);
    }

    let tx = {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        guard.x11_window = x11_window;
        guard.enabled = prefs.minimize_to_tray;
        guard.always_on_top = prefs.always_on_top;
        guard.labels = labels;
        guard.quitting = false;
        guard.last_tip = "Fern".to_owned();
        guard.sender.clone()
    };
    let Some(tx) = tx else { return };

    // Spawn D-Bus StatusNotifierItem off the foreground UI thread so startup never stalls
    // if no StatusNotifierWatcher or session bus is running.
    let shared = Arc::clone(state());
    let _ = std::thread::Builder::new()
        .name("fern-tray".into())
        .spawn(move || {
            let tray = FernTray {
                tx,
                labels,
                always_on_top: prefs.always_on_top,
                tooltip: "Fern".to_owned(),
                icon: decode_tray_icon(),
            };
            match tray.spawn() {
                Ok(handle) => {
                    let (update_tx, update_rx) = mpsc::channel::<TrayUpdate>();
                    let initial = {
                        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if guard.quitting {
                            None
                        } else {
                            guard.handle = Some(handle.clone());
                            guard.update_tx = Some(update_tx);
                            Some((guard.labels, guard.always_on_top, guard.last_tip.clone()))
                        }
                    };
                    let Some((latest_labels, latest_topmost, latest_tip)) = initial else {
                        handle.shutdown();
                        return;
                    };
                    if handle
                        .update(move |t| {
                            t.labels = latest_labels;
                            t.always_on_top = latest_topmost;
                            t.tooltip = latest_tip;
                        })
                        .is_some()
                    {
                        // Single persistent worker loop applying D-Bus tray updates strictly in
                        // order and coalescing consecutive tooltip ticks so slow D-Bus never piles up.
                        let mut next_msg: Option<TrayUpdate> = None;
                        'worker: loop {
                            let first = match next_msg.take() {
                                Some(msg) => msg,
                                None => match update_rx.recv() {
                                    Ok(msg) => msg,
                                    Err(_) => break,
                                },
                            };
                            if handle.is_closed() {
                                break;
                            }
                            let update = match first {
                                TrayUpdate::SetTooltip(mut tip) => {
                                    loop {
                                        match update_rx.try_recv() {
                                            Ok(TrayUpdate::SetTooltip(newer)) => tip = newer,
                                            Ok(other) => {
                                                next_msg = Some(other);
                                                break;
                                            }
                                            Err(TryRecvError::Empty) => break,
                                            Err(TryRecvError::Disconnected) => break 'worker,
                                        }
                                    }
                                    TrayUpdate::SetTooltip(tip)
                                }
                                other => other,
                            };
                            let applied = match update {
                                TrayUpdate::SetTooltip(tip) => handle.update(move |tray| tray.tooltip = tip),
                                TrayUpdate::SetAlwaysOnTop(topmost) => {
                                    handle.update(move |tray| tray.always_on_top = topmost)
                                }
                                TrayUpdate::UpdateLabels(labels) => handle.update(move |tray| tray.labels = labels),
                            };
                            if applied.is_none() {
                                break;
                            }
                        }
                    }
                    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if guard.handle.as_ref().is_some_and(|h| h.is_closed()) {
                        guard.update_tx = None;
                    }
                }
                Err(err) => {
                    eprintln!("system tray unavailable: {err}");
                }
            }
        });
}

pub(super) fn set_tooltip(tip: &str) {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    if guard.last_tip == tip {
        return;
    }
    guard.last_tip.clear();
    guard.last_tip.push_str(tip);
    if let Some(tx) = guard.update_tx.as_ref() {
        let _ = tx.send(TrayUpdate::SetTooltip(tip.to_owned()));
    }
}

pub(super) fn set_enabled(is_enabled: bool) {
    state().lock().unwrap_or_else(|e| e.into_inner()).enabled = is_enabled;
}

pub(super) fn set_always_on_top(topmost: bool) {
    let x11_window = {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        guard.always_on_top = topmost;
        if let Some(tx) = guard.update_tx.as_ref() {
            let _ = tx.send(TrayUpdate::SetAlwaysOnTop(topmost));
        }
        guard.x11_window
    };
    if let Some(id) = x11_window {
        desktop_common::sticky_windows::x11_set_topmost(id, topmost);
    }
}

pub(super) fn update_labels(labels: TrayLabels) {
    let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
    guard.labels = labels;
    if let Some(tx) = guard.update_tx.as_ref() {
        let _ = tx.send(TrayUpdate::UpdateLabels(labels));
    }
}

pub(super) fn hide_to_tray() -> bool {
    let (quitting, has_tray, x11_window) = {
        let guard = state().lock().unwrap_or_else(|e| e.into_inner());
        let has_tray = guard.handle.as_ref().is_some_and(|h| !h.is_closed());
        (guard.quitting, has_tray, guard.x11_window)
    };
    if quitting || !has_tray {
        return false;
    }
    // Only hide to tray when we have a way to unmap/remap the window (X11); on pure Wayland
    // callers fall back to normal minimize/close so the window is never orphaned.
    x11_window.is_some_and(desktop_common::sticky_windows::x11_hide_window)
}

pub(super) fn hide_to_tray_if_enabled() -> bool {
    let enabled = state().lock().unwrap_or_else(|e| e.into_inner()).enabled;
    enabled && hide_to_tray()
}

pub(super) fn restore_main_window() -> bool {
    let x11_window = state().lock().unwrap_or_else(|e| e.into_inner()).x11_window;
    x11_window.is_some_and(desktop_common::sticky_windows::x11_show_window)
}

pub(super) fn prepare_quit() {
    let handle = {
        let mut guard = state().lock().unwrap_or_else(|e| e.into_inner());
        guard.quitting = true;
        guard.sender = None;
        guard.update_tx = None;
        guard.handle.take()
    };
    if let Some(handle) = handle {
        handle.shutdown();
    }
}
