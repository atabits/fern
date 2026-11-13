//! Sticky-note preferences, with the original's twelve verified color pairs.
use fern_core::Store;
use serde::{Deserialize, Serialize};

pub const PALETTE: [(u32, u32); 12] = [
    (0xfff2aa, 0xffed92), (0xffd4c4, 0xffcebe), (0xffadad, 0xffa5a5),
    (0xcaf1ff, 0xbeedff), (0xc4d4ff, 0xbbceff), (0xd4c9fc, 0xd0c3ff),
    (0xafebbb, 0xa0e7af), (0xffffff, 0xf9f9f9), (0xeeeeee, 0xe8e8e8),
    (0x3d3f45, 0x383a3e), (0x152240, 0x131f3b), (0x1f2126, 0x1a1c20),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StickySettings {
    pub color: usize,
    pub opacity: u8,
    pub font: usize,
    pub pin_default: bool,
}

impl Default for StickySettings {
    fn default() -> Self { Self { color: 0, opacity: 100, font: 1, pin_default: false } }
}

impl StickySettings {
    pub fn load(store: &Store) -> Self {
        let mut settings: Self = store.setting("sticky_settings").ok().flatten()
            .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        settings.color = settings.color.min(PALETTE.len() - 1);
        settings.opacity = settings.opacity.clamp(10, 100);
        settings.font = settings.font.min(3);
        settings
    }

    pub fn save(self, store: &Store) -> anyhow::Result<()> {
        store.set_setting("sticky_settings", &serde_json::to_string(&self)?)
    }

    pub fn font_px(self) -> f32 { [12., 14., 18., 22.][self.font.min(3)] }
    pub fn foreground(self) -> u32 { if self.color >= 9 { 0xf5f5f5 } else { 0x24262b } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistence_and_invalid_preferences() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(StickySettings::load(&store), StickySettings::default());
        let saved = StickySettings { color: 10, opacity: 60, font: 3, pin_default: true };
        saved.save(&store).unwrap();
        assert_eq!(StickySettings::load(&store), saved);
        store.set_setting("sticky_settings", r#"{"color":99,"opacity":0,"font":99}"#).unwrap();
        let settings = StickySettings::load(&store);
        assert_eq!((settings.color, settings.opacity, settings.font), (11, 10, 3));
        store.set_setting("sticky_settings", "invalid").unwrap();
        assert_eq!(StickySettings::load(&store), StickySettings::default());
    }
}
