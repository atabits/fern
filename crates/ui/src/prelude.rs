//! What a view usually needs: GPUI's prelude, the components and the theme accessor.

pub use gpui::prelude::*;
pub use gpui::{
    AnyElement, App, Context, Div, ElementId, Hsla, ParentElement, Pixels, RenderOnce,
    SharedString, Stateful, Styled, Window, div, px,
};
pub use theme::ActiveTheme;

pub use crate::{
    Checkbox, Chip, Clickable, Color, Divider, Icon, IconButton, IconName, IconSize, Label,
    LabelSize, ListItem, Toggleable, Tooltip, h_flex, v_flex,
};
