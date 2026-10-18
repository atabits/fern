//! Loads key bindings from a JSON keymap, in the format Zed uses:
//! a list of sections, each with an optional key `context` and `bindings` from keystrokes to
//! action names.

use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui::{App, AssetSource, KeyBinding, KeyBindingContextPredicate};
use serde::Deserialize;

pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default.json";

/// Actions shown in the shortcut list, in order; text editing keys are left out.
pub const LISTED_ACTIONS: [&str; 23] = [
    "fern::NewTask",
    "fern::Search",
    "fern::ToggleSidebar",
    "fern::ShowInbox",
    "fern::ShowToday",
    "fern::ShowTomorrow",
    "fern::ShowNext7Days",
    "fern::ShowShortcuts",
    "fern::Quit",
    "task_list::SelectNext",
    "task_list::SelectPrevious",
    "task_list::ToggleComplete",
    "task_list::DeleteTask",
    "task_list::PinTask",
    "task_list::SetDate",
    "task_list::ClearDate",
    "task_list::SetToday",
    "task_list::SetTomorrow",
    "task_list::SetNextWeek",
    "task_list::PriorityNone",
    "task_list::PriorityLow",
    "task_list::PriorityMedium",
    "task_list::PriorityHigh",
];

/// The keys bound to each listed action, as they read on screen ("Ctrl+N", "↓"), in the order
/// of `LISTED_ACTIONS`.
pub fn shortcut_list(source: &str) -> Result<Vec<(&'static str, Vec<String>)>> {
    let sections: Vec<KeymapSection> = serde_json::from_str(source).context("invalid keymap JSON")?;
    Ok(LISTED_ACTIONS
        .iter()
        .map(|action| {
            let keys = sections
                .iter()
                .flat_map(|s| s.bindings.iter())
                .filter(|(_, a)| a.as_str() == *action)
                .map(|(keys, _)| display_keys(keys))
                .collect();
            (*action, keys)
        })
        .collect())
}

/// "secondary-n" → "Ctrl+N" (⌘N on macOS), "down" → "↓", "space" → "Space".
pub fn display_keys(keystroke: &str) -> String {
    // A sequence ("tab m") reads "Tab, M".
    if keystroke.contains(' ') {
        return keystroke.split(' ').map(display_keys).collect::<Vec<_>>().join(", ");
    }
    keystroke
        .split('-')
        .map(|part| match part {
            "secondary" => if cfg!(target_os = "macos") { "⌘".to_owned() } else { "Ctrl".to_owned() },
            "ctrl" => "Ctrl".to_owned(),
            "shift" => "Shift".to_owned(),
            "alt" => "Alt".to_owned(),
            "down" => "↓".to_owned(),
            "up" => "↑".to_owned(),
            "left" => "←".to_owned(),
            "right" => "→".to_owned(),
            "delete" => "Del".to_owned(),
            other => {
                let mut chars = other.chars();
                chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
            }
        })
        .collect::<Vec<_>>()
        .join(if cfg!(target_os = "macos") { "" } else { "+" })
}

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
    #[test]
    fn shortcut_list_reads_the_keymap() {
        let list = super::shortcut_list(include_str!("../../../assets/keymaps/default.json")).unwrap();
        assert_eq!(list.len(), super::LISTED_ACTIONS.len());
        let keys = |action: &str| list.iter().find(|(a, _)| *a == action).unwrap().1.clone();
        assert!(!keys("fern::NewTask").is_empty());
        let mut next = keys("task_list::SelectNext");
        next.sort();
        assert_eq!(next, ["J", "↓"]);
        assert_eq!(keys("task_list::PinTask"), ["Tab, P"]);
        if !cfg!(target_os = "macos") {
            assert_eq!(super::display_keys("secondary-shift-p"), "Ctrl+Shift+P");
        }
    }

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
