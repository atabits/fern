//! Transient native surfaces for a note which can be smaller than its menus.
//! The task editor owns the draft and actions; this view only hosts their existing UI.

use gpui::{AnyView, AvailableSpace, Focusable as _, Point, Size, WeakFocusHandle, WindowBackgroundAppearance};

use super::*;

const MARGIN: f32 = 8.;

#[derive(Clone, Copy)]
pub(super) struct Anchor {
    pub point: Point<Pixels>,
    pub above: bool,
    pub right: bool,
    pub screen: Bounds<Pixels>,
}

impl Anchor {
    pub fn bounds(self, desired: Size<Pixels>) -> Bounds<Pixels> {
        let screen = self.screen;
        let size = size(
            desired.width.min(screen.size.width),
            desired.height.min(screen.size.height),
        );
        let x = if self.right {
            self.point.x - size.width
        } else {
            self.point.x
        };
        let preferred_y = if self.above {
            self.point.y - size.height
        } else {
            self.point.y
        };
        let alternate_y = if self.above {
            self.point.y
        } else {
            self.point.y - size.height
        };
        let y = if preferred_y < screen.top() || preferred_y + size.height > screen.bottom() {
            alternate_y
        } else {
            preferred_y
        };
        Bounds::new(
            point(
                x.max(screen.left()).min(screen.right() - size.width),
                y.max(screen.top()).min(screen.bottom() - size.height),
            ),
            size,
        )
    }
}

fn options(
    owner: &Window,
    anchor: Anchor,
    initial_size: Size<Pixels>,
    focus: bool,
    cx: &App,
) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(owner.pixel_snap_bounds(anchor.bounds(initial_size)))),
        titlebar: None,
        kind: WindowKind::PopUp,
        display_id: owner.display(cx).map(|display| display.id()),
        window_background: WindowBackgroundAppearance::Transparent,
        is_resizable: false,
        is_minimizable: false,
        focus,
        ..Default::default()
    }
}

/// Use the compositor's positioner where supported (Wayland). macOS and X11
/// currently need exact initial bounds; GPUI does not expose moving those windows.
pub(super) fn open<V: Render + 'static>(
    owner: &Window,
    anchor: Anchor,
    measured: Size<Pixels>,
    focus: bool,
    build: impl FnOnce(bool, &mut Window, &mut App) -> Entity<V>,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<V>> {
    if !cfg!(windows) {
        use gpui::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupNotSupportedError, PopupOptions};
        let mut build = Some(build);
        let mut anchored = options(owner, anchor, measured, focus, cx);
        anchored.kind = WindowKind::AnchoredPopup(PopupOptions {
            parent: owner.window_handle(),
            anchor_rect: Bounds::new(anchor.point - owner.bounds().origin, Size::default()),
            anchor: PopupAnchor::Center,
            gravity: match (anchor.above, anchor.right) {
                (true, true) => PopupGravity::TopLeft,
                (true, false) => PopupGravity::TopRight,
                (false, true) => PopupGravity::BottomLeft,
                (false, false) => PopupGravity::BottomRight,
            },
            constraint_adjustment: PopupConstraintAdjustment::FLIP_Y
                | PopupConstraintAdjustment::SLIDE_X | PopupConstraintAdjustment::SLIDE_Y,
            offset: Point::default(),
            // Menus are opened from clicks, after the native grab serial expires.
            // Their activation/outside-click handlers own dismissal instead.
            grab: false,
        });
        match cx.open_window(anchored, |window, cx| build.take().unwrap()(true, window, cx)) {
            Err(error) if error.is::<PopupNotSupportedError>() => {},
            result => return result,
        }
        return cx.open_window(options(owner, anchor, measured, focus, cx), |window, cx| {
            build.take().unwrap()(false, window, cx)
        });
    }
    cx.open_window(options(owner, anchor, measured, focus, cx), |window, cx| build(false, window, cx))
}

pub(super) fn screen(owner: &Window, cx: &App) -> Bounds<Pixels> {
    owner
        .display(cx)
        .map_or(owner.bounds(), |display| display.bounds())
}

#[cfg(windows)]
pub(super) fn attach(
    owner: Option<windows::Win32::Foundation::HWND>,
    window: &Window,
    tooltip: bool,
) {
    use windows::Win32::Graphics::Dwm::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    if let (Some(owner), Some(popup)) = (owner, win32_window_handle(window)) {
        unsafe {
            SetWindowLongPtrW(popup, GWLP_HWNDPARENT, owner.0 as isize);
            // GPUI's Windows PopUp currently uses an overlapped window style. DWM then
            // draws a second rounded frame around our transparent shadow margin.
            let style = GetWindowLongPtrW(popup, GWL_STYLE);
            SetWindowLongPtrW(
                popup,
                GWL_STYLE,
                (style | WS_POPUP.0 as isize) & !((WS_CAPTION | WS_THICKFRAME).0 as isize),
            );
            let _ = DwmSetWindowAttribute(
                popup,
                DWMWA_NCRENDERING_POLICY,
                &DWMNCRP_DISABLED as *const _ as _,
                std::mem::size_of_val(&DWMNCRP_DISABLED) as u32,
            );
            // These attributes are available on Windows 11; the style above also works
            // on Windows 10, where unsupported decoration attributes are harmless.
            let _ = DwmSetWindowAttribute(
                popup,
                DWMWA_BORDER_COLOR,
                &DWMWA_COLOR_NONE as *const _ as _,
                std::mem::size_of_val(&DWMWA_COLOR_NONE) as u32,
            );
            let _ = DwmSetWindowAttribute(
                popup,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &DWMWCP_DONOTROUND as *const _ as _,
                std::mem::size_of_val(&DWMWCP_DONOTROUND) as u32,
            );
            if let Err(err) = SetWindowPos(
                popup,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            ) {
                eprintln!("failed to remove sticky popup frame: {err}");
            }
            if tooltip {
                let style = GetWindowLongPtrW(popup, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    popup,
                    GWL_EXSTYLE,
                    style | (WS_EX_NOACTIVATE | WS_EX_TRANSPARENT).0 as isize,
                );
            }
        }
    }
}

fn fit_window(window: &mut Window, bounds: Bounds<Pixels>, _cx: &mut App) -> bool {
    #[cfg(windows)]
    if let Some(hwnd) = win32_window_handle(window) {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let scale = window.scale_factor();
        let bounds = bounds.to_device_pixels(scale);
        // SetWindowPos dispatches WM_SIZE synchronously. As in GPUI's WindowsWindow::resize,
        // run it outside App/Window updates so the resize callback can borrow them and
        // refresh the viewport and paint mask before drawing the enlarged menu.
        _cx.foreground_executor()
            .spawn(async move {
                if let Err(err) = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        bounds.origin.x.0,
                        bounds.origin.y.0,
                        bounds.size.width.0,
                        bounds.size.height.0,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                } {
                    eprintln!("failed to position sticky popup: {err}");
                }
            })
            .detach();
        return true;
    }
    false
}

fn measure(mut content: AnyElement, window: &mut Window, cx: &mut App) -> Size<Pixels> {
    content.layout_as_root(size(AvailableSpace::MaxContent, AvailableSpace::MaxContent), window, cx)
}

fn detail_surface(detail: &Entity<TaskDetail>, anchor: Anchor, cx: &mut App) -> Div {
    let available = anchor.screen.size - size(px(MARGIN * 2.), px(MARGIN * 2.));
    let content = detail.update(cx, |detail, cx| detail.render_sticky_popup(available, cx));
    div().p(px(MARGIN)).pl(px(MARGIN) + detail.read(cx).sticky_submenu_space()).child(content)
}

pub(super) fn measure_detail(detail: &Entity<TaskDetail>, anchor: Anchor, window: &mut Window, cx: &mut App) -> Size<Pixels> {
    measure(detail_surface(detail, anchor, cx).into_any_element(), window, cx)
}

/// Measure once and paint that same element, as GPUI's root/anchored elements do.
/// The screen limits the content; the note's viewport never does.
fn content_surface(
    mut content: AnyElement,
    anchor: Anchor,
    anchored: bool,
    window: &mut Window,
    cx: &mut App,
    fit: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let measured = content.layout_as_root(
        size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        window,
        cx,
    );
    let bounds = window.pixel_snap_bounds(anchor.bounds(measured));
    let changed = if anchored { window.bounds().size != bounds.size } else { window.bounds() != bounds };
    if changed {
        window.defer(cx, move |window, cx| fit(bounds, window, cx));
    }
    gpui::canvas(
        move |_, window, cx| {
            content.prepaint_at(point(px(0.), px(0.)), window, cx);
            content
        },
        |_, mut content, window, cx| content.paint(window, cx),
    )
    .w(measured.width)
    .h(measured.height)
}

pub(super) struct DetailPopup {
    note: WindowHandle<StickyNote>,
    detail: Entity<TaskDetail>,
    anchor: Anchor,
    anchored: bool,
    replacing: bool,
    focus: FocusHandle,
    return_focus: Option<WeakFocusHandle>,
    _inputs: crate::task_detail::PopupWindow,
}

impl DetailPopup {
    pub fn new(
        note: WindowHandle<StickyNote>,
        detail: Entity<TaskDetail>,
        anchor: Anchor,
        anchored: bool,
        return_focus: Option<WeakFocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let (input_focus, _) = detail.read(cx).sticky_popup_inputs(cx);
        if let Some(focus) = input_focus {
            window.focus(&focus, cx);
        }
        let inputs = detail.update(cx, |detail, cx| detail.bind_popup_window(window, cx));
        // The owner reads this view during rendering, so GPUI's implicit entity
        // window can point at the note. Lifecycle callbacks must target the popup.
        let popup_window = window.window_handle();
        cx.observe(&detail, move |_, _, cx| {
            let popup = cx.weak_entity();
            cx.defer(move |cx| {
                popup_window.update(cx, |_, window, cx| {
                    popup.update(cx, |this, cx| {
                        if this.detail.read(cx).has_popup() {
                            cx.notify();
                        } else {
                            this.close(true, window, cx);
                        }
                    }).ok();
                }).ok();
            });
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() && !this.replacing {
                // Activating an owned window can briefly activate its owner on Windows.
                let popup = cx.weak_entity();
                window.defer(cx, move |window, cx| {
                    popup.update(cx, |this, cx| {
                        let owner_active = this
                            .note
                            .update(cx, |_, owner, _| owner.is_window_active())
                            .unwrap_or(false);
                        if !window.is_window_active() && !owner_active && !this.replacing {
                            this.close(false, window, cx);
                        }
                    }).ok();
                });
            }
        })
        .detach();
        Self {
            note,
            detail,
            anchor,
            anchored,
            replacing: false,
            focus,
            return_focus,
            _inputs: inputs,
        }
    }

    fn fit(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.anchored {
            window.resize(bounds.size);
            return;
        }
        if fit_window(window, bounds, cx) || self.replacing { return; }
        // A resize-only fallback leaves the origin at the old size's anchor.
        // Reopen at the measured bounds on backends without a positioner/move API.
        // The same entities retain drafts, text and selection; submenu hover does
        // not enter here because its area is reserved in the main surface.
        let Some(previous) = window.window_handle().downcast::<Self>() else { return };
        let note_handle = self.note;
        let detail = self.detail.clone();
        let anchor = self.anchor;
        let return_focus = self.return_focus.clone();
        let (input, [tags, time]) = detail.read(cx).sticky_popup_inputs(cx);
        let focused = window.focused(cx).filter(|focus| {
            input.as_ref() == Some(focus) || *focus == tags.focus_handle(cx) || *focus == time.focus_handle(cx)
        });
        self.replacing = true;
        cx.defer(move |cx| {
            note_handle.update(cx, |note, owner, cx| {
                if note.detail_popup != Some(previous) || !detail.read(cx).has_popup() { return; }
                match open(owner, anchor, bounds.size, true, |anchored, window, cx| {
                    window.set_window_title(i18n::t(cx).more);
                    let popup = cx.new(|cx| Self::new(note_handle, detail, anchor, anchored, return_focus, window, cx));
                    if let Some(focus) = focused { window.focus(&focus, cx); }
                    popup
                }, cx) {
                    Ok(replacement) => {
                        note.detail_popup = Some(replacement);
                        previous.update(cx, |_, window, _| window.remove_window()).ok();
                    }
                    Err(error) => {
                        eprintln!("failed to reposition sticky task menu: {error:#}");
                        previous.update(cx, |popup, _, _| popup.replacing = false).ok();
                    }
                }
            }).ok();
        });
    }

    fn close(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = window
            .focused(cx)
            .filter(|focus| self.detail.read(cx).is_editor_focus(focus, cx))
            .or_else(|| {
                self.return_focus
                    .as_ref()
                    .and_then(WeakFocusHandle::upgrade)
            });
        self.detail.update(cx, |detail, cx| {
            detail.dismiss_popup(window, cx);
        });
        let note = self.note;
        window.remove_window();
        cx.defer(move |cx| {
            note.update(cx, |note, owner, cx| {
                note.detail_popup = None;
                if restore {
                    owner.activate_window();
                    if let Some(focus) = target {
                        owner.focus(&focus, cx);
                    }
                }
            })
            .ok();
        });
    }
}

impl Render for DetailPopup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = detail_surface(&self.detail, self.anchor, cx)
            .id("sticky-detail-popup")
            .key_context("TaskDetail")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &DismissTransient, window, cx| this.close(true, window, cx)),
            )
            .into_any_element();
        let this = cx.weak_entity();
        content_surface(content, self.anchor, self.anchored, window, cx, move |bounds, window, cx| {
            this.update(cx, |this, cx| this.fit(bounds, window, cx)).ok();
        })
    }
}

/// GPUI controls the delay and hover lifetime, while the tooltip itself is outside the note.
pub(super) fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |owner, cx| {
        let anchor = Anchor {
            point: owner.bounds().origin + owner.mouse_position() + point(px(8.), px(22.)),
            above: false,
            right: false,
            screen: screen(owner, cx),
        };
        let text = text.clone();
        cx.new(|cx| {
            cx.on_release(|lifetime: &mut TooltipLifetime, cx| {
                if let Some(popup) = lifetime.popup {
                    popup.update(cx, |_, window, _| window.remove_window()).ok();
                }
            })
            .detach();
            TooltipLifetime { text, anchor, popup: None, opening: false }
        })
        .into()
    }
}

struct TooltipLifetime {
    text: SharedString,
    anchor: Anchor,
    popup: Option<WindowHandle<NoteTooltip>>,
    opening: bool,
}
impl Render for TooltipLifetime {
    fn render(&mut self, owner: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.popup.is_none() && !self.opening {
            let measured = measure(tooltip_content(self.text.clone(), cx), owner, cx);
            self.opening = true;
            cx.defer_in(owner, move |this, owner, cx| {
                #[cfg(windows)]
                let native_owner = win32_window_handle(owner);
                let text = this.text.clone();
                let anchor = this.anchor;
                this.popup = open(owner, anchor, measured, false, |anchored, window, cx| {
                    #[cfg(windows)]
                    attach(native_owner, window, true);
                    cx.new(|_| NoteTooltip { text, anchor, anchored })
                }, cx).ok();
            });
        }
        div().size_0()
    }
}

struct NoteTooltip {
    text: SharedString,
    anchor: Anchor,
    anchored: bool,
}
fn tooltip_content(text: SharedString, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        div()
            .p_1()
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(colors.elevated_surface_background)
                    .border_1()
                    .border_color(colors.border)
                    .text_color(colors.text)
                    .text_size(px(12.))
                    .whitespace_nowrap()
                    .child(text),
            )
            .into_any_element()
}
impl Render for NoteTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let anchored = self.anchored;
        content_surface(tooltip_content(self.text.clone(), cx), self.anchor, anchored, window, cx, move |bounds, window, cx| {
            if anchored { window.resize(bounds.size); } else { fit_window(window, bounds, cx); }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_placement_fits_each_screen_edge_including_a_left_hand_monitor() {
        let screen = Bounds::new(point(px(-1920.), px(0.)), size(px(1920.), px(1080.)));
        for position in [screen.origin, screen.bottom_right(), screen.center()] {
            for above in [true, false] {
                for right in [true, false] {
                    let bounds = Anchor {
                        point: position,
                        above,
                        right,
                        screen,
                    }
                    .bounds(size(px(300.), px(700.)));
                    assert!(bounds.left() >= screen.left() && bounds.right() <= screen.right());
                    assert!(bounds.top() >= screen.top() && bounds.bottom() <= screen.bottom());
                    assert_eq!(bounds.size, size(px(300.), px(700.)));
                }
            }
        }
        let bounds = Anchor {
            point: screen.center(),
            above: true,
            right: true,
            screen,
        }
        .bounds(size(px(2400.), px(1600.)));
        assert_eq!(bounds, screen);
    }
}
