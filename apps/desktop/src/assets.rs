use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

/// Иконки вшиваются в бинарник, чтобы приложение не зависело от файлов рядом с ним.
/// Таблица генерируется в `build.rs` по содержимому `assets/icons`.
pub struct Assets;

const ICONS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/icons.rs"));

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| (*p).into()).collect())
    }
}
