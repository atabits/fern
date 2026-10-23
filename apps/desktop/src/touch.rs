//! Touch screens on Windows. Windows hands a finger to a window as a mouse with its button held,
//! so a swipe over a list would select text or drag a task instead of scrolling, and GPUI scrolls
//! only with a wheel or a touchpad. Here the finger's pointer messages are taken before GPUI sees
//! them and turned into gestures:
//! - a tap clicks where the finger touched; a second tap close by clicks the same spot, so that
//!   it counts as a double click;
//! - a swipe scrolls what is under the finger, and keeps going for a moment after it is lifted;
//!   a tap while it is going only stops it;
//! - a press held still shows a ring, and opens the context menu when the finger is lifted (a
//!   right click); a tap with two fingers opens it too;
//! - a press held still and then moved drags as a mouse would: tasks, lists, text selection;
//! - in a drag zone (a panel's resize edge) a swipe drags straight away.
//!
//! While the last input came from a finger, `active` is true, and the controls that otherwise
//! show only under the mouse are shown all the time. Elsewhere this does nothing.

use gpui::{App, Bounds, Pixels, Point, Window};

/// Starts taking touch input for `window`.
pub fn init(window: &Window, cx: &mut App) {
    imp::init(window, cx)
}

/// Whether the last input came from a finger rather than a mouse.
pub fn active() -> bool {
    imp::active()
}

/// Whether the mouse press being handled is a tap made here. The touch keyboard opens for it.
pub fn tapping() -> bool {
    imp::tapping()
}

/// Where a finger is held still long enough for the context menu, in window coordinates.
pub fn held() -> Option<Point<Pixels>> {
    imp::held()
}

/// Called as the window is drawn, before its elements: forgets the last frame's drag zones and
/// notes the line height, which converts a swipe into wheel steps.
pub fn begin_frame(window: &Window) {
    imp::begin_frame(window)
}

/// Marks `bounds` (in window coordinates) for this frame as a place where a swipe drags.
pub fn drag_zone(bounds: Bounds<Pixels>) {
    imp::drag_zone(bounds)
}

#[cfg(windows)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    use futures::StreamExt as _;
    use futures::channel::mpsc::{UnboundedSender, unbounded};
    use gpui::{App, Bounds, Pixels, Point, Window, point, px};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    use windows::Win32::UI::Input::Pointer::GetPointerType;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageExtraInfo, KillTimer, PT_TOUCH, SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetTimer, SystemParametersInfoW, WHEEL_DELTA,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_POINTERCAPTURECHANGED,
        WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_TIMER,
    };

    /// How far a finger moves, in logical pixels, before a press becomes a swipe or a drag.
    const SLOP: f32 = 8.;
    /// How close, in logical pixels, a second tap lands to count with the first.
    const DOUBLE_TAP_SLOP: f32 = 24.;
    /// How long a finger stays still before a press is held.
    const HOLD: Duration = Duration::from_millis(450);
    /// The momentum's step.
    const FRAME: Duration = Duration::from_millis(16);
    /// How much of its speed the momentum keeps per millisecond.
    const FRICTION: f32 = 0.996;
    /// Below this speed, in logical pixels per millisecond, the momentum stops.
    const MIN_SPEED: f32 = 0.05;
    /// The finger's last stretch of movement that sets the momentum's speed.
    const VELOCITY_WINDOW: Duration = Duration::from_millis(100);

    /// The held buttons in a mouse message.
    const MK_LBUTTON: usize = 0x1;
    const MK_RBUTTON: usize = 0x2;
    const SUBCLASS_ID: usize = 0xFE7A;
    const HOLD_TIMER: usize = 0xFE7A_0001;
    const MOMENTUM_TIMER: usize = 0xFE7A_0002;
    /// `POINTER_FLAG_PRIMARY` in a pointer message's flags: the first finger down.
    const POINTER_FLAG_PRIMARY: u32 = 0x2000;
    /// Mouse messages Windows makes from touch and pen input carry this in their extra info.
    const MI_WP_SIGNATURE: isize = 0xFF51_5700;
    const SIGNATURE_MASK: isize = 0xFFFF_FF00;

    #[derive(Clone, Copy, PartialEq)]
    enum Axis {
        Both,
        X,
        Y,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Phase {
        /// Down, neither moved past the slop nor held long enough.
        Pressed,
        /// Held still: lifting opens the context menu, moving drags.
        Held,
        /// A second finger came down on a press: lifting opens the context menu.
        TwoFingers,
        /// Scrolling along `Axis`.
        Panning(Axis),
        /// A mouse drag with the left button held.
        Dragging,
        /// Nothing happens when the finger is lifted.
        Dropped,
    }

    struct Gesture {
        pointer: u32,
        phase: Phase,
        /// Screen coordinates, in physical pixels.
        start: POINT,
        last: POINT,
        in_zone: bool,
        /// The press stopped a scroll's momentum, so a tap does nothing else.
        halted: bool,
        samples: VecDeque<(Instant, POINT)>,
    }

    struct Momentum {
        /// Logical pixels per millisecond.
        velocity: (f32, f32),
        at: POINT,
        last: Instant,
    }

    #[derive(Default)]
    struct State {
        gesture: Option<Gesture>,
        momentum: Option<Momentum>,
        /// Wheel steps not sent yet, being less than one.
        remainder: (f32, f32),
        /// The last tap, which a second tap close by snaps to.
        last_tap: Option<(Instant, POINT)>,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::default();
        static ACTIVE: Cell<bool> = const { Cell::new(false) };
        static TAPPING: Cell<bool> = const { Cell::new(false) };
        static HELD: Cell<Option<Point<Pixels>>> = const { Cell::new(None) };
        static LINE_HEIGHT: Cell<f32> = const { Cell::new(20.) };
        static ZONES: RefCell<Vec<Bounds<Pixels>>> = const { RefCell::new(Vec::new()) };
        static LAST_MOUSE: Cell<isize> = const { Cell::new(-1) };
        static REFRESH: RefCell<Option<UnboundedSender<()>>> = const { RefCell::new(None) };
    }

    /// What a message turns into, sent once the state is no longer borrowed: GPUI handles the
    /// messages right away and may call back in here.
    enum Out {
        Mouse(u32, usize, POINT),
        Wheel(u32, i16, POINT),
    }

    pub fn init(window: &Window, cx: &mut App) {
        // The trait's method: `Window` has an inherent `window_handle` too.
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let hwnd = HWND(handle.hwnd.get() as _);
        if !unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) }.as_bool() {
            eprintln!("touch: failed to subclass the window");
            return;
        }
        // Switching between the finger and the mouse, and the hold's ring, redraw the window.
        let (sender, mut changes) = unbounded();
        REFRESH.set(Some(sender));
        cx.spawn(async move |cx| {
            while changes.next().await.is_some() {
                cx.update(|cx| cx.refresh_windows());
            }
        })
        .detach();
    }

    pub fn active() -> bool {
        ACTIVE.get()
    }

    pub fn tapping() -> bool {
        TAPPING.get()
    }

    pub fn held() -> Option<Point<Pixels>> {
        HELD.get()
    }

    pub fn begin_frame(window: &Window) {
        LINE_HEIGHT.set(f32::from(window.line_height()));
        ZONES.with_borrow_mut(Vec::clear);
    }

    pub fn drag_zone(bounds: Bounds<Pixels>) {
        ZONES.with_borrow_mut(|zones| zones.push(bounds));
    }

    fn refresh() {
        REFRESH.with_borrow(|sender| {
            if let Some(sender) = sender {
                sender.unbounded_send(()).ok();
            }
        });
    }

    fn set_active(active: bool) {
        if ACTIVE.replace(active) != active {
            refresh();
        }
    }

    fn set_held(held: Option<Point<Pixels>>) {
        if HELD.replace(held) != held {
            refresh();
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
                let pointer = (wparam.0 & 0xFFFF) as u32;
                let mut kind = Default::default();
                if unsafe { GetPointerType(pointer, &mut kind) }.is_ok() && kind == PT_TOUCH {
                    // Not passed on: Windows would make mouse messages of it.
                    on_pointer(hwnd, msg, pointer, wparam, lparam);
                    return LRESULT(0);
                }
            }
            WM_TIMER if wparam.0 == HOLD_TIMER || wparam.0 == MOMENTUM_TIMER => {
                on_timer(hwnd, wparam.0);
                return LRESULT(0);
            }
            // A mouse that really moved (not one made here or from a pen) ends the touch mode.
            // Windows also sends moves to the same spot, when a window changes under the cursor.
            WM_MOUSEMOVE if !TAPPING.get() && unsafe { GetMessageExtraInfo() }.0 & SIGNATURE_MASK != MI_WP_SIGNATURE => {
                if LAST_MOUSE.replace(lparam.0) != lparam.0 && ACTIVE.get() {
                    set_active(false);
                }
            }
            _ => {}
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    fn on_pointer(hwnd: HWND, msg: u32, pointer: u32, wparam: WPARAM, lparam: LPARAM) {
        let at = POINT { x: lparam.0 as i16 as i32, y: (lparam.0 >> 16) as i16 as i32 };
        let primary = (wparam.0 >> 16) as u32 & POINTER_FLAG_PRIMARY != 0;
        let scale = scale(hwnd);
        let mut out = Vec::new();
        let mut held = HELD.get();
        STATE.with_borrow_mut(|state| {
            match msg {
                WM_POINTERDOWN if !primary => {
                    // A second finger on a press is a two-finger tap; other fingers are left
                    // out: no pinching.
                    if let Some(gesture) = state.gesture.as_mut().filter(|g| g.phase == Phase::Pressed) {
                        unsafe { KillTimer(Some(hwnd), HOLD_TIMER) }.ok();
                        gesture.phase = Phase::TwoFingers;
                    }
                }
                WM_POINTERDOWN => {
                    let halted = state.momentum.take().is_some();
                    if halted {
                        unsafe { KillTimer(Some(hwnd), MOMENTUM_TIMER) }.ok();
                    }
                    if let Some(gesture) = state.gesture.take() {
                        cancel(gesture, &mut out);
                    }
                    held = None;
                    state.remainder = (0., 0.);
                    state.gesture = Some(Gesture {
                        pointer,
                        phase: Phase::Pressed,
                        start: at,
                        last: at,
                        in_zone: in_zone(hwnd, at, scale),
                        halted,
                        samples: VecDeque::from([(Instant::now(), at)]),
                    });
                    unsafe { SetTimer(Some(hwnd), HOLD_TIMER, HOLD.as_millis() as u32, None) };
                }
                WM_POINTERUPDATE => {
                    let Some(gesture) = state.gesture.as_mut().filter(|g| g.pointer == pointer) else {
                        return;
                    };
                    let now = Instant::now();
                    gesture.samples.push_back((now, at));
                    while gesture.samples.len() > 2 && now - gesture.samples[0].0 > VELOCITY_WINDOW {
                        gesture.samples.pop_front();
                    }
                    let moved = distance(at, gesture.start) / scale > SLOP;
                    match gesture.phase {
                        Phase::Pressed if moved => {
                            unsafe { KillTimer(Some(hwnd), HOLD_TIMER) }.ok();
                            if gesture.in_zone {
                                start_drag(gesture, at, &mut out);
                            } else {
                                let (dx, dy) = ((at.x - gesture.start.x).abs(), (at.y - gesture.start.y).abs());
                                // A swipe mostly along one axis stays on it, as on rails.
                                let axis = if dy > 2 * dx {
                                    Axis::Y
                                } else if dx > 2 * dy {
                                    Axis::X
                                } else {
                                    Axis::Both
                                };
                                gesture.phase = Phase::Panning(axis);
                                let delta = along(axis, at.x - gesture.start.x, at.y - gesture.start.y);
                                scroll(&mut state.remainder, gesture.start, delta, scale, &mut out);
                            }
                        }
                        Phase::Held if moved => {
                            held = None;
                            start_drag(gesture, at, &mut out);
                        }
                        // Two fingers that wander off are not a tap.
                        Phase::TwoFingers if moved => gesture.phase = Phase::Dropped,
                        Phase::Panning(axis) => {
                            let delta = along(axis, at.x - gesture.last.x, at.y - gesture.last.y);
                            scroll(&mut state.remainder, gesture.start, delta, scale, &mut out);
                        }
                        Phase::Dragging => out.push(Out::Mouse(WM_MOUSEMOVE, MK_LBUTTON, at)),
                        Phase::Pressed | Phase::Held | Phase::TwoFingers | Phase::Dropped => {}
                    }
                    gesture.last = at;
                }
                WM_POINTERUP => {
                    let Some(gesture) = state.gesture.take_if(|g| g.pointer == pointer) else {
                        return;
                    };
                    unsafe { KillTimer(Some(hwnd), HOLD_TIMER) }.ok();
                    held = None;
                    match gesture.phase {
                        Phase::Pressed if gesture.halted => {}
                        Phase::Pressed => {
                            set_active(true);
                            let now = Instant::now();
                            let double_click = Duration::from_millis(unsafe { GetDoubleClickTime() }.into());
                            let target = match state.last_tap {
                                Some((when, spot)) if now - when <= double_click && distance(spot, gesture.start) / scale <= DOUBLE_TAP_SLOP => spot,
                                _ => gesture.start,
                            };
                            state.last_tap = Some((now, target));
                            out.push(Out::Mouse(WM_MOUSEMOVE, 0, target));
                            out.push(Out::Mouse(WM_LBUTTONDOWN, MK_LBUTTON, target));
                            out.push(Out::Mouse(WM_LBUTTONUP, 0, target));
                        }
                        Phase::Held | Phase::TwoFingers => {
                            set_active(true);
                            out.push(Out::Mouse(WM_MOUSEMOVE, 0, gesture.start));
                            out.push(Out::Mouse(WM_RBUTTONDOWN, MK_RBUTTON, gesture.start));
                            out.push(Out::Mouse(WM_RBUTTONUP, 0, gesture.start));
                        }
                        Phase::Panning(axis) => {
                            set_active(true);
                            if let Some(velocity) = velocity(&gesture.samples, scale) {
                                let velocity = along_f(axis, velocity);
                                if velocity.0.hypot(velocity.1) > MIN_SPEED {
                                    state.momentum = Some(Momentum { velocity, at: gesture.start, last: Instant::now() });
                                    unsafe { SetTimer(Some(hwnd), MOMENTUM_TIMER, FRAME.as_millis() as u32, None) };
                                }
                            }
                        }
                        Phase::Dragging => out.push(Out::Mouse(WM_LBUTTONUP, 0, at)),
                        Phase::Dropped => {}
                    }
                }
                // Windows took the finger away, for a system gesture say.
                _ => {
                    if let Some(gesture) = state.gesture.take_if(|g| g.pointer == pointer) {
                        unsafe { KillTimer(Some(hwnd), HOLD_TIMER) }.ok();
                        held = None;
                        cancel(gesture, &mut out);
                    }
                }
            }
        });
        set_held(held);
        send(hwnd, out);
    }

    fn on_timer(hwnd: HWND, timer: usize) {
        let scale = scale(hwnd);
        let mut out = Vec::new();
        let mut held = HELD.get();
        STATE.with_borrow_mut(|state| {
            if timer == HOLD_TIMER {
                unsafe { KillTimer(Some(hwnd), HOLD_TIMER) }.ok();
                if let Some(gesture) = state.gesture.as_mut().filter(|g| g.phase == Phase::Pressed) {
                    gesture.phase = Phase::Held;
                    held = client_point(hwnd, gesture.start, scale);
                }
                return;
            }
            let Some(momentum) = state.momentum.as_mut() else {
                unsafe { KillTimer(Some(hwnd), MOMENTUM_TIMER) }.ok();
                return;
            };
            let now = Instant::now();
            let elapsed = (now - momentum.last).as_secs_f32() * 1000.;
            momentum.last = now;
            let (vx, vy) = momentum.velocity;
            let delta = ((vx * elapsed * scale).round() as i32, (vy * elapsed * scale).round() as i32);
            let at = momentum.at;
            let decay = FRICTION.powf(elapsed);
            momentum.velocity = (vx * decay, vy * decay);
            if momentum.velocity.0.hypot(momentum.velocity.1) < MIN_SPEED {
                state.momentum = None;
                unsafe { KillTimer(Some(hwnd), MOMENTUM_TIMER) }.ok();
            }
            scroll(&mut state.remainder, at, delta, scale, &mut out);
        });
        set_held(held);
        send(hwnd, out);
    }

    /// A held or moved press becomes a mouse drag: the button goes down where the finger did.
    fn start_drag(gesture: &mut Gesture, at: POINT, out: &mut Vec<Out>) {
        set_active(true);
        gesture.phase = Phase::Dragging;
        out.push(Out::Mouse(WM_MOUSEMOVE, 0, gesture.start));
        out.push(Out::Mouse(WM_LBUTTONDOWN, MK_LBUTTON, gesture.start));
        out.push(Out::Mouse(WM_MOUSEMOVE, MK_LBUTTON, at));
    }

    /// A drag in progress lets go of the button; anything else is dropped.
    fn cancel(gesture: Gesture, out: &mut Vec<Out>) {
        if gesture.phase == Phase::Dragging {
            out.push(Out::Mouse(WM_LBUTTONUP, 0, gesture.last));
        }
    }

    fn distance(a: POINT, b: POINT) -> f32 {
        ((a.x - b.x) as f32).hypot((a.y - b.y) as f32)
    }

    fn along(axis: Axis, dx: i32, dy: i32) -> (i32, i32) {
        match axis {
            Axis::Both => (dx, dy),
            Axis::X => (dx, 0),
            Axis::Y => (0, dy),
        }
    }

    fn along_f(axis: Axis, (dx, dy): (f32, f32)) -> (f32, f32) {
        match axis {
            Axis::Both => (dx, dy),
            Axis::X => (dx, 0.),
            Axis::Y => (0., dy),
        }
    }

    /// The finger's speed over its last stretch, in logical pixels per millisecond; none if it
    /// stopped before it was lifted.
    fn velocity(samples: &VecDeque<(Instant, POINT)>, scale: f32) -> Option<(f32, f32)> {
        let (first, last) = (samples.front()?, samples.back()?);
        if last.0.elapsed() > Duration::from_millis(50) {
            return None;
        }
        let ms = (last.0 - first.0).as_secs_f32() * 1000.;
        (ms > 0.).then(|| ((last.1.x - first.1.x) as f32 / scale / ms, (last.1.y - first.1.y) as f32 / scale / ms))
    }

    /// Scrolls what is under `at` by the finger's `delta` in physical pixels. GPUI turns a wheel
    /// step into the system's lines (or characters, sideways) times the line height, so the
    /// finger's distance is turned back into steps; what is less than a step waits for the next.
    fn scroll(remainder: &mut (f32, f32), at: POINT, (dx, dy): (i32, i32), scale: f32, out: &mut Vec<Out>) {
        let line = LINE_HEIGHT.get().max(1.);
        let per_step = |action, fallback: u32| {
            let mut value = 0u32;
            let ok = unsafe {
                SystemParametersInfoW(action, 0, Some(&mut value as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
            }
            .is_ok();
            // 0 turns the wheel off, and a whole page is far more than a finger.
            let lines = if ok && value > 0 && value <= 100 { value } else { fallback };
            lines as f32 * line / WHEEL_DELTA as f32
        };
        if dy != 0 {
            remainder.1 += dy as f32 / scale / per_step(SPI_GETWHEELSCROLLLINES, 3);
        }
        if dx != 0 {
            // GPUI turns a step to the right into content moving left.
            remainder.0 -= dx as f32 / scale / per_step(SPI_GETWHEELSCROLLCHARS, 3);
        }
        for (message, value) in [(WM_MOUSEWHEEL, &mut remainder.1), (WM_MOUSEHWHEEL, &mut remainder.0)] {
            let whole = value.trunc().clamp(i16::MIN as f32, i16::MAX as f32);
            if whole != 0. {
                *value -= whole;
                out.push(Out::Wheel(message, whole as i16, at));
            }
        }
    }

    fn send(hwnd: HWND, out: Vec<Out>) {
        for message in out {
            match message {
                Out::Mouse(msg, keys, at) => {
                    let mut client = at;
                    unsafe { ScreenToClient(hwnd, &mut client) }.ok().ok();
                    let lparam = (client.x as i16 as u16 as isize) | ((client.y as i16 as u16 as isize) << 16);
                    TAPPING.set(true);
                    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(keys)), Some(LPARAM(lparam))) };
                    TAPPING.set(false);
                }
                // The wheel's position is in screen coordinates.
                Out::Wheel(msg, steps, at) => {
                    let wparam = (steps as u16 as usize) << 16;
                    let lparam = (at.x as i16 as u16 as isize) | ((at.y as i16 as u16 as isize) << 16);
                    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) };
                }
            }
        }
    }

    fn scale(hwnd: HWND) -> f32 {
        match unsafe { GetDpiForWindow(hwnd) } {
            0 => 1.,
            dpi => dpi as f32 / 96.,
        }
    }

    /// A screen point in the window's logical coordinates.
    fn client_point(hwnd: HWND, at: POINT, scale: f32) -> Option<Point<Pixels>> {
        let mut client = at;
        unsafe { ScreenToClient(hwnd, &mut client) }
            .as_bool()
            .then(|| point(px(client.x as f32 / scale), px(client.y as f32 / scale)))
    }

    fn in_zone(hwnd: HWND, at: POINT, scale: f32) -> bool {
        let Some(position) = client_point(hwnd, at, scale) else {
            return false;
        };
        ZONES.with_borrow(|zones| zones.iter().any(|zone| zone.contains(&position)))
    }
}

#[cfg(not(windows))]
mod imp {
    use gpui::{App, Bounds, Pixels, Point, Window};

    pub fn init(_: &Window, _: &mut App) {}

    pub fn active() -> bool {
        false
    }

    pub fn tapping() -> bool {
        false
    }

    pub fn held() -> Option<Point<Pixels>> {
        None
    }

    pub fn begin_frame(_: &Window) {}

    pub fn drag_zone(_: Bounds<Pixels>) {}
}
