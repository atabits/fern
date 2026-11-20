use std::{collections::HashMap, time::{Duration, Instant}};

use fern_core::Id;
use gpui::{App, Global};

pub const SETTLE_DURATION: Duration = Duration::from_millis(260);

#[derive(Default)]
struct CompletionMarks(HashMap<Id, Instant>);

impl Global for CompletionMarks {}

pub fn record(task: Id, cx: &mut App) {
    if !cx.reduce_motion() {
        let now = cx.background_executor().now();
        if cx.try_global::<CompletionMarks>().is_none() {
            cx.set_global(CompletionMarks::default());
        }
        let marks = &mut cx.global_mut::<CompletionMarks>().0;
        marks.retain(|_, at| now.saturating_duration_since(*at) < SETTLE_DURATION);
        marks.insert(task, now);
    }
}

pub fn started(task: Id, cx: &App) -> Option<Instant> {
    let at = *cx.try_global::<CompletionMarks>()?.0.get(&task)?;
    (cx.background_executor().now().saturating_duration_since(at) < ui::CHECKBOX_COMPLETION_DURATION).then_some(at)
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use gpui::{
        Context, InteractiveElement, ParentElement, Render, Styled, TestAppContext, Window, div,
        px, size,
    };
    use std::time::Duration;
    use ui::{Checkbox, Toggleable};

    struct Preview {
        task: Id,
        checked: bool,
    }

    impl Render for Preview {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
            div().flex().items_start().child(
                div()
                    .flex()
                    .flex_none()
                    .debug_selector(|| "completion-checkbox".into())
                    .child(
                        Checkbox::new("check")
                            .toggle_state(self.checked)
                            .animate_completion(started(self.task, cx)),
                    ),
            )
        }
    }

    #[gpui::test]
    fn the_completion_stroke_preserves_checkbox_bounds(cx: &mut TestAppContext) {
        cx.update(theme::init);
        let task = Id::now_v7();
        let (view, cx) = cx.add_window_view(|_, _| Preview {
            task,
            checked: false,
        });
        let before = cx.debug_bounds("completion-checkbox").unwrap();
        assert_eq!(before.size, size(px(16.), px(16.)));
        cx.update(|_, cx| {
            view.update(cx, |preview, cx| {
                record(task, cx);
                preview.checked = true;
                cx.notify();
            })
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("completion-checkbox"), Some(before));
        assert!(cx.update(|window, cx| window.simulate_next_frame(cx)) > 0);
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("completion-checkbox"), Some(before));
        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 0);
    }
}
