//! Plugin modal overlays (`View`, `Confirm`, `Prompt`) and auto-expiring toast stack.

use std::collections::HashSet;
use std::rc::Rc;

use desktop_common::plugin_bus::{PluginBus, PluginModalKind, PluginModalSpec, PluginToast, ToastLevel};
use desktop_common::text_input::{Cancelled, Mode, Submitted, TextInput};
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, ElementId, Entity, Focusable as _, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div, px,
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

use crate::host::{PluginHost, schedule_toast_expiry};
use crate::view::{UiNotifySink, UiTreeState};

struct ActivePromptState {
    request_id: u64,
    input: Entity<TextInput>,
    _subs: [Subscription; 2],
}

/// Stateful GPUI overlay view rendering active plugin modals and the floating toast stack.
pub struct PluginOverlayView {
    prompt: Option<ActivePromptState>,
    modal_tree_key: Option<(String, String)>,
    modal_tree_state: UiTreeState,
    modal_scroll: gpui::ScrollHandle,
    scheduled_toasts: HashSet<u64>,
    _bus_sub: Subscription,
}

impl PluginOverlayView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let bus_sub = cx.observe_global::<PluginBus>(|_, cx| {
            cx.notify();
        });
        Self {
            prompt: None,
            modal_tree_key: None,
            modal_tree_state: UiTreeState::new(),
            modal_scroll: gpui::ScrollHandle::new(),
            scheduled_toasts: HashSet::new(),
            _bus_sub: bus_sub,
        }
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
            PluginHost::resolve_prompt_async(request_id, PromptReplyValue::Prompt(Some(text)), cx);
            PluginBus::update(cx, |bus| bus.close_modal());
        });
        let sub_cancel = cx.subscribe(&input, move |_, _, _: &Cancelled, cx| {
            PluginHost::resolve_prompt_async(request_id, PromptReplyValue::Prompt(None), cx);
            PluginBus::update(cx, |bus| bus.close_modal());
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let title = spec.title.clone();

        match &spec.kind {
            PluginModalKind::Confirm { body, request_id } => {
                let req_id = *request_id;
                ConfirmDialog::new("plugin-confirm-modal", title)
                    .description(body.clone())
                    .cancel_label(strings.cancel)
                    .confirm_label(strings.confirm)
                    .on_cancel(move |_, _, cx| {
                        PluginHost::resolve_prompt_async(
                            req_id,
                            PromptReplyValue::Confirm(false),
                            cx,
                        );
                        PluginBus::update(cx, |bus| bus.close_modal());
                    })
                    .on_confirm(move |_, _, cx| {
                        PluginHost::resolve_prompt_async(
                            req_id,
                            PromptReplyValue::Confirm(true),
                            cx,
                        );
                        PluginBus::update(cx, |bus| bus.close_modal());
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
                        PluginHost::resolve_prompt_async(
                            req_id,
                            PromptReplyValue::Prompt(None),
                            cx,
                        );
                        PluginBus::update(cx, |bus| bus.close_modal());
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
                                        PluginHost::resolve_prompt_async(
                                            req_id,
                                            PromptReplyValue::Prompt(None),
                                            cx,
                                        );
                                        PluginBus::update(cx, |bus| bus.close_modal());
                                    }),
                            )
                            .child(
                                Button::new("plugin-prompt-ok", strings.ok)
                                    .variant(ButtonVariant::Primary)
                                    .on_click(move |_, _, cx| {
                                        let val = input_confirm.read(cx).text().to_owned();
                                        PluginHost::resolve_prompt_async(
                                            req_id,
                                            PromptReplyValue::Prompt(Some(val)),
                                            cx,
                                        );
                                        PluginBus::update(cx, |bus| bus.close_modal());
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            PluginModalKind::View { tree } => {
                let next_key = (spec.plugin_id.clone(), spec.view_id.clone());
                if self.modal_tree_key.as_ref() != Some(&next_key) {
                    self.modal_tree_state.clear();
                    self.modal_tree_key = Some(next_key);
                }

                let parsed: Result<UiNode, _> = serde_json::from_value(tree.clone());
                let body = match parsed {
                    Ok(root) => {
                        let payload = UiTreePayload {
                            plugin_id: spec.plugin_id.clone(),
                            view_id: spec.view_id.clone(),
                            root,
                        };
                        let pid = spec.plugin_id.clone();
                        let sink = Rc::new(move |ev, cx: &mut App| {
                            PluginHost::dispatch_ui_event_async(pid.clone(), ev, cx);
                        });
                        let weak = cx.weak_entity();
                        let notify_sink: UiNotifySink = Rc::new(move |cx| {
                            let _ = weak.update(cx, |_, cx| cx.notify());
                        });
                        self.modal_tree_state
                            .render_tree(&payload, sink, notify_sink, cx)
                    }
                    Err(err) => Label::new(format!("{err}"))
                        .size(LabelSize::Small)
                        .color(Color::Error)
                        .into_any_element(),
                };

                Modal::new("plugin-view-modal")
                    .max_width(px(560.))
                    .max_height(px(620.))
                    .on_dismiss(|_, _, cx| {
                        PluginBus::update(cx, |bus| bus.close_modal());
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
                                        PluginBus::update(cx, |bus| bus.close_modal());
                                    }),
                            ),
                    )
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
        let (active_modal, toasts) = PluginBus::global(cx)
            .map(|bus| (bus.active_modal.clone(), bus.toasts.clone()))
            .unwrap_or((None, Vec::new()));

        let active_ids: HashSet<u64> = toasts.iter().map(|t| t.id).collect();
        self.scheduled_toasts.retain(|id| active_ids.contains(id));
        for toast in &toasts {
            if self.scheduled_toasts.insert(toast.id) {
                schedule_toast_expiry(toast.id, cx);
            }
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
        ) && self.modal_tree_key.is_some()
        {
            self.modal_tree_key = None;
            self.modal_tree_state.clear();
        }

        if active_modal.is_none() && toasts.is_empty() {
            return div().into_any_element();
        }

        let modal_el = active_modal.map(|spec| self.render_modal(&spec, window, cx));
        let toast_stack = (!toasts.is_empty()).then(|| render_toast_stack(&toasts, cx));

        div()
            .absolute()
            .inset_0()
            .children(modal_el)
            .children(toast_stack)
            .into_any_element()
    }
}

/// Renders active plugin modals (`View`, `Confirm`, `Prompt`) and the toast stack above the window.
pub fn render_plugin_overlays(window: &mut Window, cx: &mut App) -> AnyElement {
    let view = window.use_keyed_state("plugin-overlays-root", cx, |_, cx| {
        PluginOverlayView::new(cx)
    });
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
