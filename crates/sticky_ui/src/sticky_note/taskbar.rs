//! Taskbar buttons for independent sticky-note windows on Windows.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Bounds, Pixels, Task};
use windows::Win32::Foundation::{E_FAIL, ERROR_SUCCESS, GetLastError, HWND, RECT, SetLastError};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GW_OWNER, GWL_EXSTYLE, GWLP_HWNDPARENT, GetClientRect,
    GetWindow, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, IsIconic, IsWindow,
    IsWindowVisible, SET_WINDOW_POS_FLAGS, SW_RESTORE, SW_SHOWMINNOACTIVE, SW_SHOWNA,
    SW_SHOWNOACTIVATE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSENDCHANGING, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_LONG_PTR_INDEX, WINDOWPLACEMENT, WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{Error, PCWSTR, Result, w};

pub(super) struct TaskbarButton {
    update: Option<Task<()>>,
    window: Rc<NoteHandle>,
}

struct NoteHandle {
    note: HWND,
    owner: Cell<Option<HWND>>,
    applied_hidden: Cell<Option<bool>>,
    needs_show: Cell<bool>,
}

impl TaskbarButton {
    pub(super) fn new(note: HWND, hidden: bool, cx: &App) -> Self {
        let mut button = Self {
            window: Rc::new(NoteHandle {
                note,
                owner: Cell::new(None),
                applied_hidden: Cell::new(Some(false)),
                needs_show: Cell::new(false),
            }),
            update: None,
        };
        // When requested, GPUI creates this HWND invisible. Set its owner
        // synchronously, before GPUI applies the saved placement and shows it.
        if hidden && let Err(err) = button.window.apply(true) {
            desktop_common::logging::log_error(
                "sticky_ui",
                format!("failed to prepare sticky note taskbar visibility: {err}"),
            );
            button.set_hidden(true, cx);
        }
        button
    }

    pub(super) fn set_hidden(&mut self, hidden: bool, cx: &App) {
        self.update.take();
        if self.window.applied_hidden.get() == Some(hidden) {
            return;
        }
        // Showing the window can synchronously request a GPUI frame. Release the
        // App borrow first. The task belongs to this note; a newer request or
        // closing the note cancels a pending retry.
        let window = self.window.clone();
        let background = cx.background_executor().clone();
        self.update = Some(cx.foreground_executor().spawn(async move {
            if !window.is_valid() {
                return;
            }
            if let Err(first) = window.apply(hidden) {
                background.timer(Duration::from_millis(150)).await;
                if !window.is_valid() {
                    return;
                }
                if let Err(err) = window.apply(hidden) {
                    desktop_common::logging::log_error(
                        "sticky_ui",
                        format!(
                            "failed to change sticky note taskbar visibility: {first}; retry: {err}"
                        ),
                    );
                }
            }
        }));
    }
}

impl NoteHandle {
    fn is_valid(&self) -> bool {
        unsafe { IsWindow(Some(self.note)) }.as_bool()
    }

    fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.note) }.as_bool()
    }

    fn hidden_owner(&self) -> Result<HWND> {
        if let Some(owner) = self.owner.get() {
            return Ok(owner);
        }
        // WPF's ShowInTaskbar gives each window its own invisible owner. This
        // keeps a note independent of the main window and of the other notes.
        let owner = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                PCWSTR::null(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )?
        };
        self.owner.set(Some(owner));
        Ok(owner)
    }

    fn apply(&self, hidden: bool) -> Result<()> {
        if !self.is_valid() {
            return Ok(());
        }
        self.applied_hidden.set(None);
        let change = self.change_style(hidden);
        self.restore_visibility()?;
        change?;
        self.applied_hidden.set(Some(hidden));
        Ok(())
    }

    fn change_style(&self, hidden: bool) -> Result<()> {
        let owner = if hidden {
            self.hidden_owner()?.0 as isize
        } else {
            0
        };
        let style = get_window_long(self.note, GWL_EXSTYLE)?;
        let app_window = WS_EX_APPWINDOW.0 as isize;
        let new_style = if hidden {
            style & !app_window
        } else {
            style | app_window
        };
        let old_owner = get_window_long(self.note, GWLP_HWNDPARENT)?;
        if style == new_style && owner == old_owner {
            return Ok(());
        }
        // Explorer reads the owner and APPWINDOW bit when a window is shown.
        if self.is_visible() {
            self.needs_show.set(true);
            set_visibility(self.note, SWP_HIDEWINDOW)?;
        }
        set_window_long(self.note, GWL_EXSTYLE, new_style)?;
        if owner != old_owner
            && let Err(err) = set_window_long(self.note, GWLP_HWNDPARENT, owner)
        {
            set_window_long(self.note, GWL_EXSTYLE, style)?;
            return Err(err);
        }
        Ok(())
    }

    fn restore_visibility(&self) -> Result<()> {
        if self.needs_show.get() && !self.is_visible() {
            let result = set_visibility(self.note, SWP_SHOWWINDOW);
            if !self.is_visible() {
                let show = if unsafe { IsIconic(self.note) }.as_bool() {
                    SW_SHOWMINNOACTIVE
                } else {
                    SW_SHOWNA
                };
                let _ = unsafe { ShowWindow(self.note, show) };
            }
            if !self.is_visible() {
                result?;
                return Err(Error::new(E_FAIL, "sticky note is still hidden"));
            }
        }
        self.needs_show.set(false);
        Ok(())
    }
}

impl Drop for NoteHandle {
    fn drop(&mut self) {
        let Some(owner) = self.owner.get() else {
            return;
        };
        // GPUI may release the view before the native note is destroyed.
        if self.is_valid()
            && unsafe { GetWindow(self.note, GW_OWNER) }.ok() == Some(owner)
            && let Err(err) = set_window_long(self.note, GWLP_HWNDPARENT, 0)
        {
            desktop_common::logging::log_error(
                "sticky_ui",
                format!("failed to detach sticky note from its taskbar owner: {err}"),
            );
        }
        if unsafe { IsWindow(Some(owner)) }.as_bool() {
            let _ = unsafe { DestroyWindow(owner) };
        }
    }
}

pub(super) fn show_inactive(note: HWND, bounds: Bounds<Pixels>, scale: f32, cx: &App) {
    cx.foreground_executor()
        .spawn(async move {
            if !unsafe { IsWindow(Some(note)) }.as_bool() {
                return;
            }
            if let Err(err) = place_inactive(note, bounds, scale) {
                desktop_common::logging::log_error(
                    "sticky_ui",
                    format!("failed to show restored sticky note: {err}"),
                );
            }
        })
        .detach();
}

pub(super) fn activate(note: HWND, cx: &App) {
    cx.foreground_executor()
        .spawn(async move {
            unsafe {
                if !IsWindow(Some(note)).as_bool() {
                    return;
                }
                if IsIconic(note).as_bool() {
                    let _ = ShowWindow(note, SW_RESTORE);
                }
                let _ = SetForegroundWindow(note);
                let _ = SetActiveWindow(note);
                let _ = SetFocus(Some(note));
            }
        })
        .detach();
}

fn place_inactive(note: HWND, bounds: Bounds<Pixels>, scale: f32) -> Result<()> {
    let mut window_rect = RECT::default();
    let mut client_rect = RECT::default();
    unsafe {
        GetWindowRect(note, &mut window_rect)?;
        GetClientRect(note, &mut client_rect)?;
    }
    let width_offset = (window_rect.right - window_rect.left) - (client_rect.right - client_rect.left);
    let height_offset = (window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top);
    let left_offset = width_offset / 2;
    let top_offset = height_offset / 2;
    let right_offset = width_offset - left_offset;
    let bottom_offset = height_offset - top_offset;
    let bounds = bounds.to_device_pixels(scale);
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(note, &mut placement)? };
    placement.showCmd = SW_SHOWNOACTIVATE.0 as u32;
    placement.rcNormalPosition = RECT {
        left: bounds.left().0 - left_offset,
        top: bounds.top().0 - top_offset,
        right: bounds.right().0 + right_offset,
        bottom: bounds.bottom().0 + bottom_offset,
    };
    unsafe { SetWindowPlacement(note, &placement) }
}

fn set_visibility(note: HWND, visibility: SET_WINDOW_POS_FLAGS) -> Result<()> {
    let flags = SWP_NOMOVE
        | SWP_NOSIZE
        | SWP_NOZORDER
        | SWP_NOOWNERZORDER
        | SWP_NOACTIVATE
        | SWP_NOSENDCHANGING
        | visibility;
    unsafe { SetWindowPos(note, None, 0, 0, 0, 0, flags) }
}

fn set_window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> Result<()> {
    // A zero previous value is valid for an unowned window.
    unsafe {
        SetLastError(ERROR_SUCCESS);
        if SetWindowLongPtrW(hwnd, index, value) == 0 {
            GetLastError().ok()?;
        }
    }
    Ok(())
}

fn get_window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX) -> Result<isize> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
        let value = GetWindowLongPtrW(hwnd, index);
        if value == 0 {
            GetLastError().ok()?;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_LAYERED;

    struct TestWindow(HWND);

    impl TestWindow {
        fn new() -> Self {
            Self(
                unsafe {
                    CreateWindowExW(
                        WS_EX_APPWINDOW | WS_EX_LAYERED,
                        w!("STATIC"),
                        PCWSTR::null(),
                        WS_OVERLAPPEDWINDOW,
                        0,
                        0,
                        120,
                        80,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                .unwrap(),
            )
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) }.unwrap();
        }
    }

    #[gpui::test]
    fn notes_get_independent_taskbar_owners_before_their_first_show(cx: &mut gpui::TestAppContext) {
        let first = TestWindow::new();
        let second = TestWindow::new();
        let (mut a, b) = cx.update(|cx| {
            (
                TaskbarButton::new(first.0, true, cx),
                TaskbarButton::new(second.0, true, cx),
            )
        });
        let first_owner = unsafe { GetWindow(first.0, GW_OWNER) }.unwrap();
        let second_owner = unsafe { GetWindow(second.0, GW_OWNER) }.unwrap();
        assert_ne!(first_owner, second_owner);
        for window in [&first, &second] {
            assert!(!unsafe { IsWindowVisible(window.0) }.as_bool());
            let style = get_window_long(window.0, GWL_EXSTYLE).unwrap();
            assert_eq!(style & WS_EX_APPWINDOW.0 as isize, 0);
            assert_ne!(style & WS_EX_LAYERED.0 as isize, 0);
        }

        cx.update(|cx| a.set_hidden(false, cx));
        cx.run_until_parked();
        assert_eq!(get_window_long(first.0, GWLP_HWNDPARENT).unwrap(), 0);
        assert_ne!(
            get_window_long(first.0, GWL_EXSTYLE).unwrap() & WS_EX_APPWINDOW.0 as isize,
            0
        );
        assert!(!unsafe { IsWindowVisible(first.0) }.as_bool());
        assert_eq!(
            unsafe { GetWindow(second.0, GW_OWNER) }.unwrap(),
            second_owner
        );
        assert_eq!(b.window.applied_hidden.get(), Some(true));
    }
}
