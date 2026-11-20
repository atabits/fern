use gpui::{Hsla, rgb};

/// Colors offered for lists, as stored in the database.
pub const LIST_COLORS: [&str; 8] =
    ["#EF4444", "#F97316", "#EAB308", "#22C55E", "#06B6D4", "#3B82F6", "#A855F7", "#EC4899"];

/// A tag or list color parsed from `#RRGGBB`.
pub fn parse_color(value: Option<&str>) -> Option<Hsla> {
    value
        .and_then(|c| u32::from_str_radix(c.trim_start_matches('#'), 16).ok())
        .map(|c| rgb(c).into())
}
