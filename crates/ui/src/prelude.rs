//! What a view usually needs: GPUI's prelude, the components and the theme accessor.

pub use gpui::prelude::*;
pub use gpui::{
    AnyElement, App, Context, Div, ElementId, Hsla, ParentElement, Pixels, RenderOnce,
    SharedString, Stateful, Styled, Window, div, px,
};
pub use theme::ActiveTheme;

pub use crate::{
    Button, ButtonSize, ButtonVariant, Checkbox, Chip, Clickable, Color, ConfirmDialog,
    Disableable, Divider, EmptyState, Icon, IconButton, IconName, IconSize, Label, LabelSize,
    ListItem, Modal, SegmentedTab, SegmentedTabs, Switch, ToggleChip, Toggleable, Tooltip, h_flex,
    v_flex, v_scroll,
};
