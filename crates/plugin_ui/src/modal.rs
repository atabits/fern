//! Plugin modal overlays (`View`, `Confirm`, `Prompt`) and auto-expiring toast stack.

use std::rc::Rc;
use std::sync::Arc;

use desktop_common::actions::{DismissSettings, DismissTransient};
use desktop_common::plugin_bus::{
    PluginBus, PluginModalKind, PluginModalSpec, PluginToast, ToastLevel,
};
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyWindowHandle, App, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, Global, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render,
    SharedString, Styled, Subscription, WeakEntity, Window, div, px,
};
use icons::IconName;
use plugins::PromptReplyValue;
use plugins::ui_ast::{UiNode, UiTreePayload};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonVariant, ConfirmDialog, Icon, IconButton, IconSize, Label, LabelSize, Modal,
    v_scroll,
};

use crate::host::{PluginHost, resolve_displaced_modal, schedule_toast_expiry};
use crate::view::{UiNotifySink, UiTreeState};

#[derive(Default)]
struct OverlayWindows(Vec<(AnyWindowHandle, Option<WeakEntity<PluginOverlayView>>)>);

impl Global for OverlayWindows {}

fn register_overlay_window(
    handle: AnyWindowHandle,
    view: Option<WeakEntity<PluginOverlayView>>,
    cx: &mut App,
) {
    let open = cx.windows();
    let mounted = cx.default_global::<OverlayWindows>();
    mounted.0.retain(|(h, _)| open.contains(h));
    if let Some((_, existing)) = mounted.0.iter_mut().find(|(h, _)| *h == handle) {
        if view.is_some() {
            *existing = view;
        }
    } else {
        mounted.0.push((handle, view));
    }
}

fn unregister_overlay_window(handle: AnyWindowHandle, cx: &mut App) {
    if cx.has_global::<OverlayWindows>() {
        cx.update_global::<OverlayWindows, _>(|mounted, _| {
            mounted.0.retain(|(h, _)| *h != handle);
        });
    }
}

fn should_render_overlays_in_window(window: &Window, cx: &App) -> bool {
    let current = window.window_handle();
    let open = cx.windows();
    let mounted: Vec<AnyWindowHandle> = cx
        .try_global::<OverlayWindows>()
        .map(|m| {
            m.0.iter()
                .map(|(h, _)| *h)
                .filter(|h| open.contains(h))
                .collect()
        })
        .unwrap_or_default();

    if let Some(active) = cx.active_window()
        && (mounted.is_empty() || mounted.contains(&active))
    {
        return active == current;
    }

    if let Some(&first_mounted) = mounted.first() {
        first_mounted == current
    } else {
        open.first().is_none_or(|&first| first == current)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ModalFocusSig {
    generation: u64,
    plugin_id: String,
    view_id: String,
    kind_tag: u8,
    request_id: u64,
}

fn modal_focus_sig(spec: &PluginModalSpec, generation: u64) -> ModalFocusSig {
    let (kind_tag, request_id) = match &spec.kind {
        PluginModalKind::Confirm { request_id, .. } => (1, *request_id),
        PluginModalKind::Prompt { request_id, .. } => (2, *request_id),
        PluginModalKind::View { .. } => (3, 0),
    };
    ModalFocusSig {
        generation,
        plugin_id: spec.plugin_id.clone(),
        view_id: spec.view_id.clone(),
        kind_tag,
        request_id,
    }
}

struct ActivePromptState {
    request_id: u64,
    input: Entity<TextInput>,
    _subs: [Subscription; 2],
}

/// Stateful GPUI overlay view rendering active plugin modals and the floating toast stack.
pub struct PluginOverlayView {
    focus_handle: FocusHandle,
    previous_focus: Option<FocusHandle>,
    focused_modal_sig: Option<ModalFocusSig>,
    prompt: Option<ActivePromptState>,
    modal_tree_key: Option<(String, String)>,
    modal_title: String,
    seen_modal_gen: u64,
    modal_tree_payload: Option<Result<Arc<UiTreePayload>, String>>,
    modal_tree_state: UiTreeState,
    modal_scroll: gpui::ScrollHandle,
    _bus_sub: Subscription,
    _host_sub: Subscription,
    _activation_sub: Option<Subscription>,
}

impl Focusable for PluginOverlayView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PluginOverlayView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::create(None, cx)
    }

    fn new_in_window(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::create(Some(window), cx)
    }

    fn create(window: Option<&mut Window>, cx: &mut Context<Self>) -> Self {
        let bus_sub = cx.observe_global::<PluginBus>(|_, cx| {
            cx.notify();
        });
        let host_sub = cx.observe_global::<PluginHost>(|_, cx| {
            let viewing_modal = PluginBus::global(cx).is_some_and(|b| {
                matches!(
                    b.active_modal.as_ref().map(|m| &m.kind),
                    Some(PluginModalKind::View { .. })
                )
            });
            if viewing_modal {
                cx.notify();
            }
        });
        let activation_sub = window.map(|win| {
            let handle = win.window_handle();
            register_overlay_window(handle, Some(cx.weak_entity()), cx);
            cx.on_release(move |_, cx| {
                unregister_overlay_window(handle, cx);
            })
            .detach();
            cx.observe_window_activation(win, |_, _, cx| {
                cx.notify();
            })
        });
        Self {
            focus_handle: cx.focus_handle(),
            previous_focus: None,
            focused_modal_sig: None,
            prompt: None,
            modal_tree_key: None,
            modal_title: String::new(),
            seen_modal_gen: 0,
            modal_tree_payload: None,
            modal_tree_state: UiTreeState::new(),
            modal_scroll: gpui::ScrollHandle::new(),
            _bus_sub: bus_sub,
            _host_sub: host_sub,
            _activation_sub: activation_sub,
        }
    }

    fn dismiss_active_modal_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        let is_view_modal = PluginBus::global(cx).is_some_and(|bus| {
            matches!(
                bus.active_modal.as_ref().map(|m| &m.kind),
                Some(PluginModalKind::View { .. })
            )
        });
        if is_view_modal && self.modal_tree_state.close_select() {
            cx.notify();
            return true;
        }
        dismiss_active_plugin_modal(cx)
    }

    fn sync_prompt(
        &mut self,
        request_id: u64,
        placeholder: &str,
        default_value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(active) = &self.prompt
            && active.request_id == request_id
        {
            return active.input.clone();
        }

        let ph = placeholder.to_owned();
        let dv = default_value.to_owned();
        let input = cx.new(|cx| {
            let mut inp = TextInput::new(Mode::SingleLine, ph, cx);
            inp.set_text(dv, cx);
            inp
        });

        let sub_submit = cx.subscribe(&input, move |_, _, ev: &Submitted, cx| {
            let text = ev.0.clone();
            resolve_and_close_prompt(request_id, PromptReplyValue::Prompt(Some(text)), cx);
        });
        let sub_cancel = cx.subscribe(&input, move |_, _, _: &Cancelled, cx| {
            resolve_and_close_prompt(request_id, PromptReplyValue::Prompt(None), cx);
        });

        window.focus(&input.focus_handle(cx), cx);
        self.prompt = Some(ActivePromptState {
            request_id,
            input: input.clone(),
            _subs: [sub_submit, sub_cancel],
        });
        input
    }

    fn render_modal(
        &mut self,
        spec: &PluginModalSpec,
        modal_gen: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let colors = theme.colors();
        let status_colors = theme.status();
        let title = spec.title.clone();

        match &spec.kind {
            PluginModalKind::Confirm { body, request_id } => {
                let req_id = *request_id;
                ConfirmDialog::new("plugin-confirm-modal", title, strings.cancel, strings.confirm)
                    .description(body.clone())
                    .on_cancel(move |_, _, cx| {
                        resolve_and_close_prompt(req_id, PromptReplyValue::Confirm(false), cx);
                    })
                    .on_confirm(move |_, _, cx| {
                        resolve_and_close_prompt(req_id, PromptReplyValue::Confirm(true), cx);
                    })
                    .into_any_element()
            }
            PluginModalKind::Prompt {
                placeholder,
                default_value,
                request_id,
            } => {
                let req_id = *request_id;
                let input =
                    self.sync_prompt(req_id, placeholder, default_value, window, cx);
                let input_confirm = input.clone();

                Modal::new("plugin-prompt-modal")
                    .width(px(420.))
                    .on_dismiss(move |_, _, cx| {
                        resolve_and_close_prompt(req_id, PromptReplyValue::Prompt(None), cx);
                    })
                    .child(
                        div()
                            .w_full()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(title),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(32.))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .rounded(px(6.))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .child(input),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .pt_1()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("plugin-prompt-cancel", strings.cancel)
                                    .variant(ButtonVariant::Secondary)
                                    .on_click(move |_, _, cx| {
                                        resolve_and_close_prompt(
                                            req_id,
                                            PromptReplyValue::Prompt(None),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                Button::new("plugin-prompt-ok", strings.ok)
                                    .variant(ButtonVariant::Primary)
                                    .on_click(move |_, _, cx| {
                                        let val = input_confirm.read(cx).text().to_owned();
                                        resolve_and_close_prompt(
                                            req_id,
                                            PromptReplyValue::Prompt(Some(val)),
                                            cx,
                                        );
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            PluginModalKind::View { tree } => {
                let next_key = (spec.plugin_id.clone(), spec.view_id.clone());
                let key_changed = self.modal_tree_key.as_ref() != Some(&next_key);
                let gen_changed = self.seen_modal_gen != modal_gen;
                if key_changed || gen_changed || self.modal_tree_payload.is_none() {
                    if key_changed || gen_changed {
                        self.modal_scroll = gpui::ScrollHandle::new();
                        self.modal_tree_state.close_select();
                        if key_changed || self.modal_title != spec.title {
                            self.modal_tree_state.clear();
                        }
                        self.modal_tree_key = Some(next_key);
                        self.modal_title = spec.title.clone();
                        self.seen_modal_gen = modal_gen;
                    }
                    self.modal_tree_payload = Some(
                        serde_json::from_value::<UiNode>(tree.clone())
                            .map(|root| {
                                Arc::new(UiTreePayload {
                                    plugin_id: spec.plugin_id.clone(),
                                    view_id: spec.view_id.clone(),
                                    root,
                                })
                            })
                            .map_err(|err| format!("{err}")),
                    );
                }

                let modal_error = PluginHost::global(cx)
                    .and_then(|h| h.view_error(&spec.plugin_id, &spec.view_id).map(str::to_owned));

                let body = match self.modal_tree_payload.as_ref() {
                    Some(Ok(payload)) => {
                        let pid = spec.plugin_id.clone();
                        let sink = Rc::new(move |ev, cx: &mut App| {
                            PluginHost::dispatch_ui_event_async(pid.clone(), ev, cx);
                        });
                        let weak = cx.weak_entity();
                        let notify_sink: UiNotifySink = Rc::new(move |cx| {
                            let _ = weak.update(cx, |_, cx| cx.notify());
                        });
                        self.modal_tree_state
                            .render_tree(payload, sink, notify_sink, cx)
                    }
                    Some(Err(err)) => Label::new(err.clone())
                        .size(LabelSize::Small)
                        .color(Color::Error)
                        .into_any_element(),
                    None => div().into_any_element(),
                };

                Modal::new("plugin-view-modal")
                    .max_width(px(560.))
                    .max_height(px(620.))
                    .on_dismiss(|_, _, cx| {
                        dismiss_active_plugin_modal(cx);
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .gap_2()
                            .child(
                                Label::new(title)
                                    .size(LabelSize::Default)
                                    .weight(FontWeight::SEMIBOLD)
                                    .truncate(),
                            )
                            .child(
                                IconButton::new("plugin-view-modal-close", IconName::X)
                                    .icon_color(Color::Muted)
                                    .on_click(|_, _, cx| {
                                        dismiss_active_plugin_modal(cx);
                                    }),
                            ),
                    )
                    .when_some(modal_error, |modal, err| {
                        modal.child(
                            h_flex()
                                .w_full()
                                .p_2p5()
                                .gap_2()
                                .items_start()
                                .rounded(px(6.))
                                .bg(status_colors.error.opacity(0.12))
                                .when(cfg!(test), |el| {
                                    el.debug_selector(|| "plugin-view-modal-error".to_owned())
                                })
                                .child(
                                    Icon::new(IconName::CircleAlert)
                                        .size(IconSize::Small)
                                        .color(Color::Error),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(LabelSize::Small.pixels())
                                        .text_color(status_colors.error)
                                        .child(err),
                                ),
                        )
                    })
                    .child(
                        v_scroll(
                            "plugin-view-modal-scroll",
                            v_flex().id("plugin-view-modal-vp").w_full().child(body),
                        )
                        .track_scroll(&self.modal_scroll)
                        .max_h(px(520.))
                        .w_full(),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for PluginOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !should_render_overlays_in_window(window, cx) {
            self.focused_modal_sig = None;
            self.prompt = None;
            if let Some(prev) = self.previous_focus.take() {
                window.focus(&prev, cx);
            }
            return div().into_any_element();
        }

        let (active_modal, toasts) = PluginBus::global(cx)
            .map(|bus| (bus.active_modal.clone(), bus.toasts.clone()))
            .unwrap_or((None, Vec::new()));

        for toast in &toasts {
            schedule_toast_expiry(toast.id, cx);
        }

        if !matches!(
            active_modal.as_ref().map(|m| &m.kind),
            Some(PluginModalKind::Prompt { .. })
        ) {
            self.prompt = None;
        }
        if !matches!(
            active_modal.as_ref().map(|m| &m.kind),
            Some(PluginModalKind::View { .. })
        ) && (self.modal_tree_key.is_some() || self.modal_tree_payload.is_some())
        {
            self.modal_tree_key = None;
            self.modal_tree_payload = None;
            self.modal_title.clear();
            self.modal_tree_state.clear();
            self.modal_scroll = gpui::ScrollHandle::new();
        }

        let modal_gen = PluginHost::global(cx).map_or(0, PluginHost::modal_generation);
        if let Some(spec) = &active_modal {
            let sig = modal_focus_sig(spec, modal_gen);
            if self.focused_modal_sig.as_ref() != Some(&sig) {
                if self.previous_focus.is_none() {
                    self.previous_focus = window
                        .focused(cx)
                        .filter(|handle| *handle != self.focus_handle);
                }
                self.focused_modal_sig = Some(sig);
                if !matches!(spec.kind, PluginModalKind::Prompt { .. }) {
                    window.focus(&self.focus_handle, cx);
                }
            }
        } else {
            self.focused_modal_sig = None;
            if let Some(prev) = self.previous_focus.take() {
                window.focus(&prev, cx);
            }
        }

        if active_modal.is_none() && toasts.is_empty() {
            return div().into_any_element();
        }

        let has_modal = active_modal.is_some();
        let modal_el = active_modal.map(|spec| self.render_modal(&spec, modal_gen, window, cx));
        let toast_stack = (!toasts.is_empty()).then(|| render_toast_stack(&toasts, cx));

        div()
            .absolute()
            .inset_0()
            .when(has_modal, |el| {
                el.when(cfg!(test), |el| {
                    el.debug_selector(|| "plugin-overlay-modal".to_owned())
                })
                .track_focus(&self.focus_handle)
                    .key_context("PluginModal")
                    .on_action(cx.listener(|this, _: &DismissTransient, _, cx| {
                        this.dismiss_active_modal_on_escape(cx);
                    }))
                    .on_action(cx.listener(|this, _: &DismissSettings, _, cx| {
                        this.dismiss_active_modal_on_escape(cx);
                    }))
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape"
                            && !event.keystroke.modifiers.modified()
                            && this.dismiss_active_modal_on_escape(cx)
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            if this.dismiss_active_modal_on_escape(cx) {
                                cx.stop_propagation();
                            }
                        } else if PluginBus::global(cx).is_some_and(|b| b.active_modal.is_some()) {
                            cx.stop_propagation();
                        }
                    }))
            })
            .children(modal_el)
            .children(toast_stack)
            .into_any_element()
    }
}

/// Handles Escape for an active plugin modal: closes any open `Select` popover inside a
/// `View` modal first, otherwise dismisses the modal itself via [`dismiss_active_plugin_modal`].
pub fn handle_plugin_modal_escape(cx: &mut App) -> bool {
    let is_view_modal = match PluginBus::global(cx).and_then(|bus| bus.active_modal.as_ref()) {
        Some(modal) => matches!(modal.kind, PluginModalKind::View { .. }),
        None => return false,
    };
    if is_view_modal {
        let views: Vec<WeakEntity<PluginOverlayView>> = cx
            .try_global::<OverlayWindows>()
            .map(|mounted| {
                mounted
                    .0
                    .iter()
                    .filter_map(|(_, v)| v.clone())
                    .collect()
            })
            .unwrap_or_default();
        for weak in views {
            if let Ok(true) = weak.update(cx, |view, cx| {
                if view.modal_tree_state.close_select() {
                    cx.notify();
                    true
                } else {
                    false
                }
            }) {
                return true;
            }
        }
    }
    dismiss_active_plugin_modal(cx)
}

/// Dismisses any active plugin modal, cleanly resolving pending `Confirm`/`Prompt` promises
/// and unmounting `"__modal__"` view handlers.
pub fn dismiss_active_plugin_modal(cx: &mut App) -> bool {
    let has_modal = PluginBus::global(cx).is_some_and(|bus| bus.active_modal.is_some());
    if !has_modal {
        return false;
    }
    let prev = PluginBus::update(cx, |bus| bus.active_modal.take());
    PluginHost::bump_modal_generation(cx);
    if let Some(modal) = prev {
        resolve_displaced_modal(modal, None, cx);
        true
    } else {
        false
    }
}

fn resolve_and_close_prompt(request_id: u64, reply: PromptReplyValue, cx: &mut App) {
    PluginHost::resolve_prompt_async(request_id, reply, cx);
    let matches_request = PluginBus::global(cx)
        .and_then(|bus| bus.active_modal.as_ref())
        .is_some_and(|modal| match &modal.kind {
            PluginModalKind::Confirm {
                request_id: active_id,
                ..
            }
            | PluginModalKind::Prompt {
                request_id: active_id,
                ..
            } => *active_id == request_id,
            PluginModalKind::View { .. } => false,
        });
    if matches_request {
        PluginBus::update(cx, |bus| bus.close_modal());
        PluginHost::bump_modal_generation(cx);
    }
}

/// Renders active plugin modals (`View`, `Confirm`, `Prompt`) and the toast stack above the
/// currently active window (or primary mounted fallback window when none is active).
pub fn render_plugin_overlays(window: &mut Window, cx: &mut App) -> AnyElement {
    let view = window.use_keyed_state("plugin-overlays-root", cx, |window, cx| {
        PluginOverlayView::new_in_window(window, cx)
    });
    register_overlay_window(window.window_handle(), Some(view.downgrade()), cx);
    view.into_any_element()
}

fn render_toast_stack(toasts: &[PluginToast], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = theme.colors();
    let status = theme.status();

    v_flex()
        .id("plugin-toast-stack")
        .absolute()
        .bottom_4()
        .right_4()
        .w(px(340.))
        .max_w_full()
        .gap_2()
        .children(toasts.iter().map(|toast| {
            let (icon, accent) = match toast.level {
                ToastLevel::Info => (IconName::Info, status.info),
                ToastLevel::Success => (IconName::CircleCheck, status.success),
                ToastLevel::Warning => (IconName::CircleAlert, status.warning),
                ToastLevel::Error => (IconName::CircleAlert, status.error),
            };
            let toast_id = toast.id;
            h_flex()
                .id(ElementId::Name(SharedString::from(format!(
                    "plugin-toast-{toast_id}"
                ))))
                .occlude()
                .w_full()
                .p_3()
                .gap_2p5()
                .items_start()
                .rounded(px(8.))
                .bg(colors.elevated_surface_background)
                .border_1()
                .border_color(colors.border)
                .shadow_lg()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .pt(px(2.))
                        .child(Icon::new(icon).size(IconSize::Small).color(Color::Custom(accent))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(LabelSize::Small.pixels())
                                .text_color(colors.text)
                                .child(toast.message.clone()),
                        )
                        .when(!toast.plugin_id.is_empty(), |col| {
                            col.child(
                                Label::new(toast.plugin_id.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            )
                        }),
                )
                .child(
                    IconButton::new(
                        ElementId::Name(SharedString::from(format!(
                            "plugin-toast-dismiss-{toast_id}"
                        ))),
                        IconName::X,
                    )
                    .size(px(22.))
                    .icon_size(IconSize::XSmall)
                    .icon_color(Color::IconMuted)
                    .on_click(move |_, _, cx| {
                        PluginBus::update(cx, |bus| bus.dismiss_toast(toast_id));
                    }),
                )
        }))
        .into_any_element()
}
