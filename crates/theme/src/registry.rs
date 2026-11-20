use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, BorrowAppContext as _, Global, SharedString};

use crate::{DARK_PRESETS, LIGHT_PRESETS, Theme};

/// Holds the bundled presets overlaid with any user-defined themes loaded from disk.
pub struct ThemeRegistry {
    bundled_light_ids: Vec<SharedString>,
    bundled_dark_ids: Vec<SharedString>,
    bundled_defaults: HashMap<SharedString, Arc<Theme>>,
    custom_ids: Vec<SharedString>,
    by_id: HashMap<SharedString, Arc<Theme>>,
    cached_bundled_light: Vec<Arc<Theme>>,
    cached_bundled_dark: Vec<Arc<Theme>>,
    cached_custom_themes: Vec<Arc<Theme>>,
    cached_custom_dark: Vec<Arc<Theme>>,
    errors: Vec<String>,
}

impl Global for ThemeRegistry {}

impl ThemeRegistry {
    fn new() -> Self {
        let mut bundled_light_ids = Vec::with_capacity(LIGHT_PRESETS.len());
        let mut bundled_dark_ids = Vec::with_capacity(DARK_PRESETS.len());
        let mut bundled_defaults = HashMap::with_capacity(LIGHT_PRESETS.len() + DARK_PRESETS.len());
        let mut by_id = HashMap::with_capacity(LIGHT_PRESETS.len() + DARK_PRESETS.len());

        for preset in LIGHT_PRESETS.iter() {
            let theme = Arc::new(Theme::from_preset(preset));
            let id = theme.id_shared();
            bundled_light_ids.push(id.clone());
            bundled_defaults.insert(id.clone(), theme.clone());
            by_id.insert(id, theme);
        }

        for preset in DARK_PRESETS.iter() {
            let theme = Arc::new(Theme::from_preset(preset));
            let id = theme.id_shared();
            bundled_dark_ids.push(id.clone());
            bundled_defaults.insert(id.clone(), theme.clone());
            by_id.insert(id, theme);
        }

        let mut registry = Self {
            bundled_light_ids,
            bundled_dark_ids,
            bundled_defaults,
            custom_ids: Vec::new(),
            by_id,
            cached_bundled_light: Vec::new(),
            cached_bundled_dark: Vec::new(),
            cached_custom_themes: Vec::new(),
            cached_custom_dark: Vec::new(),
            errors: Vec::new(),
        };
        registry.rebuild_caches();
        registry
    }

    fn rebuild_caches(&mut self) {
        self.cached_bundled_light = self
            .bundled_light_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect();
        self.cached_bundled_dark = self
            .bundled_dark_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect();
        self.cached_custom_themes = self
            .custom_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect();
        self.cached_custom_dark = self
            .cached_custom_themes
            .iter()
            .filter(|theme| theme.is_dark())
            .cloned()
            .collect();
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Resolves `id` if present and matching `dark`; otherwise falls back to `"night"` or `"grey"`.
    pub fn resolve(&self, dark: bool, id: Option<&str>) -> Arc<Theme> {
        if let Some(theme) = id.and_then(|id| self.by_id.get(id))
            && theme.is_dark() == dark
        {
            return theme.clone();
        }
        let fallback_id = if dark { DARK_PRESETS[0].id } else { LIGHT_PRESETS[0].id };
        self.by_id
            .get(fallback_id)
            .filter(|theme| theme.is_dark() == dark)
            .or_else(|| self.bundled_defaults.get(fallback_id))
            .cloned()
            .expect("bundled fallback theme is always registered")
    }

    pub fn is_bundled_id(&self, id: &str) -> bool {
        self.bundled_defaults.contains_key(id)
    }

    pub fn is_bundled_dark_id(&self, id: &str) -> bool {
        self.bundled_defaults.get(id).is_some_and(|theme| theme.is_dark())
    }

    pub fn bundled_light(&self) -> &[Arc<Theme>] {
        &self.cached_bundled_light
    }

    pub fn bundled_dark(&self) -> &[Arc<Theme>] {
        &self.cached_bundled_dark
    }

    pub fn custom_themes(&self) -> &[Arc<Theme>] {
        &self.cached_custom_themes
    }

    pub fn custom_dark(&self) -> &[Arc<Theme>] {
        &self.cached_custom_dark
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Replaces all user themes while preserving bundled defaults for un-overridden presets.
    /// Overrides targeting a bundled preset ID must match that preset's light/dark appearance.
    pub fn replace_user_themes(&mut self, themes: Vec<Theme>, mut errors: Vec<String>) {
        self.by_id.clear();
        for (id, theme) in &self.bundled_defaults {
            self.by_id.insert(id.clone(), theme.clone());
        }
        self.custom_ids.clear();

        for theme in themes {
            let id = theme.id_shared();
            if let Some(bundled) = self.bundled_defaults.get(id.as_ref()) {
                if bundled.is_dark() != theme.is_dark() {
                    errors.push(format!(
                        "{}: cannot override bundled {} theme `{}` with opposite appearance",
                        theme.name(),
                        if bundled.is_dark() { "dark" } else { "light" },
                        id,
                    ));
                    continue;
                }
            } else {
                self.custom_ids.retain(|existing| existing != &id);
                self.custom_ids.push(id.clone());
            }
            self.by_id.insert(id, Arc::new(theme));
        }

        self.errors = errors;
        self.rebuild_caches();
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(ThemeRegistry::new());
}

/// Replaces user themes in the global [`ThemeRegistry`].
pub fn reload_user_themes(themes: Vec<Theme>, errors: Vec<String>, cx: &mut App) {
    if !cx.has_global::<ThemeRegistry>() {
        init(cx);
    }
    cx.update_global::<ThemeRegistry, _>(|registry, _| {
        registry.replace_user_themes(themes, errors);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_theme_family_from_str;

    #[test]
    fn custom_themes_override_bundled_and_revert_when_cleared() {
        let mut registry = ThemeRegistry::new();
        assert_eq!(registry.bundled_light().len(), LIGHT_PRESETS.len());
        assert_eq!(registry.bundled_dark().len(), DARK_PRESETS.len());
        assert!(registry.custom_themes().is_empty());

        let json = r##"{
            "name": "Test Family",
            "themes": [
                {
                    "id": "night",
                    "name": "Night Override",
                    "appearance": "dark",
                    "style": { "accent": "#ff5500" }
                },
                {
                    "id": "ocean",
                    "name": "Ocean",
                    "appearance": "dark",
                    "style": { "background": "#0f172a", "accent": "#38bdf8" }
                },
                {
                    "id": "ocean",
                    "name": "Ocean v2",
                    "appearance": "dark",
                    "style": { "background": "#0b1120", "accent": "#7dd3fc" }
                }
            ]
        }"##;
        let (themes, warnings) = load_theme_family_from_str(json).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");

        registry.replace_user_themes(themes, Vec::new());

        // Bundled override replaces in place without duplicating in custom_themes.
        let night = registry.resolve(true, Some("night"));
        assert!(night.is_custom());
        assert_eq!(night.name(), "Night Override");
        assert_eq!(registry.custom_themes().len(), 1);
        assert_eq!(registry.custom_themes()[0].id(), "ocean");
        assert_eq!(registry.custom_themes()[0].name(), "Ocean v2");
        assert_eq!(registry.bundled_dark().len() + registry.custom_dark().len(), DARK_PRESETS.len() + 1);

        // Wrong-appearance resolution falls back within requested appearance.
        assert_eq!(registry.resolve(false, Some("ocean")).id(), "grey");

        // Overriding a bundled theme with the opposite appearance is rejected and logged.
        let bad_override = r##"{
            "id": "night",
            "name": "Light Night",
            "appearance": "light",
            "style": { "background": "#ffffff" }
        }"##;
        let (bad_themes, _) = load_theme_family_from_str(bad_override).unwrap();
        registry.replace_user_themes(bad_themes, Vec::new());
        assert!(registry.resolve(true, Some("night")).is_dark());
        assert!(!registry.resolve(true, Some("night")).is_custom());
        assert_eq!(registry.errors().len(), 1);

        // Clearing user themes restores the original bundled preset.
        registry.replace_user_themes(Vec::new(), Vec::new());
        let restored = registry.resolve(true, Some("night"));
        assert!(!restored.is_custom());
        assert_eq!(restored.name(), "night");
        assert!(registry.custom_themes().is_empty());
    }
}
