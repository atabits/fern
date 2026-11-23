//! Declarative [`UiNode`] GPUI renderer and [`PluginView`] entity.

mod nodes;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, ParentElement, Render, RenderImage, ScrollHandle, SharedString, Styled,
    Subscription, Window, div, px,
};
use icons::IconName;
use plugins::PluginStatus;
use plugins::ui_ast::{HandlerId, UiEvent, UiTreePayload, sanitize_svg};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{Button, ButtonVariant, Color, EmptyState, Label, v_scroll};

use self::nodes::SvgCache;
use crate::host::PluginHost;

const INPUT_DEBOUNCE: Duration = Duration::from_millis(150);

/// Callback invoked whenever an interactive [`UiNode`] widget emits a [`UiEvent`].
pub type UiEventSink = Rc<dyn Fn(UiEvent, &mut App)>;

/// Callback invoked when [`UiTreeState`] internal UI state (e.g. an open select popover) changes.
pub type UiNotifySink = Rc<dyn Fn(&mut App)>;

/// Events emitted by [`PluginView`] to parent shell containers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginViewEvent {
    OpenLogs { plugin_id: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayoutCtx {
    /// Inside a vertical container (`VStack`, `Card`, `Section`, `Scroll`, `Grid`, or root).
    Column,
    /// Inside a width-constrained `HStack` (`w_full()` or `flex_1()`).
    RowConstrained,
    /// Inside an auto-sized `HStack` (`flex_none()`): children must NOT use `flex_1()`,
    /// because `flex-basis: 0%` inside an auto-width row collapses them to `0px`.
    RowAuto,
}

struct InputCallbacks {
    view_id: String,
    on_change: Option<HandlerId>,
    on_submit: Option<HandlerId>,
    debounce_task: Option<gpui::Task<()>>,
    sink: UiEventSink,
}

struct InputSlot {
    entity: Entity<TextInput>,
    revision: u64,
    mode: Mode,
    placeholder: String,
    read_only: bool,
    shared: Rc<RefCell<InputCallbacks>>,
    used: bool,
    _subs: [Subscription; 2],
}

impl Drop for InputSlot {
    fn drop(&mut self) {
        self.shared.borrow_mut().debounce_task = None;
    }
}

/// Stateful GPUI widget state for a rendered declarative [`UiTreePayload`] tree.
///
/// Preserves [`TextInput`] focus and cursor position across re-renders via `revision` guards,
/// persists [`ScrollHandle`] instances by `Scroll.key`, and caches rasterized SVG [`RenderImage`]s.
pub struct UiTreeState {
    inputs: HashMap<String, InputSlot>,
    scrolls: HashMap<String, (ScrollHandle, bool)>,
    open_select: Rc<RefCell<Option<String>>>,
    svg_cache: SvgCache,
    node_counter: usize,
    #[cfg(test)]
    test_text_counter: usize,
    #[cfg(test)]
    test_card_counter: usize,
}

impl Default for UiTreeState {
    fn default() -> Self {
        Self::new()
    }
}

impl UiTreeState {
    pub fn new() -> Self {
        Self {
            inputs: HashMap::new(),
            scrolls: HashMap::new(),
            open_select: Rc::new(RefCell::new(None)),
            svg_cache: SvgCache::default(),
            node_counter: 0,
            #[cfg(test)]
            test_text_counter: 0,
            #[cfg(test)]
            test_card_counter: 0,
        }
    }

    pub fn clear(&mut self) {
        self.inputs.clear();
        self.scrolls.clear();
        *self.open_select.borrow_mut() = None;
        self.svg_cache.clear();
    }

    pub(crate) fn close_select(&mut self) -> bool {
        self.open_select.borrow_mut().take().is_some()
    }

    pub fn input_entity(&self, key: &str) -> Option<&Entity<TextInput>> {
        self.inputs.get(key).map(|slot| &slot.entity)
    }

    pub fn scroll_handle(&self, key: &str) -> Option<&ScrollHandle> {
        self.scrolls.get(key).map(|(handle, _)| handle)
    }

    /// Renders `payload` into GPUI elements while syncing stateful inputs, scroll handles, and SVG cache.
    pub fn render_tree(
        &mut self,
        payload: &UiTreePayload,
        event_sink: UiEventSink,
        notify_sink: UiNotifySink,
        cx: &mut App,
    ) -> AnyElement {
        for slot in self.inputs.values_mut() {
            slot.used = false;
        }
        for (_, used) in self.scrolls.values_mut() {
            *used = false;
        }
        self.node_counter = 0;
        #[cfg(test)]
        {
            self.test_text_counter = 0;
            self.test_card_counter = 0;
        }

        let element = self.render_node(
            &payload.root,
            &payload.view_id,
            LayoutCtx::Column,
            &event_sink,
            &notify_sink,
            cx,
        );

        self.inputs.retain(|_, slot| slot.used);
        self.scrolls
            .retain(|key, (_, used)| *used || key == "__plugin_root__");
        element
    }

    fn next_id(&mut self, prefix: &str) -> ElementId {
        self.node_counter += 1;
        ElementId::Name(SharedString::from(format!("{prefix}-{}", self.node_counter)))
    }

    fn ensure_scroll(&mut self, key: &str) -> ScrollHandle {
        let entry = self
            .scrolls
            .entry(key.to_owned())
            .or_insert_with(|| (ScrollHandle::new(), true));
        entry.1 = true;
        entry.0.clone()
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_input(
        &mut self,
        key: &str,
        view_id: &str,
        mode: Mode,
        value: &str,
        revision: u64,
        placeholder: &str,
        disabled: bool,
        on_change: Option<HandlerId>,
        on_submit: Option<HandlerId>,
        sink: &UiEventSink,
        cx: &mut App,
    ) -> Entity<TextInput> {
        if let Some(slot) = self.inputs.get_mut(key)
            && slot.mode == mode
        {
            slot.used = true;
            {
                let mut cb = slot.shared.borrow_mut();
                cb.view_id = view_id.to_owned();
                cb.on_change = on_change;
                cb.on_submit = on_submit;
                cb.sink = Rc::clone(sink);
            }
            if slot.revision != revision {
                slot.revision = revision;
                slot.shared.borrow_mut().debounce_task = None;
                let text = value.to_owned();
                slot.entity.update(cx, |input, cx| {
                    input.set_text(text, cx);
                });
            }
            if slot.placeholder != placeholder {
                slot.placeholder = placeholder.to_owned();
                let next_ph = slot.placeholder.clone();
                slot.entity.update(cx, |input, cx| {
                    input.set_placeholder(next_ph, cx);
                });
            }
            if slot.read_only != disabled {
                slot.read_only = disabled;
                slot.entity.update(cx, |input, cx| {
                    input.set_read_only(disabled, cx);
                });
            }
            return slot.entity.clone();
        }

        let initial_text = value.to_owned();
        let placeholder_owned = placeholder.to_owned();
        let entity = cx.new(|cx| {
            let mut input = TextInput::new(mode, placeholder_owned, cx).without_cancel();
            input.set_text(initial_text, cx);
            if disabled {
                input.set_read_only(true, cx);
            }
            input
        });

        let shared = Rc::new(RefCell::new(InputCallbacks {
            view_id: view_id.to_owned(),
            on_change,
            on_submit,
            debounce_task: None,
            sink: Rc::clone(sink),
        }));

        let shared_changed = Rc::clone(&shared);
        let sub_changed = cx.subscribe(&entity, move |emitter, _: &Changed, cx| {
            // Use Weak so the pending debounce task cannot form an Rc cycle with InputCallbacks.
            let weak_shared = Rc::downgrade(&shared_changed);
            let task = cx.spawn(async move |cx| {
                cx.background_executor().timer(INPUT_DEBOUNCE).await;
                let Some(shared_inner) = weak_shared.upgrade() else {
                    return;
                };
                cx.update(|cx| {
                    let (vid, hid, sink_fn) = {
                        let cb = shared_inner.borrow();
                        (cb.view_id.clone(), cb.on_change, Rc::clone(&cb.sink))
                    };
                    if let Some(handler_id) = hid {
                        let current = emitter.read(cx).text().to_owned();
                        sink_fn(UiEvent::with_value(vid, handler_id, current), cx);
                    }
                });
            });
            shared_changed.borrow_mut().debounce_task = Some(task);
        });

        let shared_submit = Rc::clone(&shared);
        let sub_submit = cx.subscribe(&entity, move |_, ev: &Submitted, cx| {
            let (vid, on_submit_hid, on_change_hid, had_pending, sink_fn) = {
                let mut cb = shared_submit.borrow_mut();
                let had_pending = cb.debounce_task.take().is_some();
                (
                    cb.view_id.clone(),
                    cb.on_submit,
                    cb.on_change,
                    had_pending,
                    Rc::clone(&cb.sink),
                )
            };
            if let Some(handler_id) = on_submit_hid {
                sink_fn(UiEvent::with_value(vid, handler_id, ev.0.clone()), cx);
            } else if had_pending
                && let Some(handler_id) = on_change_hid
            {
                sink_fn(UiEvent::with_value(vid, handler_id, ev.0.clone()), cx);
            }
        });

        self.inputs.insert(
            key.to_owned(),
            InputSlot {
                entity: entity.clone(),
                revision,
                mode,
                placeholder: placeholder.to_owned(),
                read_only: disabled,
                shared,
                used: true,
                _subs: [sub_changed, sub_submit],
            },
        );

        entity
    }
}

/// Native GPUI entity hosting a sidebar plugin view (`fern.views.register`).
pub struct PluginView {
    plugin_id: String,
    view_id: String,
    focus_handle: FocusHandle,
    tree_state: UiTreeState,
    seen_plugin_revision: u64,
    mounted: Cell<bool>,
    embedded: bool,
    _host_sub: Subscription,
}

impl EventEmitter<PluginViewEvent> for PluginView {}

impl Focusable for PluginView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PluginView {
    pub fn new(
        plugin_id: impl Into<String>,
        view_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(plugin_id, view_id, false, cx)
    }

    /// Creates a [`PluginView`] intended for embedding inside another container (e.g. Settings → Plugins)
    /// without main-window titlebar top inset.
    pub fn new_embedded(
        plugin_id: impl Into<String>,
        view_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(plugin_id, view_id, true, cx)
    }

    fn should_mount_target(plugin_id: &str, view_id: &str, cx: &App) -> bool {
        PluginHost::global(cx).is_some_and(|h| {
            if h.is_safe_mode() {
                return false;
            }
            match h.plugin(plugin_id) {
                Some(info) => {
                    info.enabled
                        && !matches!(
                            info.status,
                            PluginStatus::Disabled
                                | PluginStatus::NeedsApproval { .. }
                                | PluginStatus::CircuitBroken { .. }
                        )
                }
                // Preserve synthetic test trees where the plugin is not registered on the host.
                None => {
                    h.view_tree(plugin_id, view_id).is_none()
                        && h.view_error(plugin_id, view_id).is_none()
                }
            }
        })
    }

    fn create(
        plugin_id: impl Into<String>,
        view_id: impl Into<String>,
        embedded: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let plugin_id = plugin_id.into();
        let view_id = view_id.into();
        let seen_plugin_revision = PluginHost::global(cx)
            .map_or(0, |h| h.plugin_revision(&plugin_id));

        let host_sub = cx.observe_global::<PluginHost>(|this, cx| {
            this.on_host_updated(cx);
            cx.notify();
        });

        cx.on_release(|this: &mut PluginView, cx| {
            this.unmount(cx);
        })
        .detach();

        let should_mount = Self::should_mount_target(&plugin_id, &view_id, cx);
        if should_mount {
            PluginHost::mount_view_async(plugin_id.clone(), view_id.clone(), cx);
        }

        Self {
            plugin_id,
            view_id,
            focus_handle: cx.focus_handle(),
            tree_state: UiTreeState::new(),
            seen_plugin_revision,
            mounted: Cell::new(should_mount),
            embedded,
            _host_sub: host_sub,
        }
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn view_id(&self) -> &str {
        &self.view_id
    }

    pub fn input_entity(&self, key: &str) -> Option<&Entity<TextInput>> {
        self.tree_state.input_entity(key)
    }

    /// Switches this view to render another `(plugin_id, view_id)` cleanly.
    pub fn set_target(
        &mut self,
        plugin_id: impl Into<String>,
        view_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let next_plugin = plugin_id.into();
        let next_view = view_id.into();
        if self.plugin_id == next_plugin && self.view_id == next_view {
            return;
        }
        self.unmount(cx);
        self.plugin_id = next_plugin;
        self.view_id = next_view;
        self.tree_state.clear();
        self.seen_plugin_revision = PluginHost::global(cx)
            .map_or(0, |h| h.plugin_revision(&self.plugin_id));
        let should_mount = Self::should_mount_target(&self.plugin_id, &self.view_id, cx);
        if should_mount {
            self.mounted.set(true);
            PluginHost::mount_view_async(self.plugin_id.clone(), self.view_id.clone(), cx);
        }
        cx.notify();
    }

    pub fn unmount(&self, cx: &mut App) {
        if self.mounted.replace(false) {
            PluginHost::unmount_view_async(self.plugin_id.clone(), self.view_id.clone(), cx);
        }
    }

    fn on_host_updated(&mut self, cx: &mut Context<Self>) {
        let Some(host) = PluginHost::global(cx) else {
            return;
        };
        let rev = host.plugin_revision(&self.plugin_id);
        if rev != self.seen_plugin_revision {
            self.seen_plugin_revision = rev;
            let can_mount = !host.is_safe_mode()
                && host.plugin(&self.plugin_id).is_some_and(|p| {
                    p.enabled
                        && !matches!(
                            p.status,
                            PluginStatus::Disabled
                                | PluginStatus::NeedsApproval { .. }
                                | PluginStatus::CircuitBroken { .. }
                        )
                });
            let already_current =
                host.view_revision(&self.plugin_id, &self.view_id) == Some(rev)
                    || host.is_view_mounting(&self.plugin_id, &self.view_id);
            if can_mount && !already_current {
                self.mounted.set(true);
                PluginHost::mount_view_async(self.plugin_id.clone(), self.view_id.clone(), cx);
            }
        }
    }

    fn render_error_fallback(&self, message: String, cx: &mut Context<Self>) -> AnyElement {
        let strings = i18n::t(cx);
        let plugin_id = self.plugin_id.clone();
        let view_id = self.view_id.clone();
        let is_circuit_broken = PluginHost::global(cx)
            .and_then(|h| h.plugin(&plugin_id))
            .is_some_and(|p| matches!(p.status, PluginStatus::CircuitBroken { .. }));

        let reload_pid = plugin_id.clone();
        let reload_vid = view_id.clone();
        let logs_pid = plugin_id.clone();

        let secondary_actions = h_flex()
            .gap_2()
            .justify_center()
            .when(is_circuit_broken, |row| {
                let reset_pid = plugin_id.clone();
                let reset_vid = view_id.clone();
                row.child(
                    Button::new("plugin-fallback-reset", strings.plugins_reset_enable)
                        .variant(ButtonVariant::Primary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mounted.set(true);
                            PluginHost::reset_and_mount_view_async(
                                reset_pid.clone(),
                                reset_vid.clone(),
                                cx,
                            );
                        })),
                )
            })
            .child(
                Button::new("plugin-fallback-reload", strings.plugins_reload)
                    .variant(if is_circuit_broken {
                        ButtonVariant::Secondary
                    } else {
                        ButtonVariant::Primary
                    })
                    .icon(IconName::RefreshCw)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.mounted.set(true);
                        PluginHost::reload_and_mount_view_async(
                            reload_pid.clone(),
                            reload_vid.clone(),
                            cx,
                        );
                    })),
            )
            .child(
                Button::new("plugin-fallback-logs", strings.plugins_open_logs)
                    .variant(ButtonVariant::Secondary)
                    .icon(IconName::FileText)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        PluginHost::request_open_logs_for(logs_pid.clone(), cx);
                        cx.emit(PluginViewEvent::OpenLogs {
                            plugin_id: logs_pid.clone(),
                        });
                    })),
            );

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_6()
            .gap_3()
            .when(cfg!(test), |el| {
                el.debug_selector(|| "plugin-view-error-fallback".to_owned())
            })
            .child(
                div()
                    .flex_none()
                    .child(
                        EmptyState::new(IconName::CircleAlert, message)
                            .title(strings.plugins_view_error_title)
                            .max_width(px(420.)),
                    ),
            )
            .child(
                div()
                    .when(cfg!(test), |el| {
                        el.debug_selector(|| "plugin-view-error-actions".to_owned())
                    })
                    .child(secondary_actions),
            )
            .into_any_element()
    }
}

impl Render for PluginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let safe_mode = PluginHost::global(cx).is_some_and(|h| h.is_safe_mode());
        if safe_mode {
            return self.render_error_fallback(strings.plugins_safe_mode_active.to_owned(), cx);
        }

        let payload = PluginHost::global(cx)
            .and_then(|h| h.view_tree_arc(&self.plugin_id, &self.view_id));
        let view_error = PluginHost::global(cx).and_then(|h| {
            h.view_error(&self.plugin_id, &self.view_id)
                .or_else(|| h.view_error(&self.plugin_id, ""))
                .map(str::to_owned)
        });
        let plugin_status = PluginHost::global(cx)
            .and_then(|h| h.plugin(&self.plugin_id))
            .map(|p| p.status.clone());

        if let Some(err) = view_error {
            return self.render_error_fallback(err, cx);
        }

        match plugin_status {
            Some(PluginStatus::Disabled) => {
                return self.render_error_fallback(strings.plugins_disabled.to_owned(), cx);
            }
            Some(PluginStatus::NeedsApproval { .. }) => {
                return self
                    .render_error_fallback(strings.plugins_permissions_changed.to_owned(), cx);
            }
            Some(PluginStatus::CircuitBroken { last_error }) => {
                return self.render_error_fallback(
                    format!("{}\n{}", strings.plugins_circuit_broken, last_error),
                    cx,
                );
            }
            Some(PluginStatus::Error(err)) if payload.is_none() => {
                return self.render_error_fallback(err, cx);
            }
            _ => {}
        }

        let Some(payload) = payload else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .p_6()
                .child(Label::new("…").color(Color::Muted))
                .into_any_element();
        };

        let plugin_id = self.plugin_id.clone();
        let event_sink: UiEventSink = Rc::new(move |ev, cx| {
            PluginHost::dispatch_ui_event_async(plugin_id.clone(), ev, cx);
        });
        let weak = cx.weak_entity();
        let notify_sink: UiNotifySink = Rc::new(move |cx| {
            let _ = weak.update(cx, |_, cx| cx.notify());
        });

        let root_el = self
            .tree_state
            .render_tree(&payload, event_sink, notify_sink, cx);
        let root_scroll = self.tree_state.ensure_scroll("__plugin_root__");

        v_flex()
            .size_full()
            .min_h_0()
            .when(!self.embedded, |el| {
                el.pt(desktop_common::layout::header_inset())
                    .bg(cx.theme().colors().background)
            })
            .track_focus(&self.focus_handle)
            .child(
                v_scroll(
                    ElementId::Name(SharedString::from(format!(
                        "plugin-view-{}-{}",
                        self.plugin_id, self.view_id
                    ))),
                    v_flex().id("plugin-view-scroll-body").w_full().child(root_el),
                )
                .track_scroll(&root_scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}

/// `usvg` options with external/data `<image>` resolution disabled so untrusted
/// plugin SVGs cannot read local files or recursively load embedded images.
pub fn safe_usvg_options() -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        image_href_resolver: resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..resvg::usvg::Options::default()
    }
}

/// Sanitizes an untrusted SVG, substitutes `currentColor` with `ink_rgb`,
/// safely rasterizes it centered inside `width_px × height_px`, and converts
/// it to a GPUI `RenderImage`.
pub fn rasterize_svg(
    raw_svg: &str,
    width_px: u32,
    height_px: u32,
    ink_rgb: (u8, u8, u8),
    cx: &App,
) -> Option<Arc<RenderImage>> {
    let sanitized = sanitize_svg(raw_svg).ok()?;
    let (r, g, b) = ink_rgb;
    let resolved_svg = if sanitized.to_ascii_lowercase().contains("currentcolor") {
        let hex = format!("#{r:02x}{g:02x}{b:02x}");
        replace_ascii_case_insensitive(&sanitized, "currentcolor", &hex)
    } else {
        sanitized
    };

    let opts = safe_usvg_options();
    let tree = resvg::usvg::Tree::from_str(&resolved_svg, &opts).ok()?;
    let svg_size = tree.size();
    let sx = width_px as f32 / svg_size.width().max(1.0);
    let sy = height_px as f32 / svg_size.height().max(1.0);
    let scale = sx.min(sy);
    let dx = (width_px as f32 - svg_size.width() * scale) * 0.5;
    let dy = (height_px as f32 - svg_size.height() * scale) * 0.5;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width_px, height_px)?;
    let transform =
        resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let png = pixmap.encode_png().ok()?;
    let gpui_img = gpui::Image::from_bytes(gpui::ImageFormat::Png, png);
    gpui_img.to_image_data(cx.svg_renderer()).ok()
}

fn replace_ascii_case_insensitive(haystack: &str, needle_lower: &str, replacement: &str) -> String {
    let mut out = String::with_capacity(haystack.len());
    let lower = haystack.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(pos) = lower[cursor..].find(needle_lower) {
        let abs = cursor + pos;
        out.push_str(&haystack[cursor..abs]);
        out.push_str(replacement);
        cursor = abs + needle_lower.len();
    }
    out.push_str(&haystack[cursor..]);
    out
}
