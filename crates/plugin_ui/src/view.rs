//! Declarative [`UiNode`] GPUI renderer and [`PluginView`] entity.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use desktop_common::text_input::{Changed, Mode, Submitted, TextInput};
use fern_core::markdown::{BlockKind, Style as MdStyle, blocks as parse_markdown_blocks};
use fern_i18n as i18n;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, FontStyle, FontWeight, HighlightStyle, Hsla, InteractiveElement, InteractiveText,
    IntoElement, ObjectFit, ParentElement, Render, RenderImage, ScrollHandle, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Styled, StyledText, Subscription,
    UnderlineStyle, Window, div, px, relative,
};
use icons::IconName;
use plugins::PluginStatus;
use plugins::ui_ast::{
    HandlerId, UiAlign, UiButtonSize, UiButtonVariant, UiEvent, UiJustify, UiNode, UiTextSize,
    UiTreePayload, UiWeight, sanitize_svg,
};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Checkbox, Chip, Color, Divider, Donut, EmptyState, Icon,
    IconButton, IconSize, Label, LabelSize, ListItem, Placement, PopoverAttachment, SegmentedTab,
    SegmentedTabs, Switch, Tooltip, popover, v_scroll,
};

use crate::host::PluginHost;

const INPUT_DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_SVG_CACHE_ENTRIES: usize = 128;

/// Callback invoked whenever an interactive [`UiNode`] widget emits a [`UiEvent`].
pub type UiEventSink = Rc<dyn Fn(UiEvent, &mut App)>;

/// Callback invoked when [`UiTreeState`] internal UI state (e.g. an open select popover) changes.
pub type UiNotifySink = Rc<dyn Fn(&mut App)>;

/// Events emitted by [`PluginView`] to parent shell containers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginViewEvent {
    OpenLogs { plugin_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SvgCacheKey {
    content_hash: u64,
    width_px: u32,
    height_px: u32,
    ink_rgba: u32,
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
    read_only: bool,
    shared: Rc<RefCell<InputCallbacks>>,
    used: bool,
    _subs: [Subscription; 2],
}

/// Stateful GPUI widget state for a rendered declarative [`UiTreePayload`] tree.
///
/// Preserves [`TextInput`] focus and cursor position across re-renders via `revision` guards,
/// persists [`ScrollHandle`] instances by `Scroll.key`, and caches rasterized SVG [`RenderImage`]s.
pub struct UiTreeState {
    inputs: HashMap<String, InputSlot>,
    scrolls: HashMap<String, ScrollHandle>,
    open_select: Rc<RefCell<Option<String>>>,
    svg_cache: HashMap<SvgCacheKey, Arc<RenderImage>>,
    node_counter: usize,
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
            svg_cache: HashMap::new(),
            node_counter: 0,
        }
    }

    pub fn clear(&mut self) {
        self.inputs.clear();
        self.scrolls.clear();
        *self.open_select.borrow_mut() = None;
        self.svg_cache.clear();
    }

    pub fn input_entity(&self, key: &str) -> Option<&Entity<TextInput>> {
        self.inputs.get(key).map(|slot| &slot.entity)
    }

    pub fn scroll_handle(&self, key: &str) -> Option<&ScrollHandle> {
        self.scrolls.get(key)
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
        self.node_counter = 0;

        let element = self.render_node(
            &payload.root,
            &payload.view_id,
            false,
            &event_sink,
            &notify_sink,
            cx,
        );

        self.inputs.retain(|_, slot| slot.used);
        element
    }

    fn next_id(&mut self, prefix: &str) -> ElementId {
        self.node_counter += 1;
        ElementId::Name(SharedString::from(format!("{prefix}-{}", self.node_counter)))
    }

    fn ensure_scroll(&mut self, key: &str) -> ScrollHandle {
        self.scrolls.entry(key.to_owned()).or_default().clone()
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
                let text = value.to_owned();
                slot.entity.update(cx, |input, cx| {
                    input.set_text(text, cx);
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
            let shared_inner = Rc::clone(&shared_changed);
            let task = cx.spawn(async move |cx| {
                cx.background_executor().timer(INPUT_DEBOUNCE).await;
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
            let (vid, hid, sink_fn) = {
                let mut cb = shared_submit.borrow_mut();
                cb.debounce_task = None;
                (cb.view_id.clone(), cb.on_submit, Rc::clone(&cb.sink))
            };
            if let Some(handler_id) = hid {
                sink_fn(UiEvent::with_value(vid, handler_id, ev.0.clone()), cx);
            }
        });

        self.inputs.insert(
            key.to_owned(),
            InputSlot {
                entity: entity.clone(),
                revision,
                mode,
                read_only: disabled,
                shared,
                used: true,
                _subs: [sub_changed, sub_submit],
            },
        );

        entity
    }

    fn render_node(
        &mut self,
        node: &UiNode,
        view_id: &str,
        in_row: bool,
        sink: &UiEventSink,
        notify_sink: &UiNotifySink,
        cx: &mut App,
    ) -> AnyElement {
        match node {
            UiNode::VStack {
                gap,
                padding,
                align,
                children,
            } => {
                let vstack_idx = self.node_counter;
                self.node_counter += 1;
                let mut el = v_flex()
                    .min_w_0()
                    .when(in_row, |el| el.flex_1())
                    .when(!in_row, |el| el.w_full())
                    .when(cfg!(test), |el| {
                        el.debug_selector(move || format!("plugin-vstack-{vstack_idx}"))
                    })
                    .gap(px(gap.unwrap_or(8.0).max(0.0)));
                if let Some(p) = padding {
                    el = el.p(px(p.max(0.0)));
                }
                el = apply_align_div(el, *align);
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, false, sink, notify_sink, cx))
                    .collect();
                el.children(rendered).into_any_element()
            }
            UiNode::HStack {
                gap,
                padding,
                align,
                justify,
                wrap,
                children,
            } => {
                let hstack_idx = self.node_counter;
                self.node_counter += 1;
                let mut el = h_flex()
                    .min_w_0()
                    .when(!in_row, |el| el.w_full())
                    .when(in_row && !*wrap, |el| el.flex_none())
                    .when(cfg!(test), |el| {
                        el.debug_selector(move || format!("plugin-hstack-{hstack_idx}"))
                    })
                    .gap(px(gap.unwrap_or(8.0).max(0.0)));
                if let Some(p) = padding {
                    el = el.p(px(p.max(0.0)));
                }
                if let Some(a) = align {
                    el = match a {
                        UiAlign::Start => el.items_start(),
                        UiAlign::Center => el.items_center(),
                        UiAlign::End => el.items_end(),
                        UiAlign::Stretch => el.items_stretch(),
                    };
                }
                if let Some(j) = justify {
                    el = match j {
                        UiJustify::Start => el.justify_start(),
                        UiJustify::Center => el.justify_center(),
                        UiJustify::End => el.justify_end(),
                        UiJustify::Between => el.justify_between(),
                    };
                }
                if *wrap {
                    el = el.flex_wrap();
                }
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, true, sink, notify_sink, cx))
                    .collect();
                el.children(rendered).into_any_element()
            }
            UiNode::Grid {
                cols,
                gap,
                children,
            } => {
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, false, sink, notify_sink, cx))
                    .collect();
                div()
                    .w_full()
                    .grid()
                    .grid_cols((*cols).max(1))
                    .gap(px(gap.unwrap_or(8.0).max(0.0)))
                    .children(rendered)
                    .into_any_element()
            }
            UiNode::Scroll {
                key,
                max_height,
                child,
                children,
            } => {
                let handle = self.ensure_scroll(key);
                let scroll_id = ElementId::Name(SharedString::from(format!("plugin-scroll-{key}")));
                let viewport_id =
                    ElementId::Name(SharedString::from(format!("plugin-scroll-vp-{key}")));
                let mut inner_items = Vec::new();
                if let Some(c) = child {
                    inner_items.push(self.render_node(c, view_id, false, sink, notify_sink, cx));
                }
                for c in children {
                    inner_items.push(self.render_node(c, view_id, false, sink, notify_sink, cx));
                }
                let viewport = v_flex().id(viewport_id).w_full().gap_2().children(inner_items);
                let mut scroll = v_scroll(scroll_id, viewport).track_scroll(&handle).w_full();
                if let Some(mh) = max_height {
                    scroll = scroll.max_h(px(mh.max(24.0)));
                } else {
                    scroll = scroll.flex_1().min_h_0();
                }
                scroll.into_any_element()
            }
            UiNode::Card {
                title,
                subtitle,
                padding,
                children,
            } => {
                let colors = cx.theme().colors().clone();
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, false, sink, notify_sink, cx))
                    .collect();
                v_flex()
                    .when(in_row, |el| el.flex_1())
                    .when(!in_row, |el| el.w_full())
                    .min_w_0()
                    .rounded(px(8.))
                    .bg(colors.surface_background)
                    .border_1()
                    .border_color(colors.border_variant)
                    .p(px(padding.unwrap_or(14.0).max(0.0)))
                    .gap_2()
                    .when_some(title.clone(), |el, title_text| {
                        el.child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    Label::new(title_text)
                                        .size(LabelSize::Default)
                                        .weight(FontWeight::SEMIBOLD),
                                )
                                .when_some(subtitle.clone(), |col, sub| {
                                    col.child(
                                        Label::new(sub)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                }),
                        )
                    })
                    .children(rendered)
                    .into_any_element()
            }
            UiNode::Section {
                title,
                subtitle,
                children,
            } => {
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, false, sink, notify_sink, cx))
                    .collect();
                v_flex()
                    .when(in_row, |el| el.flex_1())
                    .when(!in_row, |el| el.w_full())
                    .min_w_0()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Label::new(title.clone())
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::SEMIBOLD)
                                    .color(Color::Muted),
                            )
                            .when_some(subtitle.clone(), |col, sub| {
                                col.child(
                                    Label::new(sub)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                )
                            }),
                    )
                    .child(v_flex().w_full().min_w_0().gap_2().children(rendered))
                    .into_any_element()
            }
            UiNode::Divider => Divider::horizontal().into_any_element(),
            UiNode::Spacer { size } => match size {
                Some(s) => div().flex_none().size(px(s.max(0.0))).into_any_element(),
                None => div().flex_1().into_any_element(),
            },
            UiNode::Heading { text, level } => {
                let colors = cx.theme().colors();
                let font_px = match level {
                    1 => 20.,
                    2 => 17.,
                    3 => 15.,
                    _ => 14.,
                };
                div()
                    .min_w_0()
                    .when(in_row, |el| el.flex_1())
                    .text_size(px(font_px))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child(text.clone())
                    .into_any_element()
            }
            UiNode::Text {
                text,
                muted,
                weight,
                color,
                size,
            } => {
                let colors = cx.theme().colors();
                let resolved_color = color
                    .as_deref()
                    .and_then(|c| resolve_ui_color(c, cx))
                    .unwrap_or(if *muted {
                        colors.text_muted
                    } else {
                        colors.text
                    });
                let font_px = match size.unwrap_or_default() {
                    UiTextSize::Xs => 11.,
                    UiTextSize::Sm => 12.,
                    UiTextSize::Md => 14.,
                    UiTextSize::Lg => 17.,
                };
                let font_weight = match weight.unwrap_or_default() {
                    UiWeight::Normal => FontWeight::NORMAL,
                    UiWeight::Medium => FontWeight::MEDIUM,
                    UiWeight::Semibold => FontWeight::SEMIBOLD,
                    UiWeight::Bold => FontWeight::BOLD,
                };
                div()
                    .min_w_0()
                    .when(in_row, |el| el.flex_1())
                    .text_size(px(font_px))
                    .font_weight(font_weight)
                    .text_color(resolved_color)
                    .child(text.clone())
                    .into_any_element()
            }
            UiNode::Markdown { content } => render_markdown_node(content, cx),
            UiNode::KeyValue {
                label,
                value,
                muted,
            } => h_flex()
                .w_full()
                .min_w_0()
                .justify_between()
                .gap_3()
                .child(
                    Label::new(label.clone())
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Label::new(value.clone())
                        .size(LabelSize::Small)
                        .color(if *muted {
                            Color::Muted
                        } else {
                            Color::Default
                        })
                        .truncate(),
                )
                .into_any_element(),
            UiNode::Badge { label, color } => {
                let resolved = color
                    .as_deref()
                    .and_then(|c| resolve_ui_color(c, cx))
                    .unwrap_or(cx.theme().colors().accent);
                Chip::new(label.clone(), resolved).into_any_element()
            }
            UiNode::Progress {
                value,
                label,
                color,
            } => {
                let colors = cx.theme().colors();
                let frac = normalize_progress(*value);
                let fill_color = color
                    .as_deref()
                    .and_then(|c| resolve_ui_color(c, cx))
                    .unwrap_or(colors.accent);
                v_flex()
                    .w_full()
                    .gap_1()
                    .when_some(label.clone(), |col, lbl| {
                        col.child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .child(Label::new(lbl).size(LabelSize::XSmall).color(Color::Muted))
                                .child(
                                    Label::new(format!("{}%", (frac * 100.0).round() as u32))
                                        .size(LabelSize::XSmall)
                                        .color(Color::Muted),
                                ),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .bg(colors.element_hover)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(frac))
                                    .rounded_full()
                                    .bg(fill_color),
                            ),
                    )
                    .into_any_element()
            }
            UiNode::Donut {
                value,
                label,
                size,
                color,
            } => {
                let colors = cx.theme().colors();
                let frac = normalize_progress(*value);
                let fill_color = color
                    .as_deref()
                    .and_then(|c| resolve_ui_color(c, cx))
                    .unwrap_or(colors.accent);
                let px_size = size.unwrap_or(56.0).clamp(24.0, 220.0);
                let segments = if frac > 0.0 {
                    vec![
                        (frac, fill_color),
                        ((1.0 - frac).max(0.0), gpui::transparent_black()),
                    ]
                } else {
                    Vec::new()
                };
                let mut donut = Donut::new(segments)
                    .size(px(px_size))
                    .thickness(px((px_size * 0.14).clamp(4.0, 14.0)));
                let center_text = label
                    .clone()
                    .unwrap_or_else(|| format!("{}%", (frac * 100.0).round() as u32));
                let put_below = center_text.chars().count() > 6;
                if !put_below {
                    donut = donut.center(
                        Label::new(center_text.clone())
                            .size(LabelSize::XSmall)
                            .weight(FontWeight::MEDIUM),
                    );
                }
                if put_below {
                    v_flex()
                        .items_center()
                        .gap_1()
                        .child(donut)
                        .child(
                            Label::new(center_text)
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                        .into_any_element()
                } else {
                    donut.into_any_element()
                }
            }
            UiNode::EmptyState {
                icon,
                title,
                subtitle,
                action_label,
                on_action,
            } => {
                let mut empty = EmptyState::new(
                    resolve_icon_name(icon.as_deref()),
                    subtitle.clone().unwrap_or_default(),
                )
                .title(title.clone());
                if let (Some(lbl), Some(hid)) = (action_label, on_action) {
                    let action_id = self.next_id("plugin-empty-action");
                    let vid = view_id.to_owned();
                    let hid = *hid;
                    let sink = Rc::clone(sink);
                    empty = empty.action(action_id, lbl.clone(), move |_, _, cx| {
                        sink(UiEvent::click(vid.clone(), hid), cx);
                    });
                }
                empty.into_any_element()
            }
            UiNode::Svg {
                content,
                width,
                height,
            } => render_svg_node(content, *width, *height, &mut self.svg_cache, cx),
            UiNode::Button {
                label,
                variant,
                size,
                icon,
                disabled,
                loading,
                on_click,
            } => {
                let btn_id = self.next_id("plugin-btn");
                let text = if *loading {
                    format!("{label}…")
                } else {
                    label.clone()
                };
                let mut btn = Button::new(btn_id, text)
                    .variant(match variant {
                        UiButtonVariant::Primary => ButtonVariant::Primary,
                        UiButtonVariant::Secondary => ButtonVariant::Secondary,
                        UiButtonVariant::Destructive => ButtonVariant::Destructive,
                        UiButtonVariant::Ghost => ButtonVariant::Ghost,
                    })
                    .size(match size {
                        UiButtonSize::Sm => ButtonSize::Sm,
                        UiButtonSize::Md => ButtonSize::Md,
                    })
                    .disabled(*disabled || *loading);
                if let Some(icon_name) = icon {
                    btn = btn.icon(resolve_icon_name(Some(icon_name)));
                }
                if let Some(hid) = on_click {
                    let vid = view_id.to_owned();
                    let hid = *hid;
                    let sink = Rc::clone(sink);
                    btn = btn.on_click(move |_, _, cx| {
                        sink(UiEvent::click(vid.clone(), hid), cx);
                    });
                }
                btn.into_any_element()
            }
            UiNode::IconButton {
                icon,
                tooltip,
                disabled,
                on_click,
            } => {
                let btn_id = self.next_id("plugin-icon-btn");
                let mut btn = IconButton::new(btn_id, resolve_icon_name(Some(icon)))
                    .disabled(*disabled);
                if let Some(tip) = tooltip {
                    btn = btn.tooltip(Tooltip::text(tip.clone()));
                }
                if let Some(hid) = on_click {
                    let vid = view_id.to_owned();
                    let hid = *hid;
                    let sink = Rc::clone(sink);
                    btn = btn.on_click(move |_, _, cx| {
                        sink(UiEvent::click(vid.clone(), hid), cx);
                    });
                }
                btn.into_any_element()
            }
            UiNode::Input {
                key,
                value,
                revision,
                placeholder,
                disabled,
                on_change,
                on_submit,
            } => {
                let entity = self.ensure_input(
                    key,
                    view_id,
                    Mode::SingleLine,
                    value,
                    *revision,
                    placeholder.as_deref().unwrap_or(""),
                    *disabled,
                    *on_change,
                    *on_submit,
                    sink,
                    cx,
                );
                let colors = cx.theme().colors();
                div()
                    .when(in_row, |el| el.flex_1())
                    .when(!in_row, |el| el.w_full())
                    .min_w_0()
                    .h(px(30.))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .rounded(px(6.))
                    .bg(colors.background)
                    .border_1()
                    .border_color(colors.border)
                    .when(*disabled, |el| el.opacity(0.6))
                    .when(cfg!(test), |el| {
                        let sel = format!("plugin-input-{key}");
                        el.debug_selector(move || sel)
                    })
                    .child(entity)
                    .into_any_element()
            }
            UiNode::TextArea {
                key,
                value,
                revision,
                rows,
                placeholder,
                disabled,
                on_change,
            } => {
                let entity = self.ensure_input(
                    key,
                    view_id,
                    Mode::MultiLine,
                    value,
                    *revision,
                    placeholder.as_deref().unwrap_or(""),
                    *disabled,
                    *on_change,
                    None,
                    sink,
                    cx,
                );
                let colors = cx.theme().colors();
                let height_px = (f32::from((*rows).clamp(2, 24)) * 20.0 + 12.0).clamp(52.0, 480.0);
                div()
                    .w_full()
                    .min_w_0()
                    .h(px(height_px))
                    .p_2()
                    .overflow_hidden()
                    .rounded(px(6.))
                    .bg(colors.background)
                    .border_1()
                    .border_color(colors.border)
                    .when(*disabled, |el| el.opacity(0.6))
                    .child(entity)
                    .into_any_element()
            }
            UiNode::Select {
                key,
                value,
                options,
                disabled,
                on_change,
            } => {
                let colors = cx.theme().colors().clone();
                let is_open = self.open_select.borrow().as_deref() == Some(key.as_str());
                let active_label = options
                    .iter()
                    .find(|o| o.value == *value)
                    .map(|o| o.label.clone())
                    .unwrap_or_else(|| value.clone());
                let select_id = ElementId::Name(SharedString::from(format!("plugin-select-{key}")));
                let open_ref = Rc::clone(&self.open_select);
                let key_own = key.clone();
                let notify_c = Rc::clone(notify_sink);

                let menu = if is_open && !*disabled {
                    let items: Vec<AnyElement> = options
                        .iter()
                        .enumerate()
                        .map(|(idx, opt)| {
                            let selected = opt.value == *value;
                            let opt_val = opt.value.clone();
                            let open_ref_item = Rc::clone(&open_ref);
                            let notify_item = Rc::clone(notify_sink);
                            let sink_item = Rc::clone(sink);
                            let vid = view_id.to_owned();
                            let on_chg = *on_change;
                            let mut item = ListItem::new(ElementId::Name(SharedString::from(
                                format!("plugin-select-{key}-{idx}"),
                            )))
                            .toggle_state(selected);
                            if selected {
                                item = item.end_slot(
                                    Icon::new(IconName::Check)
                                        .size(IconSize::XSmall)
                                        .color(Color::Accent),
                                );
                            }
                            item.on_click(move |_, _, cx| {
                                *open_ref_item.borrow_mut() = None;
                                notify_item(cx);
                                if let Some(hid) = on_chg {
                                    sink_item(
                                        UiEvent::with_value(vid.clone(), hid, opt_val.clone()),
                                        cx,
                                    );
                                }
                            })
                            .child(Label::new(opt.label.clone()).size(LabelSize::Small).truncate())
                            .into_any_element()
                        })
                        .collect();
                    Some(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx).min_w(px(160.)).max_w(px(280.)).children(items),
                    ))
                } else {
                    None
                };

                div()
                    .relative()
                    .child(
                        h_flex()
                            .id(select_id)
                            .h(px(30.))
                            .px_2p5()
                            .gap_2()
                            .justify_between()
                            .rounded(px(6.))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border)
                            .when(*disabled, |el| el.opacity(0.5).cursor_default())
                            .when(!*disabled, |el| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(colors.element_hover))
                                    .on_click(move |_, _, cx| {
                                        let mut guard = open_ref.borrow_mut();
                                        if guard.as_deref() == Some(key_own.as_str()) {
                                            *guard = None;
                                        } else {
                                            *guard = Some(key_own.clone());
                                        }
                                        drop(guard);
                                        notify_c(cx);
                                    })
                            })
                            .child(Label::new(active_label).size(LabelSize::Small).truncate())
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .size(IconSize::XSmall)
                                    .color(Color::IconMuted),
                            ),
                    )
                    .children(menu)
                    .into_any_element()
            }
            UiNode::Checkbox {
                key,
                checked,
                label,
                disabled,
                on_change,
            } => {
                let row_id = match key {
                    Some(k) => ElementId::Name(SharedString::from(format!("plugin-check-{k}"))),
                    None => self.next_id("plugin-check"),
                };
                let cb_id = (row_id.clone(), "box");
                let cb = Checkbox::new(cb_id).accent().toggle_state(*checked);
                let next_val = !*checked;
                let vid = view_id.to_owned();
                let on_chg = *on_change;
                let sink = Rc::clone(sink);

                h_flex()
                    .id(row_id)
                    .gap_2()
                    .when(*disabled, |el| el.opacity(0.5).cursor_default())
                    .when(!*disabled, |el| {
                        el.cursor_pointer().on_click(move |_, _, cx| {
                            if let Some(hid) = on_chg {
                                sink(UiEvent::with_value(vid.clone(), hid, next_val), cx);
                            }
                        })
                    })
                    .child(cb)
                    .when_some(label.clone(), |row, lbl| {
                        row.child(Label::new(lbl).size(LabelSize::Small))
                    })
                    .into_any_element()
            }
            UiNode::Switch {
                key,
                checked,
                label,
                disabled,
                on_change,
            } => {
                let row_id = match key {
                    Some(k) => ElementId::Name(SharedString::from(format!("plugin-switch-{k}"))),
                    None => self.next_id("plugin-switch"),
                };
                let next_val = !*checked;
                let vid = view_id.to_owned();
                let on_chg = *on_change;
                let sink = Rc::clone(sink);

                h_flex()
                    .id(row_id)
                    .gap_2()
                    .justify_between()
                    .when(*disabled, |el| el.opacity(0.5).cursor_default())
                    .when(!*disabled, |el| {
                        el.cursor_pointer().on_click(move |_, _, cx| {
                            if let Some(hid) = on_chg {
                                sink(UiEvent::with_value(vid.clone(), hid, next_val), cx);
                            }
                        })
                    })
                    .when_some(label.clone(), |row, lbl| {
                        row.child(Label::new(lbl).size(LabelSize::Small))
                    })
                    .child(Switch::new().toggle_state(*checked))
                    .into_any_element()
            }
            UiNode::SegmentedTabs {
                key,
                value,
                options,
                on_change,
            } => {
                let prefix = key.as_deref().unwrap_or("seg");
                let mut tabs = SegmentedTabs::new();
                for (idx, opt) in options.iter().enumerate() {
                    let tab_id = ElementId::Name(SharedString::from(format!(
                        "plugin-tab-{prefix}-{idx}"
                    )));
                    let mut tab = SegmentedTab::new(tab_id, opt.label.clone())
                        .toggle_state(opt.value == *value);
                    if let Some(hid) = on_change {
                        let vid = view_id.to_owned();
                        let val = opt.value.clone();
                        let hid = *hid;
                        let sink = Rc::clone(sink);
                        tab = tab.on_click(move |_, _, cx| {
                            sink(UiEvent::with_value(vid.clone(), hid, val.clone()), cx);
                        });
                    }
                    tabs = tabs.tab(tab);
                }
                tabs.into_any_element()
            }
            UiNode::ListItem {
                key,
                title,
                subtitle,
                badge,
                trailing,
                selected,
                on_click,
            } => {
                let colors = cx.theme().colors();
                let item_id = ElementId::Name(SharedString::from(format!("plugin-item-{key}")));
                let height = if subtitle.is_some() { px(46.) } else { px(34.) };
                let mut item = ListItem::new(item_id).height(height).toggle_state(*selected);

                if badge.is_some() || trailing.is_some() {
                    let end = h_flex()
                        .gap_2()
                        .when_some(badge.clone(), |row, b| {
                            row.child(Chip::new(b, colors.accent))
                        })
                        .when_some(trailing.clone(), |row, tr| {
                            row.child(Label::new(tr).size(LabelSize::Small).color(Color::Muted))
                        });
                    item = item.end_slot(end);
                }
                if let Some(hid) = on_click {
                    let vid = view_id.to_owned();
                    let hid = *hid;
                    let sink = Rc::clone(sink);
                    item = item.on_click(move |_, _, cx| {
                        sink(UiEvent::click(vid.clone(), hid), cx);
                    });
                }

                item.child(
                    v_flex()
                        .min_w_0()
                        .child(Label::new(title.clone()).size(LabelSize::Small).truncate())
                        .when_some(subtitle.clone(), |col, sub| {
                            col.child(
                                Label::new(sub)
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .truncate(),
                            )
                        }),
                )
                .into_any_element()
            }
        }
    }
}

/// Native GPUI entity hosting a sidebar plugin view (`fern.views.register`).
pub struct PluginView {
    plugin_id: String,
    view_id: String,
    focus_handle: FocusHandle,
    tree_state: UiTreeState,
    seen_plugin_revision: u64,
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

        let should_mount = PluginHost::global(cx).is_some_and(|h| {
            !h.is_safe_mode()
                && h.view_tree(&plugin_id, &view_id).is_none()
                && h.view_error(&plugin_id, &view_id).is_none()
        });
        if should_mount {
            PluginHost::mount_view_async(plugin_id.clone(), view_id.clone(), cx);
        }

        Self {
            plugin_id,
            view_id,
            focus_handle: cx.focus_handle(),
            tree_state: UiTreeState::new(),
            seen_plugin_revision,
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
        PluginHost::unmount_view_async(self.plugin_id.clone(), self.view_id.clone(), cx);
        self.plugin_id = next_plugin;
        self.view_id = next_view;
        self.tree_state.clear();
        self.seen_plugin_revision = PluginHost::global(cx)
            .map_or(0, |h| h.plugin_revision(&self.plugin_id));
        if PluginHost::global(cx).is_some_and(|h| {
            !h.is_safe_mode() && h.view_tree(&self.plugin_id, &self.view_id).is_none()
        }) {
            PluginHost::mount_view_async(self.plugin_id.clone(), self.view_id.clone(), cx);
        }
        cx.notify();
    }

    pub fn unmount(&self, cx: &mut App) {
        PluginHost::unmount_view_async(self.plugin_id.clone(), self.view_id.clone(), cx);
    }

    fn on_host_updated(&mut self, cx: &mut Context<Self>) {
        let Some(host) = PluginHost::global(cx) else {
            return;
        };
        let rev = host.plugin_revision(&self.plugin_id);
        if rev != self.seen_plugin_revision {
            self.seen_plugin_revision = rev;
            let enabled = host
                .plugin(&self.plugin_id)
                .is_some_and(|p| p.enabled);
            if enabled && !host.is_safe_mode() {
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
                        .on_click(move |_, _, cx| {
                            if let Some(mgr) =
                                PluginHost::global(cx).and_then(|h| h.manager().cloned())
                            {
                                let pid = reset_pid.clone();
                                let vid = reset_vid.clone();
                                let res_pid = reset_pid.clone();
                                let res_vid = reset_vid.clone();
                                let task = cx.background_spawn(async move {
                                    mgr.reset_and_enable_plugin(&pid)?;
                                    mgr.mount_view(&pid, &vid)
                                });
                                cx.spawn(async move |cx| {
                                    let res = task.await;
                                    cx.update(|cx| match res {
                                        Ok(tree) => PluginHost::set_view_tree(
                                            res_pid, res_vid, tree, cx,
                                        ),
                                        Err(err) => PluginHost::set_view_error(
                                            res_pid,
                                            res_vid,
                                            format!("{err:#}"),
                                            cx,
                                        ),
                                    });
                                })
                                .detach();
                            }
                        }),
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
                    .on_click(move |_, _, cx| {
                        PluginHost::clear_view_error(&reload_pid, &reload_vid, cx);
                        PluginHost::reload_plugin_async(reload_pid.clone(), cx);
                        PluginHost::mount_view_async(reload_pid.clone(), reload_vid.clone(), cx);
                    }),
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
            .and_then(|h| h.view_tree(&self.plugin_id, &self.view_id).cloned());
        let view_error = PluginHost::global(cx)
            .and_then(|h| h.view_error(&self.plugin_id, &self.view_id).map(str::to_owned));
        let plugin_status = PluginHost::global(cx)
            .and_then(|h| h.plugin(&self.plugin_id))
            .map(|p| p.status.clone());

        if let Some(err) = view_error {
            return self.render_error_fallback(err, cx);
        }

        match plugin_status {
            Some(PluginStatus::Disabled) => {
                return self.render_error_fallback(strings.plugins_disable.to_owned(), cx);
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

fn apply_align_div(el: gpui::Div, align: Option<UiAlign>) -> gpui::Div {
    match align {
        Some(UiAlign::Start) => el.items_start(),
        Some(UiAlign::Center) => el.items_center(),
        Some(UiAlign::End) => el.items_end(),
        Some(UiAlign::Stretch) | None => el.items_stretch(),
    }
}

fn normalize_progress(value: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    if value > 1.0 {
        (value / 100.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// Parses hex colors (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`) and semantic theme color names.
pub(crate) fn resolve_ui_color(raw: &str, cx: &App) -> Option<Hsla> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let theme = cx.theme();
    match trimmed.to_ascii_lowercase().as_str() {
        "accent" | "primary" => Some(theme.colors().accent),
        "muted" => Some(theme.colors().text_muted),
        "text" | "default" => Some(theme.colors().text),
        "error" | "danger" | "red" => Some(theme.status().error),
        "warning" | "warn" | "amber" | "yellow" => Some(theme.status().warning),
        "info" | "blue" => Some(theme.status().info),
        "success" | "green" => Some(theme.status().success),
        _ => theme::try_parse_color(trimmed).ok(),
    }
}

/// Maps plugin icon strings (Lucide / Fern names) to bundled [`IconName`] variants.
pub(crate) fn resolve_icon_name(raw: Option<&str>) -> IconName {
    let Some(name) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return IconName::Inbox;
    };
    let norm = name.to_ascii_lowercase().replace('_', "-");
    match norm.as_str() {
        "plus" | "add" => IconName::Plus,
        "check" => IconName::Check,
        "circle-check" | "check-circle" => IconName::CircleCheck,
        "circle-alert" | "alert-circle" | "alert" | "warning" => IconName::CircleAlert,
        "info" => IconName::Info,
        "search" => IconName::Search,
        "settings" | "gear" => IconName::Settings,
        "sliders" | "settings-2" => IconName::Settings2,
        "refresh" | "refresh-cw" | "reload" | "sync" => IconName::RefreshCw,
        "trash" | "delete" | "remove" => IconName::Trash,
        "pencil" | "edit" => IconName::Pencil,
        "folder" => IconName::Folder,
        "folder-open" => IconName::FolderOpen,
        "file" | "file-text" | "doc" | "logs" => IconName::FileText,
        "calendar" | "calendar-days" => IconName::CalendarDays,
        "clock" => IconName::Clock,
        "timer" => IconName::Timer,
        "alarm" | "alarm-clock" => IconName::AlarmClock,
        "bell" | "notification" => IconName::Bell,
        "tag" => IconName::Tag,
        "flag" => IconName::Flag,
        "flame" | "fire" => IconName::Flame,
        "zap" | "bolt" | "flash" => IconName::Zap,
        "pin" => IconName::Pin,
        "link" | "external-link" => IconName::Link,
        "copy" => IconName::Copy,
        "play" | "run" => IconName::Play,
        "pause" => IconName::Pause,
        "x" | "close" => IconName::X,
        "list" => IconName::List,
        "list-checks" | "checklist" => IconName::ListChecks,
        "grid" | "layout-grid" => IconName::LayoutGrid,
        "code" => IconName::Code,
        "image" | "svg" => IconName::Image,
        "eye" => IconName::Eye,
        "eye-off" => IconName::EyeOff,
        "lightbulb" | "tip" => IconName::Lightbulb,
        "history" => IconName::History,
        "inbox" => IconName::Inbox,
        _ => IconName::Info,
    }
}

fn render_markdown_node(content: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let accent = theme.colors().text_accent;
    let muted = theme.colors().text_muted;
    let blocks = parse_markdown_blocks(content);

    let build_styled = |text: String, runs: Vec<(std::ops::Range<usize>, MdStyle)>| {
        let highlights: Vec<_> = runs
            .into_iter()
            .map(|(range, style)| {
                let mut h = HighlightStyle::default();
                if style.bold {
                    h.font_weight = Some(FontWeight::BOLD);
                }
                if style.italic {
                    h.font_style = Some(FontStyle::Italic);
                }
                if style.strike {
                    h.strikethrough = Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    });
                }
                if style.underline || style.link.is_some() {
                    h.underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    });
                }
                if style.link.is_some() {
                    h.color = Some(accent);
                }
                if style.highlight {
                    h.background_color = Some(theme.status().warning.opacity(0.32));
                }
                if style.code {
                    h.background_color = Some(theme.colors().element_hover);
                }
                (range, h)
            })
            .collect();
        StyledText::new(text).with_highlights(highlights)
    };

    v_flex()
        .w_full()
        .min_w_0()
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(theme.colors().text)
        .children(blocks.into_iter().enumerate().map(|(ix, block)| {
            let links: Vec<_> = block
                .runs
                .iter()
                .filter_map(|(range, style)| {
                    let url = style.link.as_ref()?;
                    let lower = url.to_ascii_lowercase();
                    (lower.starts_with("https://") || lower.starts_with("http://"))
                        .then(|| (range.clone(), url.clone()))
                })
                .collect();
            let ranges = links.iter().map(|(range, _)| range.clone()).collect();
            let styled_text = build_styled(block.text, block.runs);
            let layout = styled_text.layout().clone();
            let interactive = InteractiveText::new(("plugin-md-block", ix), styled_text)
                .on_click(ranges, |_, _, _| {});
            let body = div()
                .id(("plugin-md-body", ix))
                .flex_1()
                .min_w_0()
                .child(interactive)
                .when(!links.is_empty(), |el| {
                    el.on_click(move |event, _, cx| {
                        let Ok(at) = layout.index_for_position(event.position()) else {
                            return;
                        };
                        if let Some((_, url)) =
                            links.iter().find(|(range, _)| range.contains(&at))
                        {
                            cx.stop_propagation();
                            cx.open_url(url);
                        }
                    })
                });

            match block.kind {
                BlockKind::BlankLine => div().h(px(10.)).flex_none().into_any_element(),
                BlockKind::Heading(level) => div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(match level {
                        1 => 19.,
                        2 => 16.,
                        _ => 15.,
                    }))
                    .child(body)
                    .into_any_element(),
                BlockKind::Numbered(n) => h_flex()
                    .items_start()
                    .child(
                        div()
                            .w(px(20.))
                            .flex_none()
                            .text_color(accent)
                            .child(format!("{n}.")),
                    )
                    .child(body)
                    .into_any_element(),
                BlockKind::Bullet => h_flex()
                    .items_start()
                    .child(div().w(px(18.)).flex_none().text_color(accent).child("•"))
                    .child(body)
                    .into_any_element(),
                BlockKind::Check(done) => h_flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .pt(px(3.))
                            .child(Checkbox::new(("plugin-md-check", ix)).size(px(14.)).toggle_state(done)),
                    )
                    .child(body.when(done, |el| el.line_through().text_color(muted)))
                    .into_any_element(),
                BlockKind::Quote => div()
                    .pl_3()
                    .border_l_2()
                    .border_color(accent)
                    .text_color(muted)
                    .child(body)
                    .into_any_element(),
                BlockKind::Divider => Divider::horizontal().into_any_element(),
                BlockKind::Image(_) => body.text_color(muted).into_any_element(),
                BlockKind::Paragraph => body.into_any_element(),
            }
        }))
        .into_any_element()
}

fn render_svg_node(
    raw_content: &str,
    width: f32,
    height: f32,
    cache: &mut HashMap<SvgCacheKey, Arc<RenderImage>>,
    cx: &mut App,
) -> AnyElement {
    let w = if width.is_finite() {
        width.clamp(1.0, 1024.0)
    } else {
        24.0
    };
    let h = if height.is_finite() {
        height.clamp(1.0, 1024.0)
    } else {
        24.0
    };

    if let Some(image) = rasterize_svg_cached(raw_content, w, h, cache, cx) {
        return gpui::img(image)
            .w(px(w))
            .h(px(h))
            .flex_none()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    }

    Icon::new(IconName::Image)
        .size(IconSize::Custom(px(w.min(h).min(32.0))))
        .color(Color::IconMuted)
        .into_any_element()
}

fn rasterize_svg_cached(
    raw_content: &str,
    width: f32,
    height: f32,
    cache: &mut HashMap<SvgCacheKey, Arc<RenderImage>>,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let sanitized = sanitize_svg(raw_content).ok()?;
    let ink = cx.theme().colors().icon.to_rgb();
    let r = (ink.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (ink.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (ink.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    let ink_rgba = u32::from_be_bytes([r, g, b, 0xFF]);

    let resolved_svg = if sanitized.to_ascii_lowercase().contains("currentcolor") {
        let hex = format!("#{r:02x}{g:02x}{b:02x}");
        replace_ascii_case_insensitive(&sanitized, "currentcolor", &hex)
    } else {
        sanitized
    };

    let px_w = (width * 2.0).round().clamp(2.0, 2048.0) as u32;
    let px_h = (height * 2.0).round().clamp(2.0, 2048.0) as u32;
    let key = SvgCacheKey {
        content_hash: fnv1a_64(resolved_svg.as_bytes()),
        width_px: px_w,
        height_px: px_h,
        ink_rgba,
    };

    if let Some(cached) = cache.get(&key) {
        return Some(Arc::clone(cached));
    }

    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_str(&resolved_svg, &opts).ok()?;
    let svg_size = tree.size();
    let sx = px_w as f32 / svg_size.width().max(1.0);
    let sy = px_h as f32 / svg_size.height().max(1.0);
    let scale = sx.min(sy);
    let dx = (px_w as f32 - svg_size.width() * scale) * 0.5;
    let dy = (px_h as f32 - svg_size.height() * scale) * 0.5;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(px_w, px_h)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let png = pixmap.encode_png().ok()?;
    let gpui_img = gpui::Image::from_bytes(gpui::ImageFormat::Png, png);
    let render_img = gpui_img.to_image_data(cx.svg_renderer()).ok()?;

    if cache.len() >= MAX_SVG_CACHE_ENTRIES {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&render_img));
    Some(render_img)
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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
