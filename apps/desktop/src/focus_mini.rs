//! The original's focus mini window: the running clock and what it is for in a small window of
//! its own, with pause, stop and a pin that keeps it over the other windows.

use gpui::{Bounds, Entity, TitlebarOptions, WindowBounds, WindowControlArea, WindowDecorations, WindowOptions, size};
use ui::prelude::*;

use crate::focus_view::FocusView;
use crate::i18n;

const WIDTH: f32 = 300.;
const HEIGHT: f32 = 96.;

/// Opens the mini window of `focus`'s clock, kept over the other windows from the start, as the
/// original's is.
pub fn open(focus: Entity<FocusView>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some(i18n::t(cx).focus.into()), appears_transparent: true, traffic_light_position: None }),
        window_decorations: Some(WindowDecorations::Client),
        app_owns_titlebar_drag: true,
        is_resizable: false,
        app_id: Some(std::env::var("FERN_APP_ID").unwrap_or_else(|_| "fern-tasks".into())),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            cx.observe(&focus, |_, _, cx| cx.notify()).detach();
            #[cfg(windows)]
            crate::sticky_note::set_topmost(window, true);
            #[cfg(not(windows))]
            let _ = window;
            FocusMini { focus, pinned: cfg!(windows), should_move: false }
        })
    });
    if let Err(err) = opened {
        eprintln!("failed to open the focus mini window: {err:#}");
    }
}

pub struct FocusMini {
    focus: Entity<FocusView>,
    /// Kept over the other windows by its pin (Windows).
    pinned: bool,
    /// Linux and macOS: a press turns into a window move once the mouse moves.
    should_move: bool,
}

impl Render for FocusMini {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let now = chrono::Local::now().timestamp_millis();
        let (clock, label, running, paused) = self.focus.read(cx).clock_state(now, strings);
        let button = |id: &'static str, icon: IconName| IconButton::new(id, icon).size(px(28.)).icon_size(IconSize::Small);
        h_flex()
            .size_full()
            .pr_2()
            .gap_1()
            .bg(theme.colors().background)
            .border_1()
            .border_color(theme.colors().border)
            .text_color(theme.colors().text)
            .child(
                // The clock and its name move the window, as the title bar does; the buttons
                // stay out of that area.
                v_flex()
                    .id("focus-mini-drag")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .justify_center()
                    .pl_4()
                    .window_control_area(WindowControlArea::Drag)
                    .when(!cfg!(windows), |el| {
                        el.on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = true))
                            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
                            .on_mouse_move(cx.listener(|this, _, window, _| {
                                if this.should_move {
                                    this.should_move = false;
                                    window.start_window_move();
                                }
                            }))
                    })
                    .child(Label::new(label).size(LabelSize::Small).color(Color::Muted).truncate())
                    .child(div().text_size(px(30.)).line_height(px(36.)).font_weight(gpui::FontWeight::SEMIBOLD).child(clock)),
            )
            .child(
                button("focus-mini-play", if running && !paused { IconName::Pause } else { IconName::Play })
                    .icon_color(Color::Accent)
                    .tooltip(Tooltip::text(if !running { strings.focus_start } else if paused { strings.focus_continue } else { strings.focus_pause }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.focus.update(cx, |focus, cx| if running { focus.toggle_pause(cx) } else { focus.start_next(cx) });
                    })),
            )
            .when(running, |el| {
                el.child(
                    button("focus-mini-stop", IconName::Square)
                        .icon_color(Color::Muted)
                        .tooltip(Tooltip::text(strings.focus_end))
                        .on_click(cx.listener(|this, _, _, cx| this.focus.update(cx, |focus, cx| focus.finish(cx)))),
                )
            })
            .when(cfg!(windows), |el| {
                el.child(
                    button("focus-mini-pin", if self.pinned { IconName::PinOff } else { IconName::Pin })
                        .icon_color(if self.pinned { Color::Accent } else { Color::Muted })
                        .tooltip(Tooltip::text(if self.pinned { strings.unpin } else { strings.pin }))
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.pinned = !this.pinned;
                            #[cfg(windows)]
                            crate::sticky_note::set_topmost(_window, this.pinned);
                            cx.notify();
                        })),
                )
            })
            .child(
                button("focus-mini-close", IconName::X)
                    .icon_color(Color::Muted)
                    .on_click(|_, window, _| window.remove_window()),
            )
    }
}
