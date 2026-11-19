//! Bundled palettes. IDs are persisted independently of the display language.

use gpui::{Background, Hsla, linear_color_stop, linear_gradient, rgb};

#[derive(Debug)]
pub struct ThemePreset {
    pub id: &'static str,
    pub(super) accent: u32,
    pub(super) list_ink: u32,
    pub(super) base: Base,
    pub(super) gradient: Option<Gradient>,
    pub(super) preview: [u32; 2],
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Base {
    Light,
    Dark { main: u32, screen: u32 },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Gradient {
    pub stops: [u32; 2],
    /// How much of the rail's gradient shows through each working surface.
    pub lists: f32,
    pub calendar: f32,
    pub matrix: f32,
    /// Resting opacity of `[top, bottom]` white icons on the narrow rail.
    pub rail_icons: [f32; 2],
}

impl Gradient {
    pub fn surface(self, base: Hsla, opacity: f32) -> Background {
        if opacity == 0. {
            return base.into();
        }
        let [top, bottom] = self.stops.map(|c| base.blend(Hsla::from(rgb(c)).opacity(opacity)));
        vertical_gradient(top, bottom)
    }
}

pub(super) fn vertical_gradient(top: Hsla, bottom: Hsla) -> Background {
    linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.))
}

impl ThemePreset {
    const fn light(
        id: &'static str,
        accent: u32,
        list_ink: u32,
        stops: [u32; 2],
        preview: [u32; 2],
        tints: [f32; 3],
        rail_icons: [f32; 2],
    ) -> Self {
        let [lists, calendar, matrix] = tints;
        Self { id, accent, list_ink, base: Base::Light, gradient: Some(Gradient { stops, lists, calendar, matrix, rail_icons }), preview }
    }

    const fn dark(id: &'static str, accent: u32, main: u32, screen: u32, stops: [u32; 2], swatch: u32) -> Self {
        Self {
            id, accent, list_ink: 0xffffff, base: Base::Dark { main, screen },
            gradient: Some(Gradient { stops, lists: 0.20, calendar: 0., matrix: 0., rail_icons: [0.40, 0.40] }),
            preview: [swatch; 2],
        }
    }

    pub fn is_dark(&self) -> bool {
        matches!(self.base, Base::Dark { .. })
    }

    pub fn swatch(&self) -> Background {
        let [top, bottom] = self.preview_colors();
        if top == bottom { top.into() } else { vertical_gradient(top, bottom) }
    }

    pub(super) fn preview_colors(&self) -> [Hsla; 2] {
        self.preview.map(|c| Hsla::from(rgb(c)))
    }

    /// Solid preview color used for selection rings on dark swatches.
    pub fn swatch_color(&self) -> Hsla {
        rgb(self.preview[0]).into()
    }

    /// Looks up a bundled preset by exact ID across light and dark groups.
    pub(super) fn find(id: &str) -> Option<&'static Self> {
        LIGHT_PRESETS.iter().chain(&DARK_PRESETS).find(|preset| preset.id == id)
    }

    /// Unknown IDs and IDs from the other appearance fall back within the requested group.
    pub fn resolve(dark: bool, id: Option<&str>) -> &'static Self {
        let presets: &[Self] = if dark { &DARK_PRESETS } else { &LIGHT_PRESETS };
        presets.iter().find(|preset| Some(preset.id) == id).unwrap_or(&presets[0])
    }
}

pub static LIGHT_PRESETS: [ThemePreset; 11] = [
    ThemePreset { id: "grey", accent: 0x4772fa, list_ink: 0x191919, base: Base::Light, gradient: None, preview: [0xffffff; 2] },
    ThemePreset::light("classic",    0x4772fa, 0x191919, [0x4773fa, 0x6d91ff], [0x6d91ff, 0x4773fa], [0.06, 0.04, 0.06], [0.40, 0.40]),
    ThemePreset::light("turquoise",  0x1dc19d, 0x0b4444, [0x1ebc99, 0x55d3b7], [0x80dcc8, 0x27c5a2], [0.15, 0.06, 0.08], [0.40, 0.40]),
    ThemePreset::light("teal",       0x59afb2, 0x123428, [0x62b0b9, 0x91c7c8], [0x96d8d9, 0x5db4be], [0.30, 0.06, 0.06], [0.40, 0.50]),
    ThemePreset::light("matcha",     0x718c67, 0x455340, [0xa2b591, 0xc3d1ba], [0xc3d1ba, 0xa2b591], [0.30, 0.08, 0.10], [0.50, 0.60]),
    ThemePreset::light("sunshine",   0xeb9528, 0x49241a, [0xf3c268, 0xf4aa8c], [0xffd17c, 0xf9b984], [0.24, 0.06, 0.10], [0.55, 0.60]),
    ThemePreset::light("peach",      0xfa85a0, 0x351a20, [0xff79a4, 0xffafa6], [0xffb8ad, 0xff86ae], [0.24, 0.06, 0.08], [0.40, 0.50]),
    ThemePreset::light("lilac",      0x8672d3, 0x281537, [0x909ae7, 0xe5c5eb], [0xd7bedc, 0x9aa1d9], [0.30, 0.06, 0.10], [0.40, 0.50]),
    ThemePreset::light("ebony",      0x907057, 0x432e1f, [0x8d7059, 0xb8a094], [0xc7ac9d, 0xa57f61], [0.15, 0.06, 0.06], [0.40, 0.40]),
    ThemePreset::light("navy",       0x526095, 0x2d363d, [0x262f4e, 0x4f5e93], [0x3c4b80, 0x262f4e], [0.08, 0.03, 0.06], [0.40, 0.40]),
    ThemePreset::light("dark",       0x787878, 0x191919, [0x313643, 0x43454a], [0x4d5055, 0x313643], [0.06, 0.02, 0.08], [0.40, 0.40]),
];

pub static DARK_PRESETS: [ThemePreset; 6] = [
    ThemePreset { id: "night", accent: 0x4772fa, list_ink: 0xffffff, base: Base::Dark { main: 0x242424, screen: 0x1c1c1c }, gradient: None, preview: [0x3957b6; 2] },
    ThemePreset::dark("darkcyan",   0x407ab8, 0x20232c, 0x171a23, [0x2a364f, 0x203144], 0x3f6696),
    ThemePreset::dark("darkgreen",  0x42a197, 0x252a2c, 0x1c2123, [0x253941, 0x273534], 0x36786f),
    ThemePreset::dark("darkyellow", 0xe2a861, 0x2c2927, 0x23201e, [0x38280f, 0x4c3d29], 0x937041),
    ThemePreset::dark("darkpink",   0xc4678b, 0x262225, 0x1d191c, [0x4d2b3a, 0x523b49], 0x9c5b71),
    ThemePreset::dark("darkpurple", 0x9067c5, 0x252226, 0x1c191d, [0x332a5a, 0x433252], 0x6353a1),
];
