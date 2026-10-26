//! The original's system-wide Quick Add: a small field over every window that adds a task
//! without bringing the app forward. Enter adds the task, Escape or a click elsewhere closes it.

use gpui::{AnyWindowHandle, Bounds, Entity, Focusable as _, Global, WindowBounds, WindowHandle, WindowKind, WindowOptions, point, size};
use ui::prelude::*;

use crate::app_view::AppView;
use crate::i18n;
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

const WIDTH: f32 = 560.;
const HEIGHT: f32 = 60.;

/// The open Quick Add window, which the shortcut brings back rather than opening another.
struct OpenQuickAdd(AnyWindowHandle);

impl Global for OpenQuickAdd {}

pub fn open(main: WindowHandle<AppView>, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenQuickAdd>().map(|open| open.0)
        && open.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    // Centered, a third of the way down the screen, as launchers sit.
    let mut bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    if let Some(display) = cx.primary_display() {
        let screen = display.bounds();
        bounds.origin = point(bounds.origin.x, screen.origin.y + screen.size.height / 3.);
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        kind: WindowKind::PopUp,
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| cx.new(|cx| QuickAddWindow::new(main, window, cx))) {
        Ok(handle) => {
            handle.update(cx, |_, window, _| window.activate_window()).ok();
            cx.set_global(OpenQuickAdd(handle.into()));
        }
        Err(err) => eprintln!("failed to open Quick Add: {err:#}"),
    }
}

pub struct QuickAddWindow {
    input: Entity<TextInput>,
}

impl QuickAddWindow {
    fn new(main: WindowHandle<AppView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.add_task, cx));
        cx.subscribe_in(&input, window, move |_, _, Submitted(text), window, cx| {
            let text = text.trim().to_owned();
            if !text.is_empty() {
                main.update(cx, |view, _, cx| view.add_quick_task(&text, cx)).ok();
            }
            close(window, cx);
        })
        .detach();
        cx.subscribe_in(&input, window, |_, _, _: &Cancelled, window, cx| close(window, cx)).detach();
        // A click in another window puts it away, as the original's does.
        cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                close(window, cx);
            }
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        Self { input }
    }
}

fn close(window: &mut Window, cx: &mut App) {
    if cx.has_global::<OpenQuickAdd>() {
        cx.remove_global::<OpenQuickAdd>();
    }
    window.remove_window();
}

impl Render for QuickAddWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .size_full()
            .px_4()
            .gap_3()
            .bg(theme.colors().elevated_surface_background)
            .border_1()
            .border_color(theme.colors().border)
            .text_color(theme.colors().text)
            .text_size(px(16.))
            .child(Icon::new(IconName::Plus).color(Color::Accent))
            .child(div().flex_1().min_w_0().overflow_hidden().child(self.input.clone()))
    }
}
