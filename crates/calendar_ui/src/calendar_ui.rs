pub mod calendar_options_view;
pub mod calendar_view;

pub use calendar_view::*;

pub(crate) use desktop_common::countdown_helpers as countdown_view;
pub(crate) use desktop_common::data_version;
#[cfg(test)]
pub(crate) use desktop_common::keymap;
pub(crate) use desktop_common::{
    actions, calendar, sticky_opener, text_input, touch, undo,
};
pub(crate) use desktop_sync::subscriptions;
pub(crate) use fern_i18n as i18n;
pub(crate) use task_ui::{drag, lists_panel, print, task_detail, task_menu, task_rows};

pub(crate) mod title_bar {
    pub(crate) use desktop_common::layout::{
        frame_vertical_inset, header_inset, title_bar_height as height,
    };
}
