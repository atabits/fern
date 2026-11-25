//! The original's "Open as Sticky Note": a task in a small window of its own that stays over the
//! other windows, edited there as in the details. Each window shows the others' changes through
//! [`DataVersion`].

use std::rc::Rc;

use fern_core::{Id, Store};
use gpui::{App, Bounds, Entity, FocusHandle, FontWeight, Global, MouseButton, Pixels, TitlebarOptions, WindowBounds, WindowControlArea, WindowDecorations, WindowHandle, WindowKind, WindowOptions, point, size};
use ui::prelude::*;

use crate::actions::{
    AlignLeft, AlignRight, AlignTop, DismissTransient, PrintDetail, ToggleCollapse, ToggleColor,
    TogglePin,
};
use crate::i18n;
use crate::sticky_settings::{PALETTE, StickySettings};
use crate::task_detail::{OpenTask, TaskDetail, TaskTextEdited, TaskTextField, TaskUpdated};
use crate::sticky_windows::{self, COMPACT_HEIGHT, MIN_EXPANDED_HEIGHT, MIN_WIDTH, NoteWindow};

mod popup;
#[cfg(windows)]
mod sizing;
#[cfg(windows)]
mod taskbar;

#[derive(Default)]
struct OpenNotes(Vec<WindowHandle<StickyNote>>);
impl Global for OpenNotes {}

pub fn restore(store: Rc<Store>, cx: &mut App) {
    for note in sticky_windows::load(&store).into_iter().filter(|n| n.open) {
        if store.task(note.task).ok().flatten().is_some_and(|task| !task.is_deleted()) {
            open_note(store.clone(), note.task, false, cx);
        } else if let Err(err) = sticky_windows::close(&store, note.task) {
            desktop_common::logging::log_error(
                "sticky_ui",
                format!("failed to forget deleted note: {err:#}"),
            );
        }
    }
}

pub use desktop_common::data_version::{DataVersion, changed_elsewhere, data_changed};

/// The note's size when it opens.
const WIDTH: f32 = 240.;
const HEIGHT: f32 = 212.;
const BOUNDS_SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

fn display_title(text: &str, cx: &App) -> SharedString {
    let text = text.trim();
    if text.is_empty() {
        return i18n::t(cx).untitled.into();
    }
    text.replace("\r\n", " ").replace(['\r', '\n'], " ").into()
}

fn has_open_note(id: Id, cx: &App) -> bool {
    cx.try_global::<OpenNotes>().is_some_and(|notes| {
        notes.0.iter().any(|handle| {
            handle.read_with(cx, |note, cx| note.detail.read(cx).task_id() == Some(id)).unwrap_or(false)
        })
    })
}

fn activate_existing(id: Id, cx: &mut App) -> bool {
    let handles = cx.default_global::<OpenNotes>().0.clone();
    handles.into_iter().any(|handle| {
        handle.update(cx, |note, window, cx| {
            let matches = note.detail.read(cx).task_id() == Some(id);
            if matches {
                #[cfg(windows)]
                {
                    if let Some(button) = &mut note.taskbar {
                        button.set_hidden(note.settings.hide_from_taskbar, cx);
                    }
                    if let Some(hwnd) = win32_window_handle(window) {
                        taskbar::activate(hwnd, cx);
                    } else {
                        window.activate_window();
                    }
                }
                #[cfg(not(windows))]
                window.activate_window();
            }
            matches
        }).unwrap_or(false)
    })
}

/// Opens a task as a sticky note: a window of its own, drawn by the app as the main one, which
/// its pin keeps over the others.
pub fn open(store: Rc<Store>, id: Id, cx: &mut App) {
    open_note(store, id, true, cx);
}

fn open_note(store: Rc<Store>, id: Id, activate: bool, cx: &mut App) {
    let Some(task) = store.task(id).ok().flatten().filter(|task| !task.is_deleted()) else { return };
    if activate {
        if activate_existing(id, cx) {
            return;
        }
    } else if has_open_note(id, cx) {
        return;
    }
    let settings = StickySettings::current(&store, cx);
    let note = sticky_windows::load(&store).into_iter().find(|n| n.task == id).unwrap_or_else(|| {
        NoteWindow::capture(id, settings.pin_default, Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx))
    });
    let bounds = note.bounds(cx);
    // GPUI normally shows the HWND before it builds the view. Install the
    // saved taskbar policy first, then let GPUI show it at its initial placement.
    let show = !(cfg!(windows) && settings.hide_from_taskbar);
    let options = WindowOptions {
        focus: activate,
        show,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some(display_title(&task.title, cx)), appears_transparent: true, traffic_light_position: None }),
        window_decorations: Some(WindowDecorations::Client),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(MIN_WIDTH), px(COMPACT_HEIGHT))),
        app_id: Some(desktop_common::app_id()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        #[cfg(windows)]
        let scale = window.scale_factor();
        let initial_bounds = if show { window.window_bounds().get_bounds() } else { bounds };
        let view = cx.new(|cx| StickyNote::new(store, note, initial_bounds, settings, window, cx));
        if !show {
            if activate {
                window.activate_window();
            } else {
                #[cfg(windows)]
                if let Some(hwnd) = win32_window_handle(window) {
                    taskbar::show_inactive(hwnd, bounds, scale, cx);
                }
            }
        }
        view
    }) {
        Ok(handle) => {
            let live = cx.default_global::<OpenNotes>().0.clone().into_iter().filter(|h| h.read(cx).is_ok()).collect();
            cx.default_global::<OpenNotes>().0 = live;
            cx.default_global::<OpenNotes>().0.push(handle);
        }
        Err(err) => desktop_common::logging::log_error(
            "sticky_ui",
            format!("failed to open the sticky note: {err:#}"),
        ),
    }
}

pub struct StickyNote {
    store: Rc<Store>,
    detail: Entity<TaskDetail>,
    focus_handle: FocusHandle,
    /// Linux and macOS: a press on the bar turns into a window move once the mouse moves.
    should_move: bool,
    /// Kept over the other windows by its pin (Windows).
    pinned: bool,
    #[cfg(windows)]
    taskbar: Option<taskbar::TaskbarButton>,
    settings: StickySettings,
    title: SharedString,
    saved_task: Option<Id>,
    color: Option<usize>,
    palette: Option<WindowHandle<StickyPalette>>,
    detail_popup: Option<WindowHandle<popup::DetailPopup>>,
    opening_detail_popup: bool,
    compact: bool,
    expanded_height: Pixels,
    last_bounds: Bounds<Pixels>,
    pending_bounds: Option<Bounds<Pixels>>,
    save_bounds_task: Option<gpui::Task<()>>,
}

impl StickyNote {
    fn new(
        store: Rc<Store>,
        note: NoteWindow,
        initial_bounds: Bounds<Pixels>,
        settings: StickySettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(false);
            detail
        });
        cx.subscribe_in(&detail, window, |this, _, _: &TaskUpdated, window, cx| {
            data_changed(cx.entity_id(), cx);
            this.refresh_task(window, cx);
        })
        .detach();
        cx.subscribe_in(&detail, window, |this, _, edit: &TaskTextEdited, window, cx| {
            if let TaskTextField::Title(t) = &edit.field {
                let title = display_title(t, cx);
                if this.title != title {
                    window.set_window_title(&title);
                    this.title = title;
                    cx.notify();
                }
            }
        })
        .detach();
        // A subtask or parent clicked in the note opens in the note.
        cx.subscribe_in(&detail, window, |this, _, OpenTask(id), window, cx| {
            this.navigate(*id, window, cx);
        }).detach();
        cx.observe_global_in::<DataVersion>(window, |this, window, cx| {
            if changed_elsewhere(cx.entity_id(), cx) {
                this.refresh_task(window, cx);
            }
        })
        .detach();
        cx.observe_global_in::<StickySettings>(window, |this, window, cx| {
            this.settings_changed(*cx.global::<StickySettings>(), window, cx);
        }).detach();
        cx.observe_global_in::<i18n::CurrentLocale>(window, |this, window, cx| {
            this.update_title(window, cx);
        }).detach();
        cx.observe_in(&detail, window, |this, _, _, cx| {
            this.sync_detail_popup(cx);
        }).detach();
        #[cfg(windows)]
        sizing::set_compact(window, note.compact);
        set_topmost(window, note.pinned);
        set_opacity(window, settings.opacity);
        #[cfg(target_os = "linux")]
        set_skip_taskbar(window, settings.hide_from_taskbar);
        cx.observe_window_bounds(window, |this, window, cx| this.bounds_changed(window, cx)).detach();
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| this.forget(cx)).ok();
            true
        });
        let mut this = Self {
            store, detail, focus_handle: cx.focus_handle(), should_move: false, pinned: note.pinned, settings, title: SharedString::default(),
            saved_task: None, color: note.color.filter(|color| *color < PALETTE.len()),
            #[cfg(windows)]
            taskbar: win32_window_handle(window).map(|hwnd| taskbar::TaskbarButton::new(hwnd, settings.hide_from_taskbar, cx)),
            palette: None, detail_popup: None, opening_detail_popup: false,
            compact: note.compact,
            expanded_height: if note.compact { note.expanded_height() } else { initial_bounds.size.height.max(px(MIN_EXPANDED_HEIGHT)) },
            last_bounds: initial_bounds,
            pending_bounds: None,
            save_bounds_task: None,
        };
        this.show(note.task, window, cx);
        this.remember_bounds(initial_bounds, cx);
        this
    }

    fn bounds_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = window.bounds();
        // Windows also emits this notification when an owned popup changes the z-order.
        // Only an actual move or resize invalidates its anchor.
        if self.last_bounds == bounds { return; }
        self.last_bounds = bounds;
        self.dismiss_transients(window, cx);
        let viewport = window.viewport_size();
        if !self.compact {
            // Windows enforces this before resizing, through WM_GETMINMAXINFO.
            // The other backends do not expose a mutable native minimum yet.
            #[cfg(not(windows))]
            if viewport.height < px(MIN_EXPANDED_HEIGHT) {
                window.resize(size(viewport.width, self.expanded_height));
                return;
            }
            self.expanded_height = viewport.height.max(px(MIN_EXPANDED_HEIGHT));
        }
        self.pending_bounds = Some(window.window_bounds().get_bounds());
        self.save_bounds_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BOUNDS_SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                this.save_bounds_task = None;
                if let Some(bounds) = this.pending_bounds.take() {
                    this.remember_bounds(bounds, cx);
                }
            });
        }));
    }

    fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_bounds_task.take();
        self.pinned = !self.pinned;
        set_topmost(window, self.pinned);
        self.remember(window, cx);
        cx.notify();
    }

    fn align_edge(&mut self, edge: ScreenEdge, window: &mut Window, cx: &mut Context<Self>) {
        align_to_edge(window, edge);
        self.save_bounds_task.take();
        self.remember(window, cx);
    }

    /// Marks the note closed in local state and removes its native window.
    pub fn close_and_forget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.forget(cx);
        window.remove_window();
    }

    fn toggle_compact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compact = !self.compact;
        self.should_move = false;
        self.dismiss_transients(window, cx);
        #[cfg(windows)]
        sizing::set_compact(window, self.compact);
        let height = if self.compact { px(COMPACT_HEIGHT) } else { self.expanded_height };
        window.resize(size(window.viewport_size().width, height));
        self.save_bounds_task.take();
        self.remember(window, cx);
        cx.notify();
    }

    fn close_palette(&mut self, cx: &mut App) -> bool {
        self.palette.take().is_some_and(|palette| {
            palette.update(cx, |_, window, _| window.remove_window()).is_ok()
        })
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_palette(cx) {
            return;
        }
        self.dismiss_transients(window, cx);
        let Some(note) = window.window_handle().downcast::<Self>() else { return };
        #[cfg(windows)]
        let owner = win32_window_handle(window);
        let note_bounds = window.bounds();
        let below_bar = if self.compact { note_bounds.bottom() } else { note_bounds.top() + px(COMPACT_HEIGHT) };
        let popup_size = size(px(288.), px(102.));
        let anchor = popup::Anchor {
            point: point(note_bounds.right(), below_bar),
            above: false, right: true, screen: popup::screen(window, cx),
        };
        match popup::open(window, anchor, popup_size, true, |_, window, cx| {
            window.set_window_title(i18n::t(cx).color);
            #[cfg(windows)]
            popup::attach(owner, window, false);
            cx.new(|cx| StickyPalette::new(note, self.style().color, self.pinned, window, cx))
        }, cx) {
            Ok(palette) => self.palette = Some(palette),
            Err(err) => desktop_common::logging::log_error(
                "sticky_ui",
                format!("failed to open sticky note colors: {err:#}"),
            ),
        }
    }

    fn dismiss_transients(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(cx);
        self.opening_detail_popup = false;
        if let Some(popup) = self.detail_popup.take() {
            popup.update(cx, |_, window, _| window.remove_window()).ok();
        }
        self.detail.update(cx, |detail, cx| { detail.dismiss_popup(window, cx); });
    }

    fn sync_detail_popup(&mut self, cx: &mut Context<Self>) {
        if !self.detail.read(cx).has_popup()
            || self.opening_detail_popup
            || self.detail_popup.is_some_and(|popup| popup.read(cx).is_ok()) { return; }
        // Measuring elements uses GPUI's layout state and must happen during render.
        cx.notify();
    }

    fn popup_anchor(&self, window: &Window, cx: &App) -> popup::Anchor {
        let bounds = window.bounds();
        popup::Anchor {
            point: point(bounds.right() - px(4.), bounds.bottom() - px(27.)),
            above: true, right: true, screen: popup::screen(window, cx),
        }
    }

    fn open_detail_popup(&mut self, anchor: popup::Anchor, measured: gpui::Size<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.opening_detail_popup = false;
        if !self.detail.read(cx).has_popup()
            || self.detail_popup.is_some_and(|popup| popup.read(cx).is_ok()) { return; }
        self.close_palette(cx);
        let Some(note) = window.window_handle().downcast::<Self>() else { return };
        let return_focus = Some(self.detail.read(cx).sticky_popup_return_focus(window, cx));
        let detail = self.detail.clone();
        #[cfg(windows)]
        let owner = win32_window_handle(window);
        match popup::open(window, anchor, measured, true, |anchored, window, cx| {
            window.set_window_title(i18n::t(cx).more);
            #[cfg(windows)]
            popup::attach(owner, window, false);
            cx.new(|cx| popup::DetailPopup::new(note, detail.clone(), anchor, anchored, return_focus.clone(), window, cx))
        }, cx) {
            Ok(popup) => self.detail_popup = Some(popup),
            Err(err) => {
                desktop_common::logging::log_error(
                    "sticky_ui",
                    format!("failed to open sticky task menu: {err:#}"),
                );
                self.detail.update(cx, |detail, cx| detail.dismiss_popup(window, cx));
            }
        }
    }

    fn remember(&mut self, window: &Window, cx: &App) {
        self.save_bounds_task.take();
        self.pending_bounds = None;
        self.remember_bounds(window.window_bounds().get_bounds(), cx);
    }

    fn remember_bounds(&mut self, bounds: Bounds<Pixels>, cx: &App) {
        if let Some(id) = self.detail.read(cx).task_id() {
            let mut note = NoteWindow::capture(id, self.pinned, bounds);
            note.color = self.color;
            note.compact = self.compact;
            note.height = self.expanded_height.into();
            match sticky_windows::save(&self.store, note, self.saved_task) {
                Ok(()) => self.saved_task = Some(id),
                Err(err) => desktop_common::logging::log_error(
                    "sticky_ui",
                    format!("failed to save sticky window: {err:#}"),
                ),
            }
        }
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.save_bounds_task.take();
        if let Some(bounds) = self.pending_bounds.take() {
            self.remember_bounds(bounds, cx);
        }
        self.close_palette(cx);
        self.opening_detail_popup = false;
        if let Some(popup) = self.detail_popup.take() {
            popup.update(cx, |_, window, _| window.remove_window()).ok();
        }
        if let Some(id) = self.detail.read(cx).task_id().or(self.saved_task)
            && let Err(err) = sticky_windows::close(&self.store, id) {
            desktop_common::logging::log_error(
                "sticky_ui",
                format!("failed to close saved note: {err:#}"),
            );
        }
    }

    fn navigate(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.detail.read(cx).task_id() == Some(id) || activate_existing(id, cx) {
            return;
        }
        self.show(id, window, cx);
        self.remember(window, cx);
    }

    fn show(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.saved_task.is_some_and(|previous| previous != id) {
            self.color = sticky_windows::load(&self.store).into_iter().find(|n| n.task == id)
                .and_then(|note| note.color).filter(|color| *color < PALETTE.len());
        }
        let task = self.store.task(id).ok().flatten();
        let lists = self.store.lists().unwrap_or_default();
        self.detail.update(cx, |detail, cx| {
            detail.set_lists(lists);
            detail.show(task, cx);
        });
        self.update_style(cx);
        self.update_title(window, cx);
    }

    fn style(&self) -> StickySettings {
        StickySettings { color: self.color.unwrap_or(self.settings.color), ..self.settings }
    }

    fn update_style(&self, cx: &mut Context<Self>) {
        let style = self.style();
        self.detail.update(cx, |detail, cx| detail.set_sticky_style(style, cx));
        cx.notify();
    }

    fn settings_changed(&mut self, settings: StickySettings, window: &Window, cx: &mut Context<Self>) {
        let old = self.style();
        self.settings = settings;
        #[cfg(windows)]
        if let Some(button) = &mut self.taskbar {
            button.set_hidden(settings.hide_from_taskbar, cx);
        }
        #[cfg(target_os = "linux")]
        if old.hide_from_taskbar != settings.hide_from_taskbar {
            set_skip_taskbar(window, settings.hide_from_taskbar);
        }
        if old.opacity != settings.opacity {
            set_opacity(window, settings.opacity);
        }
        if old.color != self.style().color || old.font != settings.font {
            self.update_style(cx);
        }
    }

    fn update_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = display_title(self.detail.read(cx).task_title().unwrap_or_default(), cx);
        if self.title != title {
            window.set_window_title(&title);
            self.title = title;
            cx.notify();
        }
    }

    fn refresh_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.detail.read(cx).task_id() else { return };
        let task = self.store.task(id).ok().flatten().filter(|task| !task.is_deleted());
        self.detail.update(cx, |detail, cx| {
            detail.refresh(task, cx);
        });
        self.update_title(window, cx);
    }
}

impl Render for StickyNote {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.detail.read(cx).has_popup()
            && !self.opening_detail_popup
            && !self.detail_popup.is_some_and(|popup| popup.read(cx).is_ok())
        {
            self.opening_detail_popup = true;
            let anchor = self.popup_anchor(window, cx);
            let measured = popup::measure_detail(&self.detail, anchor, window, cx);
            cx.defer_in(window, move |this, window, cx| this.open_detail_popup(anchor, measured, window, cx));
        }
        let strings = i18n::t(cx);
        let style = self.style();
        let foreground: gpui::Hsla = gpui::rgb(style.foreground()).into();
        let (background, titlebar) = PALETTE[style.color];
        let hover_control = |el: gpui::Div| {
            el.when(!crate::touch::active(), |el| {
                el.opacity(0.).group_hover("sticky-window", |style| style.opacity(1.))
            })
        };
        let root = v_flex()
            .id("sticky-window")
            .group("sticky-window")
            .track_focus(&self.focus_handle)
            .key_context("StickyNote")
            .on_action(cx.listener(|this, _: &desktop_common::actions::CloseWindow, window, cx| {
                this.forget(cx);
                window.defer(cx, |window, _| window.remove_window());
            }))
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                this.dismiss_transients(window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePin, window, cx| this.toggle_pin(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleColor, window, cx| this.toggle_palette(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleCollapse, window, cx| this.toggle_compact(window, cx)))
            .on_action(cx.listener(|this, _: &PrintDetail, _, cx| {
                this.detail.update(cx, |detail, cx| detail.print_current(cx));
            }))
            .on_action(cx.listener(|this, _: &AlignTop, window, cx| {
                this.align_edge(ScreenEdge::Top, window, cx);
            }))
            .on_action(cx.listener(|this, _: &AlignLeft, window, cx| {
                this.align_edge(ScreenEdge::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &AlignRight, window, cx| {
                this.align_edge(ScreenEdge::Right, window, cx);
            }))
            .size_full()
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                if this.detail_popup.is_some() {
                    this.dismiss_transients(window, cx);
                }
            }))
            .bg(gpui::rgb(background))
            .text_color(foreground)
            // A strip to move the note by, with its close button.
            .child(
                h_flex()
                    .flex_none()
                    .h_full()
                    .pr(px(5.))
                    .when(!self.compact, |bar| bar.h(px(20.)).bg(gpui::rgb(titlebar)))
                    .child(
                        // As the title bar: Windows moves the window from the tagged area, the
                        // others once the pressed mouse moves. The close button stays out of it.
                        div()
                            .id("sticky-drag")
                            .debug_selector(|| "sticky-title-bar".into())
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                                this.close_palette(cx);
                                // GPUI keeps counting across consecutive double-clicks.
                                if event.click_count >= 2 && event.click_count.is_multiple_of(2) {
                                    cx.stop_propagation();
                                    this.toggle_compact(window, cx);
                                } else if !cfg!(windows) {
                                    this.should_move = true;
                                }
                            }))
                            .when(!cfg!(windows), |bar| {
                                bar
                                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
                                    .on_mouse_move(cx.listener(|this, _, window, _| {
                                        if this.should_move {
                                            this.should_move = false;
                                            window.start_window_move();
                                        }
                                    }))
                            })
                            .when(self.compact, |bar| {
                                bar.flex()
                                    .items_center()
                                    .px(px(12.))
                                    .tooltip(popup::tooltip(self.title.clone()))
                                    .child(
                                        div()
                                            .debug_selector(|| "sticky-compact-title".into())
                                            .w_full()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(px(14.))
                                            .line_height(px(20.))
                                            .truncate()
                                            .child(self.title.clone()),
                                    )
                            }),
                    )
                    .child(
                        h_flex()
                        .debug_selector(|| "sticky-title-controls".into())
                        .flex_none()
                        .child(
                            hover_control(div()).child(
                                IconButton::new("sticky-color", IconName::PaintRoller)
                                    .size(px(20.)).icon_size(IconSize::Small).icon_color(Color::Custom(foreground))
                                    .hover_color(foreground.opacity(0.12))
                                    .tooltip(popup::tooltip(strings.color))
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_palette(window, cx))),
                            ),
                        )
                        .when(!desktop_common::is_wayland_session(), |controls| {
                            controls.child(
                                div()
                                    .when(self.pinned, |el| {
                                        el.opacity(0.85).group_hover("sticky-window", |style| style.opacity(1.))
                                    })
                                    .when(!self.pinned && !crate::touch::active(), |el| {
                                        el.opacity(0.).group_hover("sticky-window", |style| style.opacity(1.))
                                    })
                                    .child(
                                        IconButton::new("sticky-pin", if self.pinned { IconName::PinOff } else { IconName::Pin })
                                            .size(px(20.))
                                            .icon_size(IconSize::Small)
                                            .icon_color(Color::Custom(foreground))
                                            .hover_color(foreground.opacity(0.12))
                                            .tooltip(popup::tooltip(if self.pinned { strings.unpin } else { strings.pin }))
                                            .on_click(cx.listener(|this, _, window, cx| this.toggle_pin(window, cx))),
                                    ),
                            )
                        })
                        .child(
                            hover_control(div()).child(
                                IconButton::new("sticky-compact", if self.compact { IconName::ChevronDown } else { IconName::ChevronUp })
                                    .size(px(20.)).icon_size(IconSize::Small).icon_color(Color::Custom(foreground))
                                    .hover_color(foreground.opacity(0.12))
                                    .tooltip(popup::tooltip(if self.compact { strings.habit_expand } else { strings.habit_collapse }))
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_compact(window, cx))),
                            ),
                        )
                        .child(
                            hover_control(div()).child(
                                IconButton::new("sticky-close", IconName::X)
                                    .icon_color(Color::Custom(foreground))
                                    .size(px(20.))
                                    .icon_size(IconSize::Small)
                                    .hover_color(foreground.opacity(0.12))
                                    .tooltip(popup::tooltip(strings.close))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.forget(cx);
                                        window.remove_window();
                                    })),
                            ),
                        ),
                    ),
            )
            .when(!self.compact, |note| {
                note.child(
                    div().flex_1().min_h_0()
                        .capture_any_mouse_down(cx.listener(|this, _, _, cx| { this.close_palette(cx); }))
                        .child(self.detail.clone()),
                )
            });
        desktop_common::layout::linux_window_frame(root, px(8.), window, cx)
    }
}

struct StickyPalette {
    note: WindowHandle<StickyNote>,
    selected: usize,
    focus: FocusHandle,
}

impl StickyPalette {
    fn new(note: WindowHandle<StickyNote>, selected: usize, pinned: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        set_topmost(window, pinned);
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active()
                && !this.note.update(cx, |_, owner, _| owner.is_window_active()).unwrap_or(false)
            {
                window.remove_window();
            }
        }).detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { note, selected, focus }
    }
}

impl Render for StickyPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .id("sticky-palette")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DismissTransient, window, cx| {
                this.note.update(cx, |note, owner, _| {
                    note.palette = None;
                    owner.activate_window();
                }).ok();
                window.remove_window();
            }))
            .size_full()
            .rounded(px(8.))
            .bg(theme.colors().elevated_surface_background)
            .border_1()
            .border_color(theme.colors().border)
            .justify_center()
            .items_center()
            .gap_2()
            .children(PALETTE.chunks(6).enumerate().map(|(row, colors)| {
                h_flex().gap_2().children(colors.iter().enumerate().map(|(column, (color, _))| {
                    let index = row * 6 + column;
                    let selected = self.selected == index;
                    let check_color = theme::foreground_on(&[gpui::rgb(*color).into()], 3.);
                    div()
                        .id(("sticky-palette-color", index))
                        .debug_selector(move || format!("sticky-palette-color-{index}"))
                        .size(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .bg(gpui::rgb(*color))
                        .border_2()
                        .border_color(if selected { theme.colors().accent } else { theme.colors().border })
                        .when(!selected, |el| el.hover(|s| s.border_color(theme.colors().text_muted)))
                        .when(selected, |el| {
                            el.child(
                                div()
                                    .debug_selector(|| "sticky-palette-check".into())
                                    .child(Icon::new(IconName::Check).size(IconSize::Medium).color(check_color)),
                            )
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.note.update(cx, |note, window, cx| {
                                note.color = Some(index);
                                note.palette = None;
                                note.update_style(cx);
                                note.remember(window, cx);
                            }).ok();
                            window.remove_window();
                        }))
                }))
            }))
    }
}

#[cfg(target_os = "linux")]
use desktop_common::sticky_windows::set_skip_taskbar;
use desktop_common::sticky_windows::{ScreenEdge, align_to_edge, set_opacity, set_topmost};
#[cfg(windows)]
pub(crate) use desktop_common::win32::hwnd_from_window as win32_window_handle;

#[cfg(test)]
mod tests;
