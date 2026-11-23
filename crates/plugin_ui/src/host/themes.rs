use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui::{App, AppContext as _, BorrowAppContext as _};
use plugins::PluginInfo;
use plugins::manifest::resolve_inside_dir;
use theme::{Theme, ThemeRegistry};

use super::PluginHost;

pub(super) type ThemeSourceSignature = Vec<(String, PathBuf, String)>;

impl PluginHost {
    /// Loads `themes` contributions from all enabled plugins into [`ThemeRegistry`],
    /// preserving any user-defined custom themes already loaded from `<data_dir>/themes`.
    pub fn sync_plugin_themes(cx: &mut App) {
        if !cx.has_global::<ThemeRegistry>() {
            return;
        }
        let Some(host) = cx.try_global::<Self>() else {
            return;
        };
        let sig = theme_contribution_signature(&host.plugins);
        let (plugin_themes, errors) = if host.loaded_theme_signature.as_ref() == Some(&sig) {
            (
                host.cached_plugin_themes.clone(),
                host.plugin_theme_errors.clone(),
            )
        } else if sig.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let plugins = Arc::clone(&host.plugins);
            collect_plugin_themes(&plugins)
        };
        // Invalidate any in-flight async theme load so it cannot overwrite this synchronous sync.
        host.theme_sync_seq.fetch_add(1, Ordering::AcqRel);
        apply_plugin_themes(sig, plugin_themes, errors, cx);
    }
}

pub(super) fn theme_contribution_signature(plugins: &[PluginInfo]) -> ThemeSourceSignature {
    plugins
        .iter()
        .filter(|p| p.enabled && !p.manifest.contributions.themes.is_empty())
        .map(|p| (p.manifest.id.clone(), p.dir.clone(), p.manifest_hash.clone()))
        .collect()
}

fn apply_plugin_themes(
    signature: ThemeSourceSignature,
    plugin_themes: Vec<Theme>,
    errors: Vec<String>,
    cx: &mut App,
) {
    if !cx.has_global::<ThemeRegistry>() {
        return;
    }
    let Some(host) = cx.try_global::<PluginHost>() else {
        return;
    };
    let prev_plugin_ids = host.loaded_plugin_theme_ids.clone();
    let prev_errors = host.plugin_theme_errors.clone();

    let next_plugin_ids: HashSet<String> =
        plugin_themes.iter().map(|t| t.id().to_owned()).collect();

    let registry = ThemeRegistry::global(cx);
    let mut combined: Vec<Theme> = registry
        .custom_themes()
        .iter()
        .filter(|t| !prev_plugin_ids.contains(t.id()))
        .map(|t| (**t).clone())
        .collect();
    combined.extend(plugin_themes.clone());

    let mut combined_errors: Vec<String> = registry
        .errors()
        .iter()
        .filter(|err| !prev_errors.contains(err))
        .cloned()
        .collect();
    combined_errors.extend(errors.clone());

    theme::reload_user_themes(combined, combined_errors, cx);
    cx.update_global::<PluginHost, _>(|host, _| {
        host.loaded_plugin_theme_ids = next_plugin_ids;
        host.plugin_theme_errors = errors;
        host.loaded_theme_signature = Some(signature);
        host.cached_plugin_themes = plugin_themes;
    });
}

pub(super) fn sync_plugin_themes_async(
    plugins: Arc<Vec<PluginInfo>>,
    sig: ThemeSourceSignature,
    cx: &mut App,
) {
    if !cx.has_global::<ThemeRegistry>() {
        return;
    }
    if sig.is_empty() {
        PluginHost::sync_plugin_themes(cx);
        return;
    }
    let Some((seq, signal)) = cx.try_global::<PluginHost>().map(|h| {
        (
            h.theme_sync_seq
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1),
            Arc::clone(&h.wake_signal),
        )
    }) else {
        return;
    };

    let task = cx.background_spawn(async move { collect_plugin_themes(&plugins) });
    cx.spawn(async move |cx| {
        let (themes, errors) = task.await;
        if signal.closed.load(Ordering::Acquire) {
            return;
        }
        cx.update(|cx| {
            let is_latest = cx.try_global::<PluginHost>().is_some_and(|h| {
                Arc::ptr_eq(&h.wake_signal, &signal)
                    && h.theme_sync_seq.load(Ordering::Acquire) == seq
            });
            if is_latest {
                apply_plugin_themes(sig, themes, errors, cx);
            }
        });
    })
    .detach();
}

/// Parses and validates all `themes` contributed by enabled plugins.
pub fn collect_plugin_themes(plugins: &[PluginInfo]) -> (Vec<Theme>, Vec<String>) {
    let mut themes = Vec::new();
    let mut errors = Vec::new();

    for info in plugins {
        if !info.enabled || info.manifest.contributions.themes.is_empty() {
            continue;
        }
        for contrib in &info.manifest.contributions.themes {
            if let Some(rel_path) = &contrib.path {
                match resolve_inside_dir(&info.dir, rel_path)
                    .and_then(|path| std::fs::read_to_string(&path).map_err(anyhow::Error::from))
                    .and_then(|raw| theme::load_theme_family_from_str(&raw))
                {
                    Ok((loaded, warnings)) => {
                        themes.extend(loaded);
                        for w in warnings {
                            errors.push(format!("{}: {w}", info.manifest.id));
                        }
                    }
                    Err(err) => {
                        errors.push(format!(
                            "{}: theme {:?} failed: {err:#}",
                            info.manifest.id, contrib.id
                        ));
                    }
                }
            } else if let Some(colors) = &contrib.colors {
                let doc = if colors.get("themes").is_some() || colors.get("style").is_some() {
                    colors.clone()
                } else {
                    serde_json::json!({
                        "id": contrib.id,
                        "name": contrib.name,
                        "appearance": contrib.appearance.as_deref().unwrap_or("dark"),
                        "author": info.manifest.author,
                        "style": colors,
                    })
                };
                match serde_json::to_string(&doc)
                    .map_err(anyhow::Error::from)
                    .and_then(|json| theme::load_theme_family_from_str(&json))
                {
                    Ok((loaded, warnings)) => {
                        themes.extend(loaded);
                        for w in warnings {
                            errors.push(format!("{}: {w}", info.manifest.id));
                        }
                    }
                    Err(err) => {
                        errors.push(format!(
                            "{}: inline theme {:?} failed: {err:#}",
                            info.manifest.id, contrib.id
                        ));
                    }
                }
            }
        }
    }

    (themes, errors)
}
