//! Loads key bindings from a JSON keymap, in the format Zed uses:
//! a list of sections, each with an optional key `context` and `bindings` from keystrokes to
//! action names.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui::{
    Action as _, App, AssetSource, Global, KeyBinding, KeyBindingContextPredicate,
    KeyBindingMetaIndex, Keystroke, NoAction,
};
use serde::Deserialize;

/// Precedence layers matching GPUI's `KeyBindingMetaIndex` convention (lower index wins).
const USER_META: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
const DEFAULT_META: KeyBindingMetaIndex = KeyBindingMetaIndex(1);

pub const DEFAULT_KEYMAP_PATH: &str = "keymaps/default.json";
pub const USER_KEYMAP_FILE: &str = "keymap.json";
pub const USER_KEYMAP_TEMPLATE: &str = r#"// Fern user keymap — overrides the built-in default bindings.
// After saving, click "Reload keymap" in the Shortcuts panel (Ctrl+/).
//
// Example:
// [
//   {
//     "context": "Workspace && !TextInput",
//     "bindings": {
//       "escape": "fern::MinimizeWindow"
//     }
//   },
//   {
//     "context": "TaskList",
//     "bindings": {
//       "space": null
//     }
//   }
// ]
//
// Common contexts: Workspace, Tasks, TaskList, Calendar, CalendarYear,
// Matrix, Habits, Focus, Countdown, Settings, TextInput.
// Set a key to null to unbind it in that context.
[
]
"#;

#[derive(Default)]
struct KeymapErrors(Vec<String>);

impl Global for KeymapErrors {}

#[derive(Default)]
struct CachedShortcuts(Vec<(&'static str, Vec<String>)>);

impl Global for CachedShortcuts {}

/// Any errors collected during the last keymap load or reload.
pub fn errors(cx: &App) -> &[String] {
    cx.try_global::<KeymapErrors>().map_or(&[], |state| state.0.as_slice())
}

pub fn set_errors(errors: Vec<String>, cx: &mut App) {
    cx.set_global(KeymapErrors(errors));
}

/// Cached display list of active shortcuts from the last keymap load/reload (no disk I/O).
pub fn cached_shortcuts(cx: &App) -> &[(&'static str, Vec<String>)] {
    cx.try_global::<CachedShortcuts>()
        .map_or(&[], |state| state.0.as_slice())
}

pub fn user_keymap_path() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join(USER_KEYMAP_FILE))
}

/// Ensures `<data_dir>/keymap.json` exists with a valid empty JSON array if absent, returning its path.
pub fn ensure_user_keymap_file() -> Result<PathBuf> {
    let path = user_keymap_path()?;
    ensure_user_keymap_file_at(&path)?;
    Ok(path)
}

fn ensure_user_keymap_file_at(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        std::fs::write(path, USER_KEYMAP_TEMPLATE)
            .with_context(|| format!("cannot create {}", path.display()))?;
    }
    Ok(())
}

/// Clears active bindings and reloads `default.json` plus optional `<data_dir>/keymap.json`,
/// storing any parse or action errors and the resolved shortcut list in app state.
pub fn reload(assets: &dyn AssetSource, cx: &mut App) -> Result<Vec<String>> {
    let mut read_errors = Vec::new();
    let user_keymap = match user_keymap_path() {
        Ok(path) => match std::fs::read_to_string(&path) {
            Ok(contents) => Some(contents),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                read_errors.push(format!("{USER_KEYMAP_FILE}: {err}"));
                None
            }
        },
        Err(err) => {
            read_errors.push(format!("{USER_KEYMAP_FILE}: {err}"));
            None
        }
    };
    cx.clear_key_bindings();
    match load_with_user(assets, user_keymap.as_deref(), cx) {
        Ok(mut errors) => {
            read_errors.append(&mut errors);
            set_errors(read_errors.clone(), cx);
            Ok(read_errors)
        }
        Err(err) => {
            read_errors.push(err.to_string());
            set_errors(read_errors, cx);
            Err(err)
        }
    }
}

/// Actions shown in the shortcut list, in order; text editing keys are left out.
pub const LISTED_ACTIONS: [&str; 31] = [
    "fern::NewTask",
    "fern::Search",
    "fern::ToggleSidebar",
    "fern::ShowInbox",
    "fern::ShowToday",
    "fern::ShowTomorrow",
    "fern::ShowNext7Days",
    "fern::SyncNow",
    "fern::ShowShortcuts",
    "fern::OpenSettings",
    "fern::Quit",
    "fern::DismissTransient",
    "fern::MinimizeWindow",
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
    "task_list::ToggleAllSubtasks",
    "task_list::AddSubtask",
    "task_list::Undo",
    "task_list::Redo",
];

/// The keys bound to each listed action, as they read on screen ("Ctrl+N", "↓"), in the order
/// of `LISTED_ACTIONS`.
#[cfg(test)]
pub fn shortcut_list(source: &str) -> Result<Vec<(&'static str, Vec<String>)>> {
    shortcut_list_with_user(source, None)
}

/// The keys bound to each listed action after applying optional user `keymap.json` overrides and
/// `null` unbindings on top of the default keymap.
pub fn shortcut_list_with_user(
    default_source: &str,
    user_source: Option<&str>,
) -> Result<Vec<(&'static str, Vec<String>)>> {
    let cleaned_default = theme::strip_jsonc(default_source).context("invalid keymap JSONC")?;
    let mut sections: Vec<KeymapSection> =
        serde_json::from_str(&cleaned_default).context("invalid keymap JSON")?;
    if let Some(user_source) = user_source
        && let Ok(cleaned_user) = theme::strip_jsonc(user_source)
        && !cleaned_user.trim().is_empty()
        && let Ok(user_sections) = serde_json::from_str::<Vec<KeymapSection>>(&cleaned_user)
    {
        sections.extend(user_sections);
    }

    let mut effective: Vec<(Option<String>, String, Option<String>)> = Vec::new();
    for section in sections {
        let context = section.context.map(|c| c.trim().to_owned());
        for (keys, action) in section.bindings {
            if context.is_none() {
                effective.retain(|(_, existing_keys, _)| *existing_keys != keys);
            } else {
                effective.retain(|(existing_ctx, existing_keys, _)| {
                    !(existing_ctx == &context && *existing_keys == keys)
                });
            }
            effective.push((context.clone(), keys, action));
        }
    }

    Ok(LISTED_ACTIONS
        .iter()
        .map(|action| {
            let mut keys = Vec::new();
            for (_, keystroke, bound_action) in &effective {
                if bound_action.as_deref() == Some(*action) {
                    let display = display_keys(keystroke);
                    if !keys.contains(&display) {
                        keys.push(display);
                    }
                }
            }
            (*action, keys)
        })
        .collect())
}

fn format_key_name(key: &str) -> String {
    match key {
        "down" => "↓".to_owned(),
        "up" => "↑".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "delete" => "Del".to_owned(),
        "escape" => "Esc".to_owned(),
        "space" => "Space".to_owned(),
        "enter" => "Enter".to_owned(),
        "tab" => "Tab".to_owned(),
        "backspace" => "Backspace".to_owned(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        }
    }
}

fn display_single_keystroke(raw: &str) -> String {
    if let Ok(ks) = Keystroke::parse(raw) {
        let mut parts: Vec<String> = Vec::new();
        if cfg!(target_os = "macos") {
            if ks.modifiers.control {
                parts.push("⌃".to_owned());
            }
            if ks.modifiers.alt {
                parts.push("⌥".to_owned());
            }
            if ks.modifiers.shift {
                parts.push("⇧".to_owned());
            }
            if ks.modifiers.platform {
                parts.push("⌘".to_owned());
            }
            parts.push(format_key_name(&ks.key));
            return parts.concat();
        }
        if ks.modifiers.control {
            parts.push("Ctrl".to_owned());
        }
        if ks.modifiers.alt {
            parts.push("Alt".to_owned());
        }
        if ks.modifiers.shift {
            parts.push("Shift".to_owned());
        }
        if ks.modifiers.platform {
            parts.push("Win".to_owned());
        }
        parts.push(format_key_name(&ks.key));
        return parts.join("+");
    }
    // Fallback for any unparseable keystroke token.
    format_key_name(raw)
}

/// "secondary-n" → "Ctrl+N" (`⌘N` on macOS), "down" → "↓", "space" → "Space".
pub fn display_keys(keystroke: &str) -> String {
    let chords: Vec<String> = keystroke
        .split_whitespace()
        .map(display_single_keystroke)
        .collect();
    chords.join(", ")
}

#[derive(Deserialize)]
struct KeymapSection {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    bindings: BTreeMap<String, Option<String>>,
}

/// Loads the default keymap from the assets. Returns the bindings that could not be loaded,
/// so a typo in the keymap does not break the rest of it.
pub fn load_default(assets: &dyn AssetSource, cx: &mut App) -> Result<Vec<String>> {
    load_with_user(assets, None, cx)
}

/// Loads the default keymap from assets and layers an optional user `keymap.json` on top of it.
/// Any syntax or binding error in the user keymap is reported in the returned error list without
/// breaking the default keymap.
pub fn load_with_user(
    assets: &dyn AssetSource,
    user_source: Option<&str>,
    cx: &mut App,
) -> Result<Vec<String>> {
    let source = assets
        .load(DEFAULT_KEYMAP_PATH)?
        .with_context(|| format!("{DEFAULT_KEYMAP_PATH} is missing"))?;
    let default_str = std::str::from_utf8(&source)?;
    let mut errors = load_with_meta(default_str, DEFAULT_META, cx)?;
    let trimmed_user = user_source.map(str::trim).filter(|s| !s.is_empty());
    if let Some(user_src) = trimmed_user {
        match load_with_meta(user_src, USER_META, cx) {
            Ok(user_errors) => errors.extend(user_errors),
            Err(err) => errors.push(format!("{USER_KEYMAP_FILE}: {err}")),
        }
    }
    if let Ok(shortcuts) = shortcut_list_with_user(default_str, trimmed_user) {
        cx.set_global(CachedShortcuts(shortcuts));
    }
    Ok(errors)
}

pub fn load(source: &str, cx: &mut App) -> Result<Vec<String>> {
    load_with_meta(source, USER_META, cx)
}

fn load_with_meta(source: &str, meta: KeyBindingMetaIndex, cx: &mut App) -> Result<Vec<String>> {
    let cleaned = theme::strip_jsonc(source).context("invalid keymap JSONC")?;
    let sections: Vec<KeymapSection> = serde_json::from_str(&cleaned).context("invalid keymap JSON")?;
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
            let action = match action_name {
                Some(action_name) => match cx.build_action(&action_name, None) {
                    Ok(action) => action,
                    Err(err) => {
                        errors.push(format!("{keystrokes}: {action_name}: {err}"));
                        continue;
                    }
                },
                None => NoAction.boxed_clone(),
            };
            match KeyBinding::load(
                &keystrokes,
                action,
                predicate.clone(),
                false,
                None,
                cx.keyboard_mapper().as_ref(),
            ) {
                Ok(binding) => bindings.push(binding.with_meta(meta)),
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
        assert_eq!(keys("fern::DismissTransient"), ["Esc"]);
        let mut next = keys("task_list::SelectNext");
        next.sort();
        assert_eq!(next, ["J", "↓"]);
        assert_eq!(keys("task_list::PinTask"), ["Tab, P"]);
        if cfg!(target_os = "macos") {
            assert_eq!(super::display_keys("secondary-shift-p"), "⇧⌘P");
            assert_eq!(super::display_keys("ctrl-shift-m"), "⌃⇧M");
            assert_eq!(super::display_keys("secondary--"), "⌘-");
        } else {
            assert_eq!(super::display_keys("secondary-shift-p"), "Ctrl+Shift+P");
            assert_eq!(super::display_keys("ctrl-shift-m"), "Ctrl+Shift+M");
            assert_eq!(super::display_keys("secondary--"), "Ctrl+-");
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

    #[gpui::test]
    fn user_keymap_overrides_unbinds_and_adds_minimize_window(cx: &mut TestAppContext) {
        let user_keymap = r#"[
            {
                "context": "Workspace && !TextInput",
                "bindings": {
                    "escape": "fern::MinimizeWindow"
                }
            },
            {
                "context": "TaskList",
                "bindings": {
                    "space": null
                }
            },
            {
                "bindings": {
                    "secondary-q": null,
                    "ctrl-shift-m": "fern::NewTask"
                }
            }
        ]"#;

        cx.update(|cx| {
            let errors = load_with_user(&assets::Assets, Some(user_keymap), cx).unwrap();
            assert!(errors.is_empty(), "user keymap errors: {errors:?}");

            // Broken user JSON reports an error without failing default bindings.
            let broken_errors = load_with_user(&assets::Assets, Some("{not valid json"), cx).unwrap();
            assert_eq!(broken_errors.len(), 1);
            assert!(broken_errors[0].contains(USER_KEYMAP_FILE));
        });

        let default_json = include_str!("../../../assets/keymaps/default.json");
        let list = shortcut_list_with_user(default_json, Some(user_keymap)).unwrap();
        let keys = |action: &str| list.iter().find(|(a, _)| *a == action).unwrap().1.clone();
        assert_eq!(keys("fern::MinimizeWindow"), ["Esc"]);
        assert_eq!(keys("fern::DismissTransient"), ["Esc"]);
        assert!(keys("fern::Quit").is_empty());
        assert!(keys("fern::NewTask").contains(&display_keys("ctrl-shift-m")));
        assert_eq!(keys("task_list::ToggleComplete"), ["Tab, M"]);
    }

    #[test]
    fn ensure_user_keymap_creates_valid_template_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("fern-keymap-test-{}", fern_core::Id::now_v7()));
        let path = dir.join(USER_KEYMAP_FILE);
        ensure_user_keymap_file_at(&path).unwrap();
        let created = std::fs::read_to_string(&path).unwrap();
        assert_eq!(created, USER_KEYMAP_TEMPLATE);
        assert!(shortcut_list_with_user(include_str!("../../../assets/keymaps/default.json"), Some(&created)).is_ok());

        let commented = r#"
            // Comment before array
            [
              {
                /* Block comment */
                "context": "Workspace && !TextInput",
                "bindings": {
                  "escape": "fern::MinimizeWindow" // Inline comment
                }
              }
            ]
        "#;
        let list = shortcut_list_with_user(include_str!("../../../assets/keymaps/default.json"), Some(commented)).unwrap();
        let minimize = list.iter().find(|(a, _)| *a == "fern::MinimizeWindow").unwrap().1.clone();
        assert_eq!(minimize, ["Esc"]);

        let custom = r#"[{"bindings": {"escape": "fern::MinimizeWindow"}}]"#;
        std::fs::write(&path, custom).unwrap();
        ensure_user_keymap_file_at(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
