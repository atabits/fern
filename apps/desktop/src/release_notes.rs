//! The versioned Markdown shown before an update and after the new binary starts.

use anyhow::Result;
use fern_core::Store;
use serde::{Deserialize, Serialize};
use ui::prelude::*;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUNDLED_NOTES: &str = include_str!(concat!(env!("OUT_DIR"), "/release-notes.md"));
const LAST_STARTED_VERSION: &str = "last_started_version";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseNotes {
    pub version: String,
    pub url: String,
    pub markdown: String,
}

impl ReleaseNotes {
    pub fn new(version: &str, url: &str, body: &str) -> Self {
        Self {
            version: version.into(),
            url: url.into(),
            markdown: changes(body, version).into(),
        }
    }

    /// Save the exact preview before the installer can launch the next process. The updater's
    /// download directory is cleared on startup, so the notes live with the local settings.
    pub fn cache_for_restart(&self, store: &Store) -> Result<()> {
        if self.markdown.is_empty() {
            return Ok(());
        }
        store.set_setting(&cache_key(&self.version), &serde_json::to_string(self)?)
    }
}

fn cache_key(version: &str) -> String {
    format!("release_notes:{version}")
}

fn cached(store: &Store, version: &str) -> Result<Option<ReleaseNotes>> {
    let Some(value) = store.setting(&cache_key(version))? else {
        return Ok(None);
    };
    let notes: ReleaseNotes = match serde_json::from_str(&value) {
        Ok(notes) => notes,
        Err(err) => {
            eprintln!("failed to parse cached release notes for {version}: {err:#}");
            return Ok(None);
        }
    };
    Ok((notes.version == version && !notes.markdown.trim().is_empty()).then_some(notes))
}

pub fn installed(store: &Store) -> Option<ReleaseNotes> {
    cached(store, CURRENT_VERSION)
        .unwrap_or_else(|err| {
            eprintln!("failed to read cached release notes: {err:#}");
            None
        })
        .or_else(bundled)
}

fn bundled() -> Option<ReleaseNotes> {
    let url = format!("https://github.com/atabits/fern/releases/tag/v{CURRENT_VERSION}");
    let notes = ReleaseNotes::new(CURRENT_VERSION, &url, BUNDLED_NOTES);
    (!notes.markdown.is_empty()).then_some(notes)
}

pub fn take_update_notice(store: &Store, first_start: bool) -> Result<Option<ReleaseNotes>> {
    // Development builds can share the normal database and must leave its notice available.
    if cfg!(all(debug_assertions, not(test))) || std::env::var_os("FERN_DEV_STATE").is_some() {
        return Ok(None);
    }
    notice_for_launch(store, CURRENT_VERSION, first_start, bundled)
}

fn notice_for_launch(
    store: &Store,
    version: &str,
    first_start: bool,
    bundled: impl FnOnce() -> Option<ReleaseNotes>,
) -> Result<Option<ReleaseNotes>> {
    let previous = store.setting(LAST_STARTED_VERSION)?;
    // Older Fern versions did not record their version. An existing database is the signal
    // on the first launch that has support for release notes.
    let upgraded = previous.as_deref().map_or(!first_start, |previous| is_newer(version, previous));
    let downgraded = previous.as_deref().is_some_and(|previous| is_newer(previous, version));
    let notes = if downgraded {
        None
    } else {
        cached(store, version)?.or_else(|| if upgraded { bundled() } else { None })
    };
    let notice = claim_notice(store, notes)?;
    if previous.as_deref() != Some(version) {
        store.set_setting(LAST_STARTED_VERSION, version)?;
    }
    Ok(notice)
}

fn claim_notice(store: &Store, notes: Option<ReleaseNotes>) -> Result<Option<ReleaseNotes>> {
    let Some(notes) = notes else {
        return Ok(None);
    };
    // Only one launch claims the notice if several processes share the same database.
    let announced = format!("release_notes_announced:{}", notes.version);
    Ok(store.insert_setting(&announced, "1")?.then_some(notes))
}

/// Whether version `a` comes after `b`; both `major.minor.patch`, anything after `-` ignored.
pub fn is_newer(a: &str, b: &str) -> bool {
    let parts = |version: &str| -> Option<Vec<u64>> {
        version.split('-').next()?.split('.').map(|part| part.parse().ok()).collect()
    };
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn changes<'a>(body: &'a str, version: &str) -> &'a str {
    // The release job appends a shared downloads and installation section. Older releases
    // predate the marker and start that section with the same opening sentence.
    let len = body.split_inclusive('\n')
        .take_while(|line| {
            line.trim_end() != "<!-- fern-release-template -->"
                && !line.starts_with("Fern is a free, open-source task manager:")
        })
        .map(str::len)
        .sum();
    let body = body[..len].trim();
    // The reader provides its own localized heading with the version.
    let heading = format!("## What's new in {version}");
    if body.lines().next().is_some_and(|line| line.trim_end() == heading) {
        return body.split_once('\n').map_or("", |(_, notes)| notes.trim());
    }
    body
}

pub fn action_button(id: &'static str, cx: &App) -> Stateful<Div> {
    let colors = cx.theme().colors();
    h_flex()
        .id(id)
        .tab_index(0)
        .flex_none()
        .min_h(px(28.))
        .px_2()
        .gap_1()
        .rounded(px(6.))
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .hover(|style| style.bg(colors.element_hover))
        .active(|style| style.bg(colors.element_selected))
        .focus_visible(|style| style.border_color(colors.accent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_only_the_changes_from_a_release_description() {
        let body = "## What's new in 0.2.0\r\n\r\n- New calendar view.\r\n\r\n## Fixes\r\n- Restored task links.";
        let expected = "- New calendar view.\r\n\r\n## Fixes\r\n- Restored task links.";
        let marked = format!("{body}\r\n\r\n<!-- fern-release-template -->\r\n## Downloads");
        let legacy = format!("{body}\r\n\r\nFern is a free, open-source task manager: tasks and more.");
        assert_eq!(changes(&marked, "0.2.0"), expected);
        assert_eq!(changes(&legacy, "0.2.0"), expected);
        assert_eq!(changes("<!-- fern-release-template -->\n## Downloads", "0.2.0"), "");
        assert_eq!(changes("- Updated shortcuts.", "0.2.0"), "- Updated shortcuts.");
    }

    #[test]
    fn announces_the_cached_target_once_and_keeps_the_same_notes() -> Result<()> {
        let path = std::env::temp_dir().join(format!("fern-release-notes-{}.db", uuid::Uuid::now_v7()));
        let notes = ReleaseNotes::new("0.2.0", "https://example.com/v0.2.0", "- Updated shortcuts.");
        {
            let store = Store::open(&path)?;
            assert_eq!(notice_for_launch(&store, "0.1.10", true, || None)?, None);
            notes.cache_for_restart(&store)?;
            assert_eq!(notice_for_launch(&store, "0.1.10", false, || None)?, None);
        }
        {
            let store = Store::open(&path)?;
            assert_eq!(notice_for_launch(&store, "0.1.11", false, || None)?, None);
            let bundled = || Some(ReleaseNotes::new("0.2.0", "https://example.com/v0.2.0", "- Draft notes."));
            assert_eq!(notice_for_launch(&store, "0.2.0", false, bundled)?, Some(notes.clone()));
        }
        {
            let store = Store::open(&path)?;
            assert_eq!(notice_for_launch(&store, "0.2.0", false, || None)?, None);
            assert_eq!(cached(&store, "0.2.0")?, Some(notes));
        }
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn uses_bundled_notes_on_upgrade_from_a_version_without_a_cache() -> Result<()> {
        let notes = ReleaseNotes::new("0.2.0", "https://example.com/v0.2.0", "- Bundled release notes.");
        let bundled = || Some(notes.clone());

        let existing = Store::open_in_memory()?;
        assert_eq!(notice_for_launch(&existing, "0.2.0", false, bundled)?, Some(notes.clone()));
        assert_eq!(notice_for_launch(&existing, "0.2.0", false, bundled)?, None);

        let fresh = Store::open_in_memory()?;
        assert_eq!(notice_for_launch(&fresh, "0.1.10", true, || None)?, None);
        assert_eq!(notice_for_launch(&fresh, "0.1.10", false, || None)?, None);
        assert_eq!(notice_for_launch(&fresh, "0.2.0", false, bundled)?, Some(notes.clone()));

        let new_install = Store::open_in_memory()?;
        assert_eq!(notice_for_launch(&new_install, "0.2.0", true, bundled)?, None);
        assert_eq!(notice_for_launch(&new_install, "0.2.0", false, bundled)?, None);
        Ok(())
    }

    #[test]
    fn uses_bundled_notes_on_upgrade_with_empty_or_malformed_cached_notes() -> Result<()> {
        let url = "https://example.com/v0.2.0";
        let empty_preview = ReleaseNotes::new("0.2.0", url, "");
        let bundled = ReleaseNotes::new("0.2.0", url, "- Bundled release notes.");
        let key = cache_key("0.2.0");

        let store = Store::open_in_memory()?;
        empty_preview.cache_for_restart(&store)?;
        assert_eq!(store.setting(&key)?, None);

        for value in [serde_json::to_string(&empty_preview)?, "{ truncated".into()] {
            let store = Store::open_in_memory()?;
            store.set_setting(&key, &value)?;
            assert_eq!(
                notice_for_launch(&store, "0.2.0", false, || Some(bundled.clone()))?,
                Some(bundled.clone()),
            );
            assert_eq!(notice_for_launch(&store, "0.2.0", false, || Some(bundled.clone()))?, None);
        }
        Ok(())
    }
}
