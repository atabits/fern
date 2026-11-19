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

        Self {
            bundled_light_ids,
            bundled_dark_ids,
            bundled_defaults,
            custom_ids: Vec::new(),
            by_id,
            errors: Vec::new(),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Theme>> {
        self.by_id.get(id).cloned()
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
            .cloned()
            .expect("bundled fallback theme is always registered")
    }

    pub fn is_bundled_id(&self, id: &str) -> bool {
        self.bundled_defaults.contains_key(id)
    }

    pub fn bundled_light(&self) -> Vec<Arc<Theme>> {
        self.bundled_light_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect()
    }

    pub fn bundled_dark(&self) -> Vec<Arc<Theme>> {
        self.bundled_dark_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect()
    }

    pub fn custom_themes(&self) -> Vec<Arc<Theme>> {
        self.custom_ids
            .iter()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect()
    }

    pub fn custom_dark(&self) -> Vec<Arc<Theme>> {
        self.custom_themes()
            .into_iter()
            .filter(|theme| theme.is_dark())
            .collect()
    }

    pub fn all_dark(&self) -> Vec<Arc<Theme>> {
        let mut dark = self.bundled_dark();
        dark.extend(self.custom_dark());
        dark
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Replaces all user themes while preserving bundled defaults for un-overridden presets.
    pub fn replace_user_themes(&mut self, themes: Vec<Theme>, errors: Vec<String>) {
        self.by_id.clear();
        for (id, theme) in &self.bundled_defaults {
            self.by_id.insert(id.clone(), theme.clone());
        }
        self.custom_ids.clear();

        for theme in themes {
            let id = theme.id_shared();
            let arc = Arc::new(theme);
            if !self.is_bundled_id(id.as_ref()) {
                self.custom_ids.retain(|existing| existing != &id);
                self.custom_ids.push(id.clone());
            }
            self.by_id.insert(id, arc);
        }

        self.errors = errors;
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
        assert_eq!(registry.all_dark().len(), DARK_PRESETS.len() + 1);

        // Wrong-appearance resolution falls back within requested appearance.
        assert_eq!(registry.resolve(false, Some("ocean")).id(), "grey");

        // Clearing user themes restores the original bundled preset.
        registry.replace_user_themes(Vec::new(), Vec::new());
        let restored = registry.resolve(true, Some("night"));
        assert!(!restored.is_custom());
        assert_eq!(restored.name(), "night");
        assert!(registry.custom_themes().is_empty());
    }
}
