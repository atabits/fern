use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use desktop_common::text_input::Mode;
use fern_core::markdown::{BlockKind, Style as MdStyle, blocks as parse_markdown_blocks};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, FontStyle, FontWeight, HighlightStyle, Hsla, InteractiveElement,
    InteractiveText, IntoElement, ObjectFit, ParentElement, RenderImage, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Styled, StyledText, UnderlineStyle, div, px,
    relative,
};
use icons::IconName;
use plugins::ui_ast::{
    UiAlign, UiButtonSize, UiButtonVariant, UiEvent, UiJustify, UiNode, UiTextSize, UiWeight,
};
use theme::ActiveTheme as _;
use ui::prelude::*;
use ui::{
    Button, ButtonSize, ButtonVariant, Checkbox, Chip, Color, Divider, Donut, EmptyState, Icon,
    IconButton, IconSize, Label, LabelSize, ListItem, Placement, PopoverAttachment, SegmentedTab,
    SegmentedTabs, Switch, Tooltip, popover, v_scroll,
};

use super::{LayoutCtx, UiEventSink, UiNotifySink, UiTreeState, rasterize_svg};

const MAX_SVG_CACHE_ENTRIES: usize = 128;
const MAX_SVG_CACHE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SvgCacheKey {
    content_hash: u64,
    content_len: usize,
    width_px: u32,
    height_px: u32,
    ink_rgba: u32,
}

#[derive(Default)]
pub(super) struct SvgCache {
    entries: HashMap<SvgCacheKey, Option<Arc<RenderImage>>>,
    total_bytes: usize,
}

impl SvgCache {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }
}

impl UiTreeState {
    pub(super) fn render_node(
        &mut self,
        node: &UiNode,
        view_id: &str,
        layout_ctx: LayoutCtx,
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
                    .map(|el| apply_block_width(el, layout_ctx))
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
                    .map(|c| self.render_node(c, view_id, LayoutCtx::Column, sink, notify_sink, cx))
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
                let (el_base, child_ctx) = match layout_ctx {
                    LayoutCtx::Column => (h_flex().min_w_0().w_full(), LayoutCtx::RowConstrained),
                    LayoutCtx::RowConstrained if *wrap => {
                        (h_flex().min_w_0().flex_1(), LayoutCtx::RowConstrained)
                    }
                    LayoutCtx::RowConstrained | LayoutCtx::RowAuto => {
                        (h_flex().min_w_0().flex_none(), LayoutCtx::RowAuto)
                    }
                };
                let mut el = el_base
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
                    .map(|c| self.render_node(c, view_id, child_ctx, sink, notify_sink, cx))
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
                    .map(|c| self.render_node(c, view_id, LayoutCtx::Column, sink, notify_sink, cx))
                    .collect();
                div()
                    .min_w_0()
                    .map(|el| apply_block_width(el, layout_ctx))
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
                    inner_items.push(self.render_node(
                        c,
                        view_id,
                        LayoutCtx::Column,
                        sink,
                        notify_sink,
                        cx,
                    ));
                }
                for c in children {
                    inner_items.push(self.render_node(
                        c,
                        view_id,
                        LayoutCtx::Column,
                        sink,
                        notify_sink,
                        cx,
                    ));
                }
                if let Some(mh) = max_height {
                    let viewport = v_flex()
                        .id(viewport_id)
                        .w_full()
                        .gap_2()
                        .when(cfg!(test), |el| {
                            let sel = format!("plugin-scroll-{key}");
                            el.debug_selector(move || sel)
                        })
                        .children(inner_items);
                    let mut scroll = v_scroll(scroll_id, viewport)
                        .track_scroll(&handle)
                        .min_w_0()
                        .max_h(px(mh.max(24.0)));
                    scroll = match layout_ctx {
                        LayoutCtx::Column => scroll.w_full(),
                        LayoutCtx::RowConstrained => scroll.flex_1(),
                        LayoutCtx::RowAuto => scroll,
                    };
                    scroll.into_any_element()
                } else {
                    // Inside auto-height containers (like PluginView/PluginOverlayView's outer
                    // v_scroll body), `.flex_1().min_h_0()` collapses to 0px in Taffy. Render
                    // at natural content height so the outer view scrolls cleanly.
                    let mut body = v_flex()
                        .id(scroll_id)
                        .min_w_0()
                        .gap_2()
                        .children(inner_items);
                    body = match layout_ctx {
                        LayoutCtx::Column => body.w_full(),
                        LayoutCtx::RowConstrained => body.flex_1(),
                        LayoutCtx::RowAuto => body,
                    };
                    #[cfg(test)]
                    let body = {
                        let sel = format!("plugin-scroll-{key}");
                        body.debug_selector(move || sel)
                    };
                    body.into_any_element()
                }
            }
            UiNode::Card {
                title,
                subtitle,
                padding,
                children,
            } => {
                let card_bg = cx.theme().colors().surface_background;
                let card_border = cx.theme().colors().border_variant;
                #[cfg(test)]
                let card_idx = {
                    let idx = self.test_card_counter;
                    self.test_card_counter += 1;
                    idx
                };
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, LayoutCtx::Column, sink, notify_sink, cx))
                    .collect();
                let card = v_flex()
                    .map(|el| apply_block_width(el, layout_ctx))
                    .min_w_0()
                    .rounded(px(8.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(card_border)
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
                    .children(rendered);
                #[cfg(test)]
                let card = card.debug_selector(move || format!("plugin-card-{card_idx}"));
                card.into_any_element()
            }
            UiNode::Section {
                title,
                subtitle,
                children,
            } => {
                let rendered: Vec<AnyElement> = children
                    .iter()
                    .map(|c| self.render_node(c, view_id, LayoutCtx::Column, sink, notify_sink, cx))
                    .collect();
                v_flex()
                    .map(|el| apply_block_width(el, layout_ctx))
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
                    .when(layout_ctx != LayoutCtx::Column, |el| el.flex_shrink(1.0))
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
                #[cfg(test)]
                let text_idx = {
                    let idx = self.test_text_counter;
                    self.test_text_counter += 1;
                    idx
                };
                let el = div()
                    .min_w_0()
                    .when(layout_ctx != LayoutCtx::Column, |el| el.flex_shrink(1.0))
                    .text_size(px(font_px))
                    .font_weight(font_weight)
                    .text_color(resolved_color)
                    .child(text.clone());
                #[cfg(test)]
                let el = el.debug_selector(move || format!("plugin-text-{text_idx}"));
                el.into_any_element()
            }
            UiNode::Markdown { content } => {
                let md_idx = self.node_counter;
                self.node_counter += 1;
                render_markdown_node(content, md_idx, layout_ctx, cx)
            }
            UiNode::KeyValue {
                label,
                value,
                muted,
            } => h_flex()
                .map(|el| apply_block_width(el, layout_ctx))
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
                    .map(|el| apply_block_width(el, layout_ctx))
                    .min_w_0()
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
                let input_bg = cx.theme().colors().background;
                let input_border = cx.theme().colors().border;
                div()
                    .map(|el| apply_input_width(el, layout_ctx))
                    .h(px(30.))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .rounded(px(6.))
                    .bg(input_bg)
                    .border_1()
                    .border_color(input_border)
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
                let area_bg = cx.theme().colors().background;
                let area_border = cx.theme().colors().border;
                let height_px = (f32::from((*rows).clamp(2, 24)) * 20.0 + 12.0).clamp(52.0, 480.0);
                div()
                    .map(|el| apply_input_width(el, layout_ctx))
                    .h(px(height_px))
                    .p_2()
                    .overflow_hidden()
                    .rounded(px(6.))
                    .bg(area_bg)
                    .border_1()
                    .border_color(area_border)
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
                    let open_ref_out = Rc::clone(&open_ref);
                    let notify_out = Rc::clone(notify_sink);
                    let scroll_id = ElementId::Name(SharedString::from(format!(
                        "plugin-select-scroll-{key}"
                    )));
                    let vp_id = ElementId::Name(SharedString::from(format!(
                        "plugin-select-vp-{key}"
                    )));
                    Some(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .min_w(px(160.))
                            .max_w(px(280.))
                            .on_mouse_down_out(move |_, _, cx| {
                                *open_ref_out.borrow_mut() = None;
                                notify_out(cx);
                                cx.stop_propagation();
                            })
                            .child(
                                v_scroll(scroll_id, v_flex().id(vp_id).w_full().children(items))
                                    .compact()
                                    .max_h(px(260.))
                                    .w_full(),
                            ),
                    ))
                } else {
                    None
                };

                let element_hover = cx.theme().colors().element_hover;
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
                            .bg(cx.theme().colors().background)
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .when(*disabled, |el| el.opacity(0.5).cursor_default())
                            .when(!*disabled, |el| {
                                el.cursor_pointer()
                                    .hover(move |s| s.bg(element_hover))
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
                let seg_idx = self.node_counter;
                self.node_counter += 1;
                let prefix = match key.as_deref() {
                    Some(k) => format!("k-{k}"),
                    None => format!("seg-{seg_idx}"),
                };
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

fn apply_block_width<E: Styled>(el: E, layout_ctx: LayoutCtx) -> E {
    match layout_ctx {
        LayoutCtx::Column => el.w_full(),
        LayoutCtx::RowConstrained => el.flex_1(),
        LayoutCtx::RowAuto => el,
    }
}

fn apply_input_width<E: Styled>(el: E, layout_ctx: LayoutCtx) -> E {
    match layout_ctx {
        LayoutCtx::Column => el.w_full().min_w_0(),
        LayoutCtx::RowConstrained => el.flex_1().min_w_0(),
        LayoutCtx::RowAuto => el.min_w(px(140.)),
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
fn resolve_ui_color(raw: &str, cx: &App) -> Option<Hsla> {
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
fn resolve_icon_name(raw: Option<&str>) -> IconName {
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

fn render_markdown_node(
    content: &str,
    md_idx: usize,
    layout_ctx: LayoutCtx,
    cx: &App,
) -> AnyElement {
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
        .map(|el| apply_block_width(el, layout_ctx))
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
            let block_id = ElementId::Name(SharedString::from(format!(
                "plugin-md-block-{md_idx}-{ix}"
            )));
            let body_id = ElementId::Name(SharedString::from(format!(
                "plugin-md-body-{md_idx}-{ix}"
            )));
            let check_id = ElementId::Name(SharedString::from(format!(
                "plugin-md-check-{md_idx}-{ix}"
            )));
            let interactive = InteractiveText::new(block_id, styled_text)
                .on_click(ranges, |_, _, _| {});
            let body = div()
                .id(body_id)
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
                            .child(Checkbox::new(check_id).size(px(14.)).toggle_state(done)),
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
    cache: &mut SvgCache,
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
    cache: &mut SvgCache,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let ink = cx.theme().colors().icon.to_rgb();
    let r = (ink.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (ink.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (ink.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    let ink_rgba = u32::from_be_bytes([r, g, b, 0xFF]);

    let px_w = (width * 2.0).round().clamp(2.0, 2048.0) as u32;
    let px_h = (height * 2.0).round().clamp(2.0, 2048.0) as u32;
    let key = SvgCacheKey {
        content_hash: fnv1a_64(raw_content.as_bytes()),
        content_len: raw_content.len(),
        width_px: px_w,
        height_px: px_h,
        ink_rgba,
    };

    if let Some(cached) = cache.entries.get(&key) {
        return cached.clone();
    }

    let rendered = rasterize_svg(raw_content, px_w, px_h, (r, g, b), cx);

    let entry_bytes = if rendered.is_some() {
        (px_w as usize)
            .saturating_mul(px_h as usize)
            .saturating_mul(4)
    } else {
        0
    };
    if cache.entries.len() >= MAX_SVG_CACHE_ENTRIES
        || cache.total_bytes.saturating_add(entry_bytes) > MAX_SVG_CACHE_BYTES
    {
        cache.clear();
    }
    cache.total_bytes = cache.total_bytes.saturating_add(entry_bytes);
    cache.entries.insert(key, rendered.clone());
    rendered
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
