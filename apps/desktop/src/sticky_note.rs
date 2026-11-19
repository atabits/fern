//! The original's "Open as Sticky Note": a task in a small window of its own that stays over the
//! other windows, edited there as in the details. Each window shows the others' changes through
//! [`DataVersion`].

use std::rc::Rc;

use fern_core::{Id, Store};
use gpui::{App, Bounds, Entity, EntityId, FocusHandle, FontWeight, Global, MouseButton, Pixels, TitlebarOptions, WindowBounds, WindowControlArea, WindowDecorations, WindowHandle, WindowKind, WindowOptions, point, size};
use ui::prelude::*;

use crate::actions::DismissTransient;
use crate::i18n;
use crate::sticky_settings::{PALETTE, StickySettings};
use crate::task_detail::{OpenTask, TaskDetail, TaskUpdated};
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
            open(store.clone(), note.task, cx);
        } else if let Err(err) = sticky_windows::close(&store, note.task) {
            eprintln!("failed to forget deleted note: {err:#}");
        }
    }
}

/// Counts the changes made in any window, so that the other windows show them; `by` is the view
/// that made the last one, which has it already.
#[derive(Default)]
pub struct DataVersion {
    version: u64,
    by: Option<EntityId>,
}

impl Global for DataVersion {}

/// Tells the other windows that `by` changed the data.
pub fn data_changed(by: EntityId, cx: &mut App) {
    let data = cx.default_global::<DataVersion>();
    data.version += 1;
    data.by = Some(by);
}

/// Whether the last change was made by a view other than `observer`.
pub fn changed_elsewhere(observer: EntityId, cx: &App) -> bool {
    cx.try_global::<DataVersion>().is_some_and(|data| data.by != Some(observer))
}

/// The note's size when it opens.
const WIDTH: f32 = 240.;
const HEIGHT: f32 = 212.;

fn display_title(text: &str, cx: &App) -> SharedString {
    let text = text.trim();
    if text.is_empty() {
        return i18n::t(cx).untitled.into();
    }
    text.replace("\r\n", " ").replace(['\r', '\n'], " ").into()
}

fn activate_existing(id: Id, cx: &mut App) -> bool {
    let handles = cx.default_global::<OpenNotes>().0.clone();
    handles.into_iter().any(|handle| {
        handle.update(cx, |note, window, cx| {
            let matches = note.detail.read(cx).task_id() == Some(id);
            if matches {
                #[cfg(windows)]
                if let Some(button) = &mut note.taskbar {
                    button.set_hidden(note.settings.hide_from_taskbar, cx);
                }
                window.activate_window();
            }
            matches
        }).unwrap_or(false)
    })
}

/// Opens a task as a sticky note: a window of its own, drawn by the app as the main one, which
/// its pin keeps over the others.
pub fn open(store: Rc<Store>, id: Id, cx: &mut App) {
    let Some(task) = store.task(id).ok().flatten().filter(|task| !task.is_deleted()) else { return };
    if activate_existing(id, cx) {
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
        show,
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some(display_title(&task.title, cx)), appears_transparent: true, traffic_light_position: None }),
        window_decorations: Some(WindowDecorations::Client),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(MIN_WIDTH), px(COMPACT_HEIGHT))),
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let initial_bounds = if show { window.window_bounds().get_bounds() } else { bounds };
        let view = cx.new(|cx| StickyNote::new(store, note, initial_bounds, settings, window, cx));
        if !show { window.activate_window(); }
        view
    }) {
        Ok(handle) => {
            let live = cx.default_global::<OpenNotes>().0.clone().into_iter().filter(|h| h.read(cx).is_ok()).collect();
            cx.default_global::<OpenNotes>().0 = live;
            cx.default_global::<OpenNotes>().0.push(handle);
        }
        Err(err) => eprintln!("failed to open the sticky note: {err:#}"),
    }
}

pub struct StickyNote {
    store: Rc<Store>,
    detail: Entity<TaskDetail>,
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
    compact: bool,
    expanded_height: Pixels,
    last_bounds: Bounds<Pixels>,
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
        {
            sizing::set_compact(window, note.compact);
            set_topmost(window, note.pinned);
            set_opacity(window, settings.opacity);
        }
        cx.observe_window_bounds(window, |this, window, cx| this.bounds_changed(window, cx)).detach();
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| this.forget(cx)).ok();
            true
        });
        let mut this = Self {
            store, detail, should_move: false, pinned: note.pinned, settings, title: SharedString::default(),
            saved_task: None, color: note.color.filter(|color| *color < PALETTE.len()),
            #[cfg(windows)]
            taskbar: win32_window_handle(window).map(|hwnd| taskbar::TaskbarButton::new(hwnd, settings.hide_from_taskbar, cx)),
            palette: None, detail_popup: None,
            compact: note.compact,
            expanded_height: if note.compact { note.expanded_height() } else { initial_bounds.size.height.max(px(MIN_EXPANDED_HEIGHT)) },
            last_bounds: initial_bounds,
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
        self.remember(window, cx);
    }

    fn toggle_compact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.compact = !self.compact;
        self.should_move = false;
        self.dismiss_transients(window, cx);
        #[cfg(windows)]
        sizing::set_compact(window, self.compact);
        let height = if self.compact { px(COMPACT_HEIGHT) } else { self.expanded_height };
        window.resize(size(window.viewport_size().width, height));
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
            Err(err) => eprintln!("failed to open sticky note colors: {err:#}"),
        }
    }

    fn dismiss_transients(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(cx);
        if let Some(popup) = self.detail_popup.take() {
            popup.update(cx, |_, window, _| window.remove_window()).ok();
        }
        self.detail.update(cx, |detail, cx| { detail.dismiss_popup(window, cx); });
    }

    fn sync_detail_popup(&mut self, cx: &mut Context<Self>) {
        if !self.detail.read(cx).has_popup()
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
            cx.new(|cx| popup::DetailPopup::new(note, detail, anchor, anchored, return_focus, window, cx))
        }, cx) {
            Ok(popup) => self.detail_popup = Some(popup),
            Err(err) => eprintln!("failed to open sticky task menu: {err:#}"),
        }
    }

    fn remember(&mut self, window: &Window, cx: &App) {
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
                Err(err) => eprintln!("failed to save sticky window: {err:#}"),
            }
        }
    }

    fn forget(&mut self, cx: &mut App) {
        self.close_palette(cx);
        if let Some(popup) = self.detail_popup.take() {
            popup.update(cx, |_, window, _| window.remove_window()).ok();
        }
        if let Some(id) = self.detail.read(cx).task_id().or(self.saved_task)
            && let Err(err) = sticky_windows::close(&self.store, id) {
            eprintln!("failed to close saved note: {err:#}");
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

    fn settings_changed(&mut self, settings: StickySettings, _window: &Window, cx: &mut Context<Self>) {
        let old = self.style();
        self.settings = settings;
        #[cfg(windows)]
        {
            if let Some(button) = &mut self.taskbar {
                button.set_hidden(settings.hide_from_taskbar, cx);
            }
            if old.opacity != settings.opacity {
                set_opacity(_window, settings.opacity);
            }
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
            cx.notify();
        });
        self.update_title(window, cx);
    }
}

impl Render for StickyNote {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.detail.read(cx).has_popup()
            && !self.detail_popup.is_some_and(|popup| popup.read(cx).is_ok())
        {
            let anchor = self.popup_anchor(_window, cx);
            let measured = popup::measure_detail(&self.detail, anchor, _window, cx);
            cx.defer_in(_window, move |this, window, cx| this.open_detail_popup(anchor, measured, window, cx));
        }
        let strings = i18n::t(cx);
        let style = self.style();
        let foreground: gpui::Hsla = gpui::rgb(style.foreground()).into();
        let (background, titlebar) = PALETTE[style.color];
        v_flex()
            .id("sticky-window")
            .group("sticky-window")
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
                                bar.flex().items_center().px(px(12.)).child(
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
                        .when(!crate::touch::active(), |bar| bar.opacity(0.).group_hover("sticky-window", |style| style.opacity(1.)))
                        .child(
                        IconButton::new("sticky-color", IconName::PaintRoller)
                            .size(px(20.)).icon_size(IconSize::Small).icon_color(Color::Custom(foreground))
                            .hover_color(foreground.opacity(0.12))
                            .tooltip(popup::tooltip(strings.color))
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_palette(window, cx))),
                        )
                        .when(cfg!(windows), |bar| bar.child(
                            IconButton::new("sticky-pin", if self.pinned { IconName::PinOff } else { IconName::Pin })
                                .size(px(20.))
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Custom(foreground))
                                .hover_color(foreground.opacity(0.12))
                                .tooltip(popup::tooltip(if self.pinned { strings.unpin } else { strings.pin }))
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.pinned = !this.pinned;
                                    #[cfg(windows)]
                                    set_topmost(_window, this.pinned);
                                    this.remember(_window, cx);
                                    cx.notify();
                                })),
                        ))
                        .child(
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
            )
            .when(!self.compact, |note| {
                note.child(
                    div().flex_1().min_h_0()
                        .capture_any_mouse_down(cx.listener(|this, _, _, cx| { this.close_palette(cx); }))
                        .child(self.detail.clone()),
                )
            })
    }
}

struct StickyPalette {
    note: WindowHandle<StickyNote>,
    selected: usize,
    focus: FocusHandle,
}

impl StickyPalette {
    fn new(note: WindowHandle<StickyNote>, selected: usize, _pinned: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(windows)]
        set_topmost(window, _pinned);
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
            .on_action(cx.listener(|_, _: &DismissTransient, window, _| window.remove_window()))
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
                    div()
                        .id(("sticky-palette-color", index))
                        .debug_selector(move || format!("sticky-palette-color-{index}"))
                        .size(px(36.))
                        .rounded(px(5.))
                        .bg(gpui::rgb(*color))
                        .border_2()
                        .border_color(if self.selected == index { theme.colors().accent } else { theme.colors().border })
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

/// Whole-window alpha, including the native surface, rather than a translucent fill.
#[cfg(windows)]
fn set_opacity(window: &Window, opacity: u8) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let Some(hwnd) = win32_window_handle(window) else { return };
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
        let alpha = (u16::from(opacity.clamp(10, 100)) * 255 / 100) as u8;
        if let Err(err) = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), alpha, LWA_ALPHA) {
            eprintln!("failed to set note opacity: {err}");
        }
    }
}

/// Keeps the window over the other windows, or puts it back among them: the original's pin.
#[cfg(windows)]
pub(crate) fn set_topmost(window: &Window, topmost: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos};

    let Some(hwnd) = win32_window_handle(window) else { return };
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    if let Err(err) = unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) } {
        eprintln!("failed to pin the sticky note: {err}");
    }
}

#[cfg(windows)]
fn win32_window_handle(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return None };
    Some(windows::Win32::Foundation::HWND(handle.hwnd.get() as _))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(debug_assertions)]
    use gpui::Focusable as _;
    #[cfg(debug_assertions)]
    use gpui::Modifiers;
    use gpui::TestAppContext;

    fn new_note(store: Rc<Store>, id: Id, window: &mut Window, cx: &mut Context<StickyNote>) -> StickyNote {
        let bounds = window.window_bounds().get_bounds();
        let note = NoteWindow::capture(id, false, bounds);
        let settings = StickySettings::current(&store, cx);
        StickyNote::new(store, note, bounds, settings, window, cx)
    }

    #[gpui::test]
    fn native_title_follows_renames_and_navigation_in_its_own_note(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let first = store.add_task("First").unwrap().id;
        let second = store.add_task("Second").unwrap().id;
        let child = store.add_subtask(first, "Child").unwrap().id;
        let (first_window, second_window) = cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            open(store.clone(), first, cx);
            open(store.clone(), second, cx);
            let handles = &cx.global::<OpenNotes>().0;
            (handles[0], handles[1])
        });
        let mut first_cx = gpui::VisualTestContext::from_window(first_window.into(), cx);
        let mut second_cx = gpui::VisualTestContext::from_window(second_window.into(), cx);
        assert_eq!(first_cx.window_title().as_deref(), Some("First"));

        store.set_title(first, " New\r\ntitle ").unwrap();
        first_window.update(cx, |note, _, cx| {
            note.detail.update(cx, |_, cx| cx.emit(TaskUpdated));
        }).unwrap();
        assert_eq!(first_cx.window_title().as_deref(), Some("New title"));

        store.set_title(first, "").unwrap();
        second_window.update(cx, |_, _, cx| data_changed(cx.entity_id(), cx)).unwrap();
        assert_eq!(first_cx.window_title().as_deref(), Some("Untitled"));
        cx.update(|cx| cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru)));
        assert_eq!(first_cx.window_title().as_deref(), Some(i18n::Locale::Ru.strings().untitled));

        first_window.update(cx, |note, window, cx| note.navigate(child, window, cx)).unwrap();
        assert_eq!(first_cx.window_title().as_deref(), Some("Child"));
        assert_eq!(second_cx.window_title().as_deref(), Some("Second"));
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn compact_note_reopens_at_one_line_and_restores_its_previous_height(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let title = "A long note title that needs to fit on a single line when it is collapsed";
        let task = store.add_task(title).unwrap().id;
        cx.update(theme::init);
        let (note, cx) = cx.add_window_view(|window, cx| {
            new_note(store.clone(), task, window, cx)
        });
        cx.simulate_resize(size(px(324.), px(318.)));

        let title_bar = cx.debug_bounds("sticky-title-bar").unwrap();
        let title_press = |click_count| gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: title_bar.center(),
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        };
        // Two quick double-clicks, followed by another pair after a pause.
        for (click_count, compact) in [(1, false), (2, true), (3, true), (4, false), (1, false), (2, true)] {
            cx.simulate_event(title_press(click_count));
            cx.update(|window, cx| {
                assert_eq!(note.read(cx).compact, compact, "click {click_count}");
                let height = if compact { px(COMPACT_HEIGHT) } else { px(318.) };
                assert_eq!(window.bounds().size, size(px(324.), height));
            });
        }
        assert!(cx.debug_bounds("task-detail-body").is_none());
        for height in [COMPACT_HEIGHT, COMPACT_HEIGHT + 10.] {
            cx.simulate_resize(size(px(324.), px(height)));
            for selector in ["sticky-compact-title", "sticky-title-controls"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(bounds.size.height <= px(COMPACT_HEIGHT));
                assert!((bounds.center().y - px(height / 2.)).abs() <= px(1.), "{selector}: {bounds:?}");
            }
        }
        let saved = sticky_windows::load(&store).pop().unwrap();
        assert!(saved.compact);
        assert_eq!(saved.width, 324.);
        assert_eq!(saved.height, 318.);

        let palette = cx.update(|window, cx| {
            note.update(cx, |note, cx| {
                note.toggle_palette(window, cx);
                note.palette.unwrap()
            })
        });
        let mut palette_cx = gpui::VisualTestContext::from_window(palette.into(), cx);
        let swatch = palette_cx.debug_bounds("sticky-palette-color-1").unwrap().center();
        palette_cx.simulate_click(swatch, Modifiers::default());
        cx.update(|_, cx| {
            assert!(note.read(cx).compact);
            assert_eq!(note.read(cx).color, Some(1));
        });
        assert_eq!(sticky_windows::load(&store)[0].color, Some(1));

        cx.update(|_, cx| {
            open(store.clone(), task, cx);
            let reopened = *cx.global::<OpenNotes>().0.last().unwrap();
            reopened.update(cx, |note, window, _| {
                assert!(note.compact);
                assert_eq!(window.viewport_size().height, px(COMPACT_HEIGHT));
                window.remove_window();
            }).unwrap();
        });

        cx.simulate_event(title_press(1));
        cx.simulate_event(title_press(2));
        cx.update(|_, cx| {
            assert!(!note.read(cx).compact);
            assert_eq!(note.read(cx).expanded_height, px(318.));
        });
        assert!(cx.debug_bounds("task-detail-body").is_some());
        assert!(!sticky_windows::load(&store)[0].compact);
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn small_note_opens_a_full_date_picker_and_saves_the_selected_day(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Small note").unwrap();
        let original = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        store.set_due(task.id, Some(original), None).unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (note, cx) = cx.add_window_view(|window, cx| {
            new_note(store.clone(), task.id, window, cx)
        });
        let viewport = size(px(240.), px(200.));
        cx.simulate_resize(viewport);
        cx.run_until_parked();
        let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_click(date, modifiers);
        assert!(cx.debug_bounds("due-popup-ok").is_none(), "the picker is outside the note");
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.expect("native date picker is open"));
        cx.simulate_resize(viewport); // Windows sends unchanged bounds when a child popup opens.
        cx.update(|_, cx| assert_eq!(note.read(cx).detail_popup, Some(popup)));
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        let picked = chrono::NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        let last_day = popup_cx.debug_bounds("due-day-2026-12-01").unwrap();
        let confirm = popup_cx.debug_bounds("due-popup-ok").unwrap();
        popup_cx.update(|window, _| {
            assert!(last_day.bottom() < confirm.top());
            assert!(confirm.bottom() <= window.viewport_size().height);
            assert!(window.viewport_size().height > viewport.height);
        });
        popup_cx.simulate_click(last_day.center(), modifiers);
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(original), "the picker retains a draft until OK");
        let confirm = popup_cx.debug_bounds("due-popup-ok").unwrap();
        popup_cx.simulate_click(confirm.center(), modifiers);
        assert_eq!(store.task(task.id).unwrap().unwrap().due_date, Some(picked));
        cx.update(|window, cx| {
            assert_eq!(window.viewport_size(), viewport, "opening a picker does not resize the note");
            assert!(note.read(cx).detail_popup.is_none());
        });
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn losing_popup_activation_keeps_the_note_open(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Keep this note").unwrap();
        cx.update(theme::init);
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
        let more = cx.debug_bounds("task-detail-more").unwrap().center();
        cx.simulate_click(more, Modifiers::default());
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        let popup_id = cx.update(|_, cx| popup.update(cx, |_, _, cx| cx.entity_id()).unwrap());
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        popup_cx.update(|window, _| window.activate_window());
        // The note reads its popup during rendering to check whether it still exists.
        let owner = cx.update(|window, cx| { window.refresh(); window.draw(cx).clear(cx); window.window_handle() });
        cx.cx.update(|cx| {
            assert_eq!(cx.with_window(popup_id, |window, _| window.window_handle()), Some(owner));
        });
        popup_cx.deactivate_window();
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(cx.windows().len(), 1, "only the transient menu should close");
            assert!(note.read(cx).detail_popup.is_none());
            assert_eq!(note.read(cx).detail.read(cx).task_id(), Some(task.id));
        });
        assert!(sticky_windows::load(&store).iter().any(|saved| saved.task == task.id && saved.open));
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn menu_state_observer_closes_the_popup_after_the_owner_renders_last(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Keep the owner").unwrap();
        cx.update(theme::init);
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
        let more = cx.debug_bounds("task-detail-more").unwrap().center();
        cx.simulate_click(more, Modifiers::default());
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            let detail = note.read(cx).detail.clone();
            detail.update(cx, |detail, cx| { detail.dismiss_popup(window, cx); });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(cx.windows().len(), 1);
            assert!(popup.read(cx).is_err(), "the menu window must be removed");
            assert!(note.read(cx).detail_popup.is_none());
            assert_eq!(window.root::<StickyNote>().unwrap(), Some(note.clone()));
        });
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn small_note_menu_reaches_the_last_action_and_escape_restores_focus(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Small note").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
        cx.simulate_resize(size(px(240.), px(200.)));
        let title = cx.debug_bounds("task-detail-title").unwrap().center();
        cx.simulate_click(title, Modifiers::default());
        let focused = cx.update(|window, cx| window.focused(cx).unwrap());
        let more = cx.debug_bounds("task-detail-more").unwrap().center();
        cx.simulate_click(more, Modifiers::default());
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        popup_cx.run_until_parked();
        let resized = popup_cx.update(|window, _| window.window_bounds().get_bounds().size);
        popup_cx.simulate_resize(resized); // TestWindow does not emit the platform's resize callback.
        let last = popup_cx.debug_bounds("task-menu-more-delete").unwrap();
        popup_cx.update(|window, _| assert!(last.bottom() <= window.viewport_size().height, "last={last:?}, viewport={:?}", window.viewport_size()));
        popup_cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(note.read(cx).detail_popup.is_none());
            assert!(focused.is_focused(window));
        });
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn sticky_submenus_keep_window_geometry_and_main_menu_paint_stable(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Submenu regression").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::Ru));
        });
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
        cx.simulate_resize(size(px(240.), px(200.)));
        let more = cx.debug_bounds("task-detail-more").unwrap().center();
        cx.simulate_click(more, Modifiers::default());
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        popup_cx.run_until_parked();
        let original_window = popup_cx.update(|window, _| window.window_bounds().get_bounds());
        popup_cx.simulate_resize(original_window.size);
        let original_menu = popup_cx.debug_bounds("task-more-popup").unwrap();
        assert!(popup_cx.debug_bounds("submenu-content-more-focus").is_none());
        let trigger = popup_cx.debug_bounds("submenu-more-focus").unwrap().center();
        popup_cx.simulate_mouse_move(trigger, None, Modifiers::default());
        popup_cx.run_until_parked();
        assert!(popup_cx.debug_bounds("submenu-content-more-focus").is_some());
        let expanded_menu = popup_cx.debug_bounds("task-more-popup").unwrap();
        assert_eq!(expanded_menu, original_menu);
        popup_cx.update(|window, _| {
            assert_eq!(window.window_bounds().get_bounds(), original_window, "opening a submenu must not resize or move its native window");
            assert!(expanded_menu.right() <= window.viewport_size().width, "menu={expanded_menu:?}, viewport={:?}", window.viewport_size());
            let menu = expanded_menu.scale(window.scale_factor());
            let quads = window.painted_quads();
            let surface = quads.iter().find(|quad| quad.bounds == menu).expect("main menu surface is painted");
            assert_eq!(surface.bounds.intersect(&surface.content_mask.bounds), surface.bounds, "the main menu must not be clipped after a submenu opens");
        });
        let neighbor = popup_cx.debug_bounds("task-menu-more-attachment").unwrap().center();
        popup_cx.simulate_mouse_move(neighbor, None, Modifiers::default());
        popup_cx.run_until_parked();
        assert!(popup_cx.debug_bounds("submenu-content-more-focus").is_none());
        assert_eq!(popup_cx.debug_bounds("task-more-popup").unwrap(), original_menu);
        popup_cx.update(|window, _| assert_eq!(window.window_bounds().get_bounds(), original_window, "closing a submenu must not resize or move its native window"));

        popup_cx.simulate_mouse_move(trigger, None, Modifiers::default());
        let estimate = popup_cx.debug_bounds("task-focus-estimate").unwrap().center();
        popup_cx.simulate_click(estimate, Modifiers::default());
        let (popup, anchor) = cx.update(|window, cx| {
            let note = note.read(cx);
            (note.detail_popup.unwrap(), note.popup_anchor(window, cx))
        });
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        assert!(popup_cx.debug_bounds("task-estimate-popup").is_some(), "clicking the submenu must execute its action before dismissal");
        assert!(popup_cx.debug_bounds("task-more-popup").is_none());
        popup_cx.update(|window, cx| {
            let bounds = window.bounds();
            assert_ne!(bounds.size, original_window.size);
            if cfg!(windows) {
                assert_eq!(bounds, window.pixel_snap_bounds(anchor.bounds(bounds.size)), "a different menu size must remain attached to the note");
            } else {
                // TestPlatform accepts AnchoredPopup but TestWindow::resize only
                // updates its size. The real Wayland backend reruns the positioner;
                // its compositor configure event has no equivalent in TestWindow.
                assert_eq!(bounds.origin, original_window.origin);
                assert_eq!(anchor.bounds(bounds.size).bottom(), anchor.point.y);
            }
            assert_eq!(cx.windows().len(), 2, "repositioning must retire the previous popup");
        });
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn sticky_list_sections_keep_the_native_menu_geometry_stable(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("List submenu regression").unwrap();
        let section = store.add_list_section(None, "Inbox section").unwrap();
        cx.update(theme::init);
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
        cx.simulate_resize(size(px(240.), px(200.)));
        cx.update(|_, cx| {
            note.read(cx).detail.clone().update(cx, |detail, cx| detail.dev_open_popup("list", cx));
        });
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        popup_cx.run_until_parked();
        let original_window = popup_cx.update(|window, _| window.window_bounds().get_bounds());
        popup_cx.simulate_resize(original_window.size);
        let original_menu = popup_cx.debug_bounds("task-list-popup").unwrap();
        assert!(popup_cx.debug_bounds("task-list-sections").is_none());

        let inbox = popup_cx.debug_bounds("move-to-inbox").unwrap().center();
        popup_cx.simulate_mouse_move(inbox, None, Modifiers::default());
        popup_cx.run_until_parked();
        let sections = popup_cx.debug_bounds("task-list-sections").unwrap();
        assert!(sections.right() <= original_menu.left());
        assert!(sections.left() >= px(0.));
        assert_eq!(popup_cx.debug_bounds("task-list-popup").unwrap(), original_menu);
        popup_cx.update(|window, _| assert_eq!(window.window_bounds().get_bounds(), original_window));

        popup_cx.update(|_, cx| {
            note.read(cx).detail.clone().update(cx, |detail, cx| detail.dev_open_popup("list", cx));
        });
        popup_cx.run_until_parked();
        assert!(popup_cx.debug_bounds("task-list-sections").is_none());
        assert_eq!(popup_cx.debug_bounds("task-list-popup").unwrap(), original_menu);
        popup_cx.update(|window, _| assert_eq!(window.window_bounds().get_bounds(), original_window));

        popup_cx.simulate_mouse_move(point(px(1.), px(1.)), None, Modifiers::default());
        popup_cx.simulate_mouse_move(inbox, None, Modifiers::default());
        let section_row = popup_cx.debug_bounds("task-list-sections").unwrap().center();
        popup_cx.simulate_click(section_row, Modifiers::default());
        assert_eq!(store.task(task.id).unwrap().unwrap().section_id, Some(section.id));
        cx.update(|_, cx| assert!(note.read(cx).detail_popup.is_none()));
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn clicking_note_editors_dismisses_native_popups_without_losing_the_click(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Editable title").unwrap();
        store.set_notes(task.id, "Editable notes").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
        cx.simulate_resize(size(px(400.), px(800.)));

        for editor in ["task-detail-title", "task-notes-preview", "task-notes-editor"] {
            let target = cx.debug_bounds(editor).unwrap().center();
            let more = cx.debug_bounds("task-detail-more").unwrap().center();
            cx.simulate_click(more, Modifiers::default());
            cx.update(|_, cx| assert!(note.read(cx).detail_popup.is_some()));
            cx.simulate_click(target, Modifiers::default());
            cx.update(|window, cx| {
                let note = note.read(cx);
                assert!(note.detail_popup.is_none(), "clicking {editor} must dismiss the menu");
                let focused = window.focused(cx).expect("the click still focuses the editor");
                assert!(note.detail.read(cx).is_editor_focus(&focused, cx));
            });
        }

        let titlebar = cx.debug_bounds("sticky-title-bar").unwrap();
        let color_button = point(titlebar.right() + px(10.), titlebar.center().y);
        cx.simulate_click(color_button, Modifiers::default());
        cx.update(|_, cx| assert!(note.read(cx).palette.is_some()));
        cx.simulate_click(color_button, Modifiers::default());
        cx.update(|_, cx| assert!(note.read(cx).palette.is_none(), "a second click on Color must close its palette"));
        cx.simulate_click(color_button, Modifiers::default());
        let title = cx.debug_bounds("task-detail-title").unwrap().center();
        cx.simulate_click(title, Modifiers::default());
        cx.update(|_, cx| assert!(note.read(cx).palette.is_none()));
        cx.simulate_keystrokes("enter");
        cx.simulate_input(" after dismissal");
        assert!(store.task(task.id).unwrap().unwrap().notes.ends_with(" after dismissal"));
    }

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn sticky_tag_picker_submits_in_its_own_window_and_closes_on_real_resize(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Note with tags").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            assert!(crate::keymap::load_default(&assets::Assets, cx).unwrap().is_empty());
        });
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store.clone(), task.id, window, cx));
        cx.simulate_resize(size(px(240.), px(200.)));
        cx.update(|window, cx| note.update(cx, |note, cx| {
            note.detail.update(cx, |detail, cx| detail.start("tags", window, cx));
        }));
        // Follow the currently active surface between keystrokes: backends without
        // a move API recreate it when the filtered menu's measured size changes.
        for character in "sticky tag".chars() {
            let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
            cx.dispatch_keystroke(popup.into(), gpui::Keystroke::parse(&character.to_string()).unwrap());
        }
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        popup_cx.update(|window, cx| {
            let (_, [tags, _]) = note.read(cx).detail.read(cx).sticky_popup_inputs(cx);
            assert_eq!(tags.read(cx).text(), "sticky tag", "resizing must retain the input entity and its draft");
            assert!(tags.focus_handle(cx).is_focused(window), "resizing must preserve keyboard focus");
            assert_eq!(cx.windows().len(), 2);
        });
        popup_cx.simulate_keystrokes("enter");
        assert!(store.task(task.id).unwrap().unwrap().tags.is_empty());
        let popup = cx.update(|_, cx| note.read(cx).detail_popup.unwrap());
        let mut popup_cx = gpui::VisualTestContext::from_window(popup.into(), cx);
        let confirm = popup_cx.debug_bounds("task-tags-ok").unwrap();
        popup_cx.simulate_click(confirm.center(), Modifiers::default());
        assert_eq!(store.task(task.id).unwrap().unwrap().tags, ["sticky tag"]);
        let more = cx.debug_bounds("task-detail-more").unwrap().center();
        cx.simulate_click(more, Modifiers::default());
        cx.simulate_resize(size(px(280.), px(230.)));
        cx.update(|_, cx| {
            assert!(note.read(cx).detail_popup.is_none());
            assert!(!note.read(cx).detail.read(cx).has_popup());
        });
    }

    #[gpui::test]
    fn compact_note_tooltip_has_its_own_window_and_preserves_keyboard_focus(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Compact note").unwrap();
        cx.update(theme::init);
        let (note, cx) = cx.add_window_view(|window, cx| new_note(store, task.id, window, cx));
        let lifetime = cx.update(|window, cx| {
            note.update(cx, |note, cx| note.toggle_compact(window, cx));
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            let tooltip = popup::tooltip("Color")(window, cx);
            assert!(focus.is_focused(window));
            tooltip
        });
        cx.draw(point(px(0.), px(0.)), size(px(0.), px(0.)), |_, _| lifetime.clone().into_any_element());
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(cx.windows().len(), 2));
        cx.update(|_, _| drop(lifetime));
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(cx.windows().len(), 1);
            assert!(note.read(cx).compact);
        });
    }

    #[gpui::test]
    fn navigating_to_an_open_note_keeps_each_window_associated_with_its_task(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            let store = Rc::new(Store::open_in_memory().unwrap());
            let first = store.add_task("First").unwrap().id;
            let second = store.add_task("Second").unwrap().id;
            open(store.clone(), first, cx);
            open(store.clone(), second, cx);
            let handle = cx.global::<OpenNotes>().0[0];
            handle.update(cx, |note, window, cx| {
                note.navigate(second, window, cx);
                assert_eq!(note.detail.read(cx).task_id(), Some(first));
                note.forget(cx);
                window.remove_window();
            }).unwrap();
            let notes = sticky_windows::load(&store);
            assert!(notes.iter().any(|note| note.task == second && note.open));
        });
    }

    #[gpui::test]
    fn navigating_to_a_closed_note_applies_its_saved_color(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            let store = Rc::new(Store::open_in_memory().unwrap());
            let first = store.add_task("First").unwrap().id;
            let second = store.add_task("Second").unwrap().id;
            open(store.clone(), first, cx);
            open(store.clone(), second, cx);
            let [first_handle, second_handle] = cx.global::<OpenNotes>().0[..] else { panic!("expected two notes") };
            second_handle.update(cx, |note, window, cx| {
                note.color = Some(10);
                note.remember(window, cx);
                note.forget(cx);
                window.remove_window();
            }).unwrap();
            first_handle.update(cx, |note, window, cx| {
                note.navigate(second, window, cx);
                assert_eq!(note.detail.read(cx).task_id(), Some(second));
                assert_eq!(note.style().color, 10);
            }).unwrap();
        });
    }

    #[gpui::test]
    fn reopening_restores_one_window_and_close_stays_closed(cx: &mut TestAppContext) {
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
            let store = Rc::new(Store::open_in_memory().unwrap());
            let task = store.add_task("Saved note").unwrap();
            open(store.clone(), task.id, cx);
            open(store.clone(), task.id, cx);
            assert_eq!(cx.windows().len(), 1, "opening the same task activates its existing window");
            let handle = cx.global::<OpenNotes>().0[0];
            handle.update(cx, |note, window, cx| {
                note.pinned = true;
                note.color = Some(10);
                note.remember(window, cx);
                window.remove_window(); // Simulate app shutdown without explicitly closing a note.
            }).unwrap();
            restore(store.clone(), cx);
            let handle = *cx.global::<OpenNotes>().0.last().unwrap();
            handle.update(cx, |note, window, cx| {
                assert!(note.pinned);
                assert_eq!(note.style().color, 10);
                note.forget(cx);
                window.remove_window();
            }).unwrap();
            assert!(!sticky_windows::load(&store)[0].open);
            let count = cx.windows().len();
            restore(store.clone(), cx);
            assert_eq!(cx.windows().len(), count);
            open(store.clone(), task.id, cx);
            store.delete_task(task.id).unwrap();
            for handle in cx.global::<OpenNotes>().0.clone() {
                handle.update(cx, |_, window, _| window.remove_window()).ok();
            }
            restore(store.clone(), cx);
            assert!(!sticky_windows::load(&store)[0].open, "deleted tasks must not reopen");
        });
    }
}
