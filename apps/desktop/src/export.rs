//! Saving exports where the user chooses, as the original does: the system's save dialog opens
//! in the downloads folder with a suggested file name.

use std::path::PathBuf;

use gpui::Context;

use crate::i18n::Strings;

/// The folder the save dialog starts in.
pub fn default_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| ".".into())
}

/// The message after an export: where it went, or why it failed; `None` when cancelled.
pub fn outcome(result: Option<std::io::Result<PathBuf>>, strings: &Strings) -> Option<String> {
    result.map(|result| match result {
        Ok(path) => strings.exported_to.replace("{path}", &path.display().to_string()),
        Err(err) => err.to_string(),
    })
}

/// Asks where to save `contents` under the suggested `name`, writes it there, and hands the
/// outcome to `done`: `None` when the dialog was cancelled.
pub fn save_as<V: 'static>(
    name: String,
    contents: Vec<u8>,
    cx: &mut Context<V>,
    done: impl FnOnce(&mut V, Option<std::io::Result<PathBuf>>, &mut Context<V>) + 'static,
) {
    let chosen = cx.prompt_for_new_path(&default_dir(), Some(&name));
    cx.spawn(async move |this, cx| {
        let path = chosen.await.ok().and_then(Result::ok).flatten();
        let result = path.map(|path| std::fs::write(&path, &contents).map(|()| path));
        this.update(cx, |view, cx| done(view, result, cx)).ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;

    #[test]
    fn outcomes() {
        assert_eq!(outcome(None, &EN), None);
        let saved = outcome(Some(Ok(PathBuf::from("/tmp/a.csv"))), &EN).unwrap();
        assert!(saved.contains("/tmp/a.csv"));
    }
}
