//! The original's "Open as Sticky Note": a task in a small window of its own that stays over the
//! other windows, edited there as in the details. Each window shows the others' changes through
//! [`DataVersion`].

use std::rc::Rc;

use fern_core::{Id, Store};
use gpui::{App, Bounds, Entity, EntityId, Global, MouseButton, TitlebarOptions, WindowBounds, WindowControlArea, WindowDecorations, WindowHandle, WindowOptions, size};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment};

use crate::i18n;
use crate::sticky_settings::{PALETTE, StickySettings};
use crate::task_detail::{OpenTask, TaskDetail, TaskUpdated};
use crate::sticky_windows::{self, NoteWindow};

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
const WIDTH: f32 = 320.;
const HEIGHT: f32 = 360.;

fn activate_existing(id: Id, cx: &mut App) -> bool {
    let handles = cx.default_global::<OpenNotes>().0.clone();
    handles.into_iter().any(|handle| {
        handle.update(cx, |note, window, cx| {
            let matches = note.detail.read(cx).task_id() == Some(id);
            if matches { window.activate_window(); }
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
    let saved = sticky_windows::load(&store).into_iter().find(|n| n.task == id);
    let title = task.title;
    let bounds = saved.as_ref().map(|note| note.bounds(cx))
        .unwrap_or_else(|| Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx));
    let pinned = saved.as_ref().map(|note| note.pinned).unwrap_or_else(|| StickySettings::load(&store).pin_default);
    let color = saved.and_then(|note| note.color).filter(|color| *color < PALETTE.len());
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some(title.into()), appears_transparent: true, traffic_light_position: None }),
        window_decorations: Some(WindowDecorations::Client),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(240.), px(200.))),
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        cx.new(|cx| StickyNote::new(store, id, pinned, color, window, cx))
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
    settings: StickySettings,
    saved_task: Option<Id>,
    color: Option<usize>,
    palette_open: bool,
}

impl StickyNote {
    fn new(store: Rc<Store>, id: Id, pinned: bool, color: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let detail = cx.new(|cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(false);
            detail
        });
        cx.subscribe(&detail, |this, _, _: &TaskUpdated, cx| {
            data_changed(cx.entity_id(), cx);
            this.refresh(cx);
        })
        .detach();
        // A subtask or parent clicked in the note opens in the note.
        cx.subscribe_in(&detail, window, |this, _, OpenTask(id), window, cx| {
            this.navigate(*id, window, cx);
        }).detach();
        cx.observe_global::<DataVersion>(|this, cx| {
            if changed_elsewhere(cx.entity_id(), cx) {
                this.refresh(cx);
            }
        })
        .detach();
        let settings = StickySettings::load(&store);
        #[cfg(windows)]
        set_topmost(window, pinned);
        cx.observe_window_bounds(window, |this, window, cx| this.remember(window, cx)).detach();
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| this.forget(cx)).ok();
            true
        });
        let mut this = Self { store, detail, should_move: false, pinned, settings, saved_task: None, color, palette_open: false };
        this.show(id, cx);
        this.refresh(cx);
        this.remember(window, cx);
        this
    }

    fn remember(&mut self, window: &Window, cx: &App) {
        if let Some(id) = self.detail.read(cx).task_id() {
            let mut note = NoteWindow::capture(id, self.pinned, window.window_bounds().get_bounds());
            note.color = self.color;
            match sticky_windows::save(&self.store, note, self.saved_task) {
                Ok(()) => self.saved_task = Some(id),
                Err(err) => eprintln!("failed to save sticky window: {err:#}"),
            }
        }
    }

    fn forget(&mut self, cx: &App) {
        if let Some(id) = self.detail.read(cx).task_id().or(self.saved_task)
            && let Err(err) = sticky_windows::close(&self.store, id) {
            eprintln!("failed to close saved note: {err:#}");
        }
    }

    fn navigate(&mut self, id: Id, window: &Window, cx: &mut Context<Self>) {
        if self.detail.read(cx).task_id() == Some(id) || activate_existing(id, cx) {
            return;
        }
        self.show(id, cx);
        self.remember(window, cx);
    }

    fn show(&mut self, id: Id, cx: &mut Context<Self>) {
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
    }

    /// Shows the task as it is now; a task deleted elsewhere leaves the note empty.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.settings = StickySettings::load(&self.store);
        if let Some(color) = self.color { self.settings.color = color; }
        let settings = self.settings;
        self.detail.update(cx, |detail, cx| detail.set_sticky_style(settings, cx));
        let Some(id) = self.detail.read(cx).task_id() else { return };
        let task = self.store.task(id).ok().flatten().filter(|task| !task.is_deleted());
        self.detail.update(cx, |detail, cx| detail.refresh(task, cx));
    }
}

impl Render for StickyNote {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        #[cfg(windows)]
        set_opacity(window, self.settings.opacity);
        let foreground: gpui::Hsla = gpui::rgb(self.settings.foreground()).into();
        v_flex()
            .size_full()
            .bg(gpui::rgb(PALETTE[self.settings.color].0))
            .border_1()
            .border_color(theme.colors().border)
            .text_color(theme.colors().text)
            // A strip to move the note by, with its close button.
            .child(
                h_flex()
                    .flex_none()
                    .h(px(28.))
                    .pr_1()
                    .child(
                        // As the title bar: Windows moves the window from the tagged area, the
                        // others once the pressed mouse moves. The close button stays out of it.
                        div()
                            .id("sticky-drag")
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag)
                            .when(!cfg!(windows), |bar| {
                                bar.on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = true))
                                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
                                    .on_mouse_move(cx.listener(|this, _, window, _| {
                                        if this.should_move {
                                            this.should_move = false;
                                            window.start_window_move();
                                        }
                                    }))
                            }),
                    )
                    // The pin keeps the note over the other windows, off at first as the original's.
                    .when(cfg!(windows), |bar| {
                        bar.child(
                            IconButton::new("sticky-pin", if self.pinned { IconName::PinOff } else { IconName::Pin })
                                .size(px(22.))
                                .icon_size(IconSize::Small)
                                .icon_color(Color::Custom(foreground))
                                .tooltip(Tooltip::text(if self.pinned { i18n::t(cx).unpin } else { i18n::t(cx).pin }))
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.pinned = !this.pinned;
                                    #[cfg(windows)]
                                    set_topmost(_window, this.pinned);
                                    this.remember(_window, cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div().relative().child(IconButton::new("sticky-color", IconName::PaintRoller)
                            .size(px(22.)).icon_size(IconSize::Small).icon_color(Color::Custom(foreground))
                            .tooltip(Tooltip::text(crate::sticky_settings::labels(i18n::t(cx).inbox == "Входящие")[1]))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.palette_open = !this.palette_open;
                                cx.notify();
                            })))
                            .when(self.palette_open, |el| el.child(PopoverAttachment::new(Placement::BelowRight,
                                h_flex().w(px(180.)).p_2().gap_2().flex_wrap()
                                    .bg(theme.colors().elevated_surface_background).rounded(px(8.))
                                    .border_1().border_color(theme.colors().border)
                                    .children(PALETTE.iter().enumerate().map(|(index, (color, _))| {
                                        div().id(("note-palette", index)).size(px(28.)).rounded_full()
                                            .bg(gpui::rgb(*color)).border_2()
                                            .border_color(if self.settings.color == index { theme.colors().accent } else { theme.colors().border })
                                            .cursor_pointer().on_click(cx.listener(move |this, _, window, cx| {
                                                this.color = Some(index);
                                                this.palette_open = false;
                                                this.refresh(cx);
                                                this.remember(window, cx);
                                                cx.notify();
                                            }))
                                    }))
                            ))),
                    )
                    .child(
                        IconButton::new("sticky-close", IconName::X)
                            .icon_color(Color::Custom(foreground))
                            .size(px(22.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(i18n::t(cx).close))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.forget(cx);
                                window.remove_window();
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.detail.clone()))
    }
}

/// Whole-window alpha, including the native surface, rather than a translucent fill.
#[cfg(windows)]
fn set_opacity(window: &Window, opacity: u8) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::*;
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let hwnd = HWND(handle.hwnd.get() as _);
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
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos};

    // The trait's method: `Window` has an inherent `window_handle` too.
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    let hwnd = HWND(handle.hwnd.get() as _);
    if let Err(err) = unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) } {
        eprintln!("failed to pin the sticky note: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext};

    #[gpui::test]
    #[cfg(debug_assertions)]
    fn due_popup_keeps_the_confirm_button_inside_a_small_note(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let task = store.add_task("Small note").unwrap();
        cx.update(|cx| {
            theme::init(cx);
            cx.set_global(i18n::CurrentLocale(i18n::Locale::En));
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            StickyNote::new(store.clone(), task.id, false, None, window, cx)
        });
        let viewport = size(px(240.), px(200.));
        cx.simulate_resize(viewport);
        cx.run_until_parked();
        let date = cx.debug_bounds("task-detail-due-date").unwrap().center();
        let modifiers = Modifiers::default();
        cx.simulate_mouse_move(date, None, modifiers);
        cx.simulate_click(date, modifiers);
        let confirm = cx.debug_bounds("due-popup-ok").expect("date picker is open");
        assert!(confirm.top() >= px(0.) && confirm.bottom() <= viewport.height);
        assert!(confirm.left() >= px(0.) && confirm.right() <= viewport.width);
        cx.simulate_mouse_move(confirm.center(), None, modifiers);
        cx.simulate_click(confirm.center(), modifiers);
        assert!(cx.debug_bounds("due-popup-ok").is_none(), "confirm closes the date picker");
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
                assert_eq!(note.settings.color, 10);
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
