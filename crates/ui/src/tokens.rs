//! Shared spacing, corner radius, and row-height design tokens.

use gpui::{Pixels, px};

pub mod spacing {
    use super::{Pixels, px};

    pub const XXS: Pixels = px(2.);
    pub const XS: Pixels = px(4.);
    pub const SM: Pixels = px(8.);
    pub const MD: Pixels = px(12.);
    pub const LG: Pixels = px(16.);
    pub const XL: Pixels = px(20.);
    pub const XXL: Pixels = px(24.);
}

pub mod radius {
    use super::{Pixels, px};

    pub const SM: Pixels = px(4.);
    pub const MD: Pixels = px(6.);
    pub const LG: Pixels = px(10.);
    pub const XL: Pixels = px(12.);
    pub const PILL: Pixels = px(999.);
}

pub mod row_height {
    use super::{Pixels, px};

    pub const COMPACT: Pixels = px(28.);
    pub const SM: Pixels = px(32.);
    pub const DEFAULT: Pixels = px(36.);
    pub const COMFORTABLE: Pixels = px(38.);
    pub const TASK: Pixels = px(40.);
}
