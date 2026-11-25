//! Idempotent one-shot shutdown coordinator so every exit path (`on_app_quit`, window close,
//! `TryQuit`, updater restart) releases global OS resources once in deterministic order.

use gpui::{App, Global};

#[derive(Default)]
pub struct AppShutdown {
    completed: bool,
}

impl Global for AppShutdown {}

impl AppShutdown {
    fn run_once_with(cx: &mut App, shutdown_fn: impl FnOnce(&mut App)) -> bool {
        if cx.try_global::<Self>().is_some_and(|state| state.completed) {
            return false;
        }
        cx.set_global(Self { completed: true });
        shutdown_fn(cx);
        true
    }

    /// Executes OS resource cleanup (global hotkeys, plugin workers, tray icon, single-instance lock)
    /// at most once across the lifetime of the process.
    pub fn run_once(cx: &mut App) -> bool {
        Self::run_once_with(cx, |cx| {
            crate::hotkeys::shutdown(cx);
            plugin_ui::PluginHost::shutdown(cx);
            crate::tray::prepare_quit();
            #[cfg(any(windows, unix))]
            crate::single_instance::release();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn app_shutdown_runs_exactly_once(cx: &mut gpui::TestAppContext) {
        let mut calls = 0usize;
        cx.update(|cx| {
            assert!(AppShutdown::run_once_with(cx, |_| calls += 1));
            assert!(!AppShutdown::run_once_with(cx, |_| calls += 1));
            assert!(!AppShutdown::run_once_with(cx, |_| calls += 1));
        });
        assert_eq!(calls, 1);
    }
}
