pub mod sticky_note;

pub use sticky_note::*;

pub(crate) use desktop_common::{actions, sticky_settings, sticky_windows, touch};
#[cfg(test)]
pub(crate) use desktop_common::keymap;
pub(crate) use fern_i18n as i18n;
pub(crate) use task_ui::task_detail;
