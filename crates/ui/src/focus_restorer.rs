use gpui::{App, FocusHandle, WeakFocusHandle, Window};

/// Captures the active focus target when opening a modal or popover and restores it on close.
#[derive(Clone, Default)]
pub struct FocusRestorer(Option<WeakFocusHandle>);

impl FocusRestorer {
    /// Records the currently focused element in `window`, if any.
    pub fn capture(window: &Window, cx: &App) -> Self {
        Self(window.focused(cx).map(|handle| handle.downgrade()))
    }

    /// Captures focus only if nothing is already stored.
    pub fn capture_if_empty(&mut self, window: &Window, cx: &App) {
        if self.0.is_none() {
            *self = Self::capture(window, cx);
        }
    }

    /// Restores focus to the captured element, if it is still alive.
    pub fn restore(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if let Some(handle) = self.0.take().and_then(|weak| weak.upgrade()) {
            window.focus(&handle, cx);
            true
        } else {
            false
        }
    }

    /// Restores focus to the captured element, or falls back to `fallback`.
    pub fn restore_or(&mut self, fallback: &FocusHandle, window: &mut Window, cx: &mut App) {
        if !self.restore(window, cx) {
            window.focus(fallback, cx);
        }
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }

    pub fn is_captured(&self) -> bool {
        self.0.is_some()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, IntoElement, ParentElement, Render, TestAppContext, div};
    use gpui::InteractiveElement as _;

    use super::*;

    struct FocusTestView {
        a: FocusHandle,
        b: FocusHandle,
        restorer: FocusRestorer,
    }

    impl Render for FocusTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(div().id("a").track_focus(&self.a))
                .child(div().id("b").track_focus(&self.b))
        }
    }

    #[gpui::test]
    fn focus_restorer_captures_and_restores_previous_focus(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| FocusTestView {
            a: cx.focus_handle(),
            b: cx.focus_handle(),
            restorer: FocusRestorer::default(),
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                window.focus(&v.a, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.restorer = FocusRestorer::capture(window, cx);
                assert!(v.restorer.is_captured());
                window.focus(&v.b, cx);
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                assert!(v.b.is_focused(window));
                assert!(v.restorer.restore(window, cx));
                assert!(!v.restorer.is_captured());
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            assert!(view.read(cx).a.is_focused(window));
        });
    }
}
