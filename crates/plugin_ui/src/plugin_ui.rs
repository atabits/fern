//! GPUI host, declarative view renderer, modal/toast overlays, and plugin settings UI for Fern.

pub mod host;
pub mod modal;
pub mod settings_view;
pub mod view;

pub use host::{PluginHost, PluginHostConfig, collect_plugin_themes};
pub use modal::{
    PluginOverlayView, dismiss_active_plugin_modal, handle_plugin_modal_escape,
    render_plugin_overlays,
};
pub use plugins::{HookDecision, Permission, PluginInfo, PluginManager, PluginStatus};
pub mod api {
    pub use plugins::api::{parse_task_draft, task_to_json};
}
pub use settings_view::{PluginSettingsTab, PluginSettingsView};
pub use view::{PluginView, PluginViewEvent, UiTreeState, rasterize_svg, safe_usvg_options};

#[cfg(test)]
mod tests;
