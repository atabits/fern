//! Loads key bindings from a JSON keymap, in the format Zed uses:
//! a list of sections, each with an optional key `context` and `bindings` from keystrokes to
//! action names.

use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui::{App, AssetSource, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;

pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default.json";

#[derive(Deserialize)]
struct KeymapSection {
    #[serde(default)]
    context: Option<String>,
    bindings: BTreeMap<String, String>,
}

/// Loads the default keymap from the assets. Returns the bindings that could not be loaded,
/// so a typo in the keymap does not break the rest of it.
pub fn load_default(assets: &dyn AssetSource, cx: &mut App) -> Result<Vec<String>> {
    let source = assets
        .load(DEFAULT_KEYMAP_PATH)?
        .with_context(|| format!("{DEFAULT_KEYMAP_PATH} is missing"))?;
    load(std::str::from_utf8(&source)?, cx)
}

pub fn load(source: &str, cx: &mut App) -> Result<Vec<String>> {
    let sections: Vec<KeymapSection> = serde_json::from_str(source).context("invalid keymap JSON")?;
    let mut bindings = Vec::new();
    let mut errors = Vec::new();
    for section in sections {
        let predicate = match section.context.as_deref().map(KeyBindingContextPredicate::parse) {
            Some(Ok(predicate)) => Some(Rc::new(predicate)),
            Some(Err(err)) => {
                errors.push(format!("context {:?}: {err}", section.context));
                continue;
            }
            None => None,
        };
        for (keystrokes, action_name) in section.bindings {
            let action = match cx.build_action(&action_name, None) {
                Ok(action) => action,
                Err(err) => {
                    errors.push(format!("{keystrokes}: {action_name}: {err}"));
                    continue;
                }
            };
            match KeyBinding::load(
                &keystrokes,
                action,
                predicate.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            ) {
                Ok(binding) => bindings.push(binding),
                Err(err) => errors.push(format!("{keystrokes}: {err}")),
            }
        }
    }
    cx.bind_keys(bindings);
    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn default_keymap_loads_without_errors(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let errors = load_default(&assets::Assets, cx).unwrap();
            assert!(errors.is_empty(), "keymap errors: {errors:?}");
        });
    }

    #[gpui::test]
    fn unknown_actions_are_reported(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let errors = load(r#"[{"bindings": {"ctrl-x": "fern::NoSuchAction"}}]"#, cx).unwrap();
            assert_eq!(errors.len(), 1);
        });
    }
}
