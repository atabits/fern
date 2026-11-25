//! Device-local main window bounds and maximized state persistence.
//! Kept outside synced preferences because screen layouts vary across devices.

use fern_core::Store;
use gpui::{App, Bounds, Pixels, Window, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

const SETTING_KEY: &str = "local_main_window";
pub const DEFAULT_WIDTH: f32 = 1280.;
pub const DEFAULT_HEIGHT: f32 = 760.;
pub const MIN_WIDTH: f32 = 900.;
pub const MIN_HEIGHT: f32 = 400.;
const MAX_DIMENSION: f32 = 4096.;
const MIN_VISIBLE_PATCH_W: f32 = 120.;
const MIN_VISIBLE_PATCH_H: f32 = 40.;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MainWindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl MainWindowState {
    pub fn clamped_size(&self) -> gpui::Size<Pixels> {
        let w = if self.width.is_finite() {
            self.width.clamp(MIN_WIDTH, MAX_DIMENSION)
        } else {
            DEFAULT_WIDTH
        };
        let h = if self.height.is_finite() {
            self.height.clamp(MIN_HEIGHT, MAX_DIMENSION)
        } else {
            DEFAULT_HEIGHT
        };
        size(px(w), px(h))
    }

    pub fn candidate_bounds(&self) -> Option<Bounds<Pixels>> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return None;
        }
        Some(Bounds::new(point(px(self.x), px(self.y)), self.clamped_size()))
    }
}

pub fn is_visible_on_displays(candidate: Bounds<Pixels>, displays: &[Bounds<Pixels>]) -> bool {
    displays.iter().any(|screen| {
        candidate.origin.x + px(MIN_VISIBLE_PATCH_W) > screen.origin.x
            && candidate.origin.x + px(MIN_VISIBLE_PATCH_W) <= screen.right()
            && candidate.origin.y >= screen.origin.y
            && candidate.origin.y + px(MIN_VISIBLE_PATCH_H) <= screen.bottom()
    })
}

pub fn load(store: &Store) -> Option<MainWindowState> {
    let raw = store.setting(SETTING_KEY).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(store: &Store, state: &MainWindowState) {
    if crate::is_dev_state() {
        return;
    }
    if let Ok(json) = serde_json::to_string(state) {
        let _ = store.set_setting(SETTING_KEY, &json);
    }
}

pub fn capture(window: &Window) -> MainWindowState {
    let wb = window.window_bounds();
    let bounds = wb.get_bounds();
    MainWindowState {
        x: bounds.origin.x.into(),
        y: bounds.origin.y.into(),
        width: bounds.size.width.into(),
        height: bounds.size.height.into(),
        maximized: matches!(wb, WindowBounds::Maximized(_)),
    }
}

pub fn initial_window_bounds(store: &Store, cx: &App) -> WindowBounds {
    let default_bounds = || WindowBounds::Windowed(Bounds::centered(None, size(px(DEFAULT_WIDTH), px(DEFAULT_HEIGHT)), cx));
    if crate::is_dev_state() {
        return default_bounds();
    }
    let Some(saved) = load(store) else {
        return default_bounds();
    };
    let Some(bounds) = saved.candidate_bounds() else {
        return default_bounds();
    };
    let display_bounds: Vec<Bounds<Pixels>> = cx.displays().into_iter().map(|d| d.bounds()).collect();
    if !is_visible_on_displays(bounds, &display_bounds) {
        return default_bounds();
    }
    if saved.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_json_roundtrip() {
        let state = MainWindowState {
            x: 120.5,
            y: 48.0,
            width: 1400.0,
            height: 820.0,
            maximized: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        let decoded: MainWindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, state);

        // Missing `maximized` defaults to false.
        let legacy: MainWindowState =
            serde_json::from_str(r#"{"x":10,"y":20,"width":1000,"height":700}"#).unwrap();
        assert!(!legacy.maximized);
    }

    #[test]
    fn clamped_size_handles_nan_and_bounds() {
        let bad = MainWindowState {
            x: 0.,
            y: 0.,
            width: f32::NAN,
            height: 120.,
            maximized: false,
        };
        assert_eq!(bad.clamped_size(), size(px(DEFAULT_WIDTH), px(MIN_HEIGHT)));

        let huge = MainWindowState {
            x: 0.,
            y: 0.,
            width: 99999.,
            height: 8000.,
            maximized: false,
        };
        assert_eq!(huge.clamped_size(), size(px(MAX_DIMENSION), px(MAX_DIMENSION)));

        let bad_origin = MainWindowState {
            x: f32::INFINITY,
            y: 10.,
            width: 1100.,
            height: 700.,
            maximized: false,
        };
        assert!(bad_origin.candidate_bounds().is_none());
    }

    #[test]
    fn visibility_check_requires_onscreen_header() {
        let displays = vec![Bounds::new(point(px(0.), px(0.)), size(px(1920.), px(1080.)))];
        let visible = Bounds::new(point(px(100.), px(100.)), size(px(1280.), px(760.)));
        assert!(is_visible_on_displays(visible, &displays));

        let offscreen = Bounds::new(point(px(2500.), px(100.)), size(px(1280.), px(760.)));
        assert!(!is_visible_on_displays(offscreen, &displays));

        let above_top = Bounds::new(point(px(100.), px(-20.)), size(px(1280.), px(760.)));
        assert!(!is_visible_on_displays(above_top, &displays));
    }
}
