//! Bundled assets: icons, keymaps and the UI font. Release builds embed the files; debug builds read them from
//! the checkout, so edits show up on the next launch without a rebuild.

use std::borrow::Cow;

use anyhow::Context as _;
use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "icons/**/*"]
#[include = "icon/icon-64.png"]
#[include = "keymaps/**/*"]
#[include = "fonts/**/*.ttf"]
#[exclude = "*.DS_Store"]
pub struct Assets;

impl Assets {
    /// Loads the bundled UI font, IBM Plex Sans (SIL Open Font License, `assets/fonts/OFL.txt`),
    /// which GPUI uses as the system UI font on Linux; without it there, text falls back to a
    /// font without the weights Fern asks for.
    pub fn load_fonts(&self, cx: &mut gpui::App) -> Result<()> {
        let fonts = Self::iter()
            .filter(|path| path.starts_with("fonts/") && path.ends_with(".ttf"))
            .filter_map(|path| Self::get(&path).map(|file| file.data))
            .collect::<Vec<_>>();
        cx.text_system().add_fonts(fonts)
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Self::get(path).map(|file| Some(file.data)).with_context(|| format!("loading asset {path:?}"))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter().filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}
