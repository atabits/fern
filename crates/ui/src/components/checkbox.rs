use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClickEvent, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    PathBuilder, Pixels, RenderOnce, StatefulInteractiveElement, Styled, Window, canvas, div, fill,
    point, prelude::FluentBuilder as _, px, rgb, size,
};
use theme::ActiveTheme;

use crate::traits::ClickHandler;
use crate::{Clickable, Toggleable};

pub const CHECKBOX_COMPLETION_DURATION: Duration = Duration::from_millis(180);

/// A square checkbox. A custom `tint` colors the border and fills the box lightly, which is how
/// task priorities are shown.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    dismissed: bool,
    lines: bool,
    tint: Option<Hsla>,
    size: Pixels,
    on_click: Option<ClickHandler>,
    accent: bool,
    check_mark: CheckMark,
}

enum CheckMark {
    Glyph,
    Stroke(Option<Instant>),
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), checked: false, dismissed: false, lines: false, tint: None, size: px(16.), on_click: None, accent: false, check_mark: CheckMark::Glyph }
    }

    pub fn tint(mut self, tint: Option<Hsla>) -> Self {
        self.tint = tint;
        self
    }

    /// A muted box with a cross, for tasks closed as "Won't Do".
    pub fn dismissed(mut self, dismissed: bool) -> Self {
        self.dismissed = dismissed;
        self
    }

    /// Lines inside the open box, as the original marks a task kept as a checklist.
    pub fn lines(mut self, lines: bool) -> Self {
        self.lines = lines;
        self
    }

    /// Checked in the accent color, as the original's settings boxes are (tasks are grey).
    pub fn accent(mut self) -> Self {
        self.accent = true;
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// A filled task checkbox with a drawn check. The outline never changes size.
    pub fn animate_completion(mut self, started: Option<Instant>) -> Self {
        self.check_mark = CheckMark::Stroke(started);
        self
    }
}

impl Toggleable for Checkbox {
    fn toggle_state(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

impl Clickable for Checkbox {
    fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        if self.dismissed {
            return div()
                .id(self.id)
                .size(self.size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(colors.text_muted.opacity(0.6))
                .bg(colors.text_muted.opacity(0.25))
                .cursor_pointer()
                .text_size(self.size * 0.7)
                .text_color(colors.text_muted)
                .child("✕")
                .when_some(self.on_click, |el, handler| el.on_click(handler));
        }
        if let CheckMark::Stroke(started) = self.check_mark {
            let border = self.tint.unwrap_or(colors.checkbox_border);
            return div()
                .id(self.id)
                .size(self.size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.5))
                .overflow_hidden()
                .cursor_pointer()
                .when(self.checked, |el| {
                    el.child(check_stroke(started, border, self.tint, colors.accent))
                })
                .when(!self.checked, |el| {
                    el.border_1()
                        .border_color(border)
                        .when_some(self.tint, |el, tint| el.bg(tint.opacity(0.12)))
                        .hover(|style| style.border_color(colors.accent).bg(colors.accent.opacity(0.08)))
                        .active(|style| style.bg(colors.accent.opacity(0.18)))
                        .when(self.lines, |el| {
                            el.child(div().flex().flex_col().gap(px(2.)).children(
                                (0..3).map(|_| div().w(self.size * 0.45).h(px(1.)).bg(border)),
                            ))
                        })
                })
                .when_some(self.on_click, |el, handler| el.on_click(handler));
        }
        let fill = if self.accent { colors.accent } else { colors.text_muted.opacity(0.6) };
        let border = if self.checked { fill } else { self.tint.unwrap_or(colors.checkbox_border) };
        div()
            .id(self.id)
            .size(self.size)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .when(self.checked, |el| el.bg(fill).child("✓"))
            .when(self.lines && !self.checked, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children((0..3).map(|_| div().w(self.size * 0.45).h(px(1.)).bg(border))),
                )
            })
            .when_some(self.tint.filter(|_| !self.checked), |el, tint| el.bg(tint.opacity(0.15)))
            .text_size(self.size * 0.7)
            .text_color(colors.background)
            .when_some(self.on_click, |el, handler| el.on_click(handler))
    }
}

fn check_stroke(started: Option<Instant>, border: Hsla, tint: Option<Hsla>, accent: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let progress = match started {
                Some(started) if !cx.reduce_motion() => {
                    (cx.background_executor().now().saturating_duration_since(started).as_secs_f32()
                        / CHECKBOX_COMPLETION_DURATION.as_secs_f32()).min(1.)
                }
                _ => 1.,
            };
            if progress < 1. {
                window.request_animation_frame();
            }

            // The fill settles first; the check follows a moment later. All paint stays in
            // the original box, including its border, so neither phase changes the layout.
            let ease_out = |t: f32| 1. - (1. - t.clamp(0., 1.)).powi(3);
            let filled = ease_out(progress / 0.65);
            if let Some(tint) = tint {
                window.paint_quad(fill(bounds, tint.opacity(0.12 * (1. - filled))).corner_radii(px(3.5)));
            }
            window.paint_quad(
                fill(bounds, accent.opacity(filled))
                    .corner_radii(px(3.5))
                    .border_widths(px(1.))
                    .border_color(border.blend(accent.opacity(filled))),
            );
            let stroke = ease_out((progress - 0.16) / 0.84);
            if stroke <= 0. { return; }

            let center = bounds.center();
            let side = bounds.size.width.min(bounds.size.height);
            let start = point(center.x - side * 0.23, center.y + side * 0.01);
            let bend = point(center.x - side * 0.075, center.y + side * 0.17);
            let end = point(center.x + side * 0.25, center.y - side * 0.18);
            let tip = if stroke <= 0.32 {
                start + (bend - start) * (stroke / 0.32)
            } else {
                bend + (end - bend) * ((stroke - 0.32) / 0.68)
            };
            let width = side * 0.115;
            let mut path = PathBuilder::stroke(width);
            path.move_to(start);
            if stroke > 0.32 { path.line_to(bend); }
            path.line_to(tip);
            let ink = rgb(0xffffff);
            if let Ok(path) = path.build() {
                window.paint_path(path, ink);
            }
            // Round caps and join keep a small check crisp without adding a font glyph.
            let radius = width / 2.;
            for at in [Some(start), (stroke > 0.32).then_some(bend), Some(tip)].into_iter().flatten() {
                window.paint_quad(fill(
                    Bounds::new(at - point(radius, radius), size(width, width)), ink,
                ).corner_radii(radius));
            }
        },
    )
    .size_full()
}
