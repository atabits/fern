//! The original's tag dialogs: Add Tags / Edit Tag (name, Color, Parent Tag) and Merge Tags.

use std::rc::Rc;

use fern_core::Store;
use gpui::{Entity, EventEmitter, Focusable, FontWeight, Subscription};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::i18n;
use crate::lists_panel::{LIST_COLORS, parse_color};
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

/// What the tag dialogs report to the view that opened them.
pub enum TagEvent {
    /// The tag was added, saved or merged; the name it has now.
    Saved(String),
    Closed,
}

/// The original's dialog frame: a title with ×, the body, and Close with a primary button, over
/// the window where the list dialogs are (see `list_dialog`).
fn frame(
    id: &'static str,
    title: &'static str,
    body: AnyElement,
    primary: &'static str,
    enabled: bool,
    on_close: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + Clone + 'static,
    on_primary: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    height: f32,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let button = |id: &'static str, label: &'static str, primary: bool| {
        div()
            .id(id)
            .w(px(100.))
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .when(primary, |el| {
                el.bg(theme.colors().accent.opacity(if enabled { 1. } else { 0.5 })).child(Label::new(label).color(Color::Custom(gpui::white())))
            })
            .when(!primary, |el| el.border_1().border_color(theme.colors().border).child(Label::new(label)))
    };
    div()
        .absolute()
        .inset_0()
        .pr(px(50.))
        .pb(px(86.))
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::black().opacity(0.35))
        .occlude()
        .child(
            v_flex()
                .id(id)
                .w(px(381.))
                .h(px(height))
                .px(px(20.))
                .rounded(px(10.))
                .bg(theme.colors().elevated_surface_background)
                .border_1()
                .border_color(theme.colors().border)
                .shadow_lg()
                .child(
                    h_flex()
                        .h(px(56.))
                        .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(IconButton::new(SharedString::from(format!("{id}-x")), IconName::X).icon_color(Color::Muted).on_click(on_close.clone())),
                )
                .child(body)
                .child(div().flex_1())
                .child(
                    h_flex()
                        .pb(px(18.))
                        .justify_end()
                        .gap(px(10.))
                        .child(button("tag-dialog-close", i18n::t(cx).close, false).on_click(on_close))
                        .child(button("tag-dialog-primary", primary, true).on_click(on_primary)),
                ),
        )
        .into_any_element()
}

/// A dropdown field as the original's dialogs have: the value and a chevron, its choices under it.
fn dropdown(id: &'static str, text: SharedString, open: bool, width: f32, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .w(px(width))
        .h(px(32.))
        .px_2p5()
        .rounded(px(6.))
        .border_1()
        .border_color(if open { theme.colors().accent } else { theme.colors().border })
        .cursor_pointer()
        .child(div().flex_1().min_w_0().child(Label::new(text).truncate()))
        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
}

fn choice(id: SharedString, label: SharedString, on: bool) -> ListItem {
    ListItem::new(id)
        .height(px(34.))
        .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
        .when(on, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
}

pub struct TagDialog {
    store: Rc<Store>,
    /// The tag being edited; `None` adds one.
    editing: Option<String>,
    name: Entity<TextInput>,
    color: Option<String>,
    parent: Option<String>,
    parent_open: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TagEvent> for TagDialog {}

impl TagDialog {
    /// Add Tags (under `parent` for the menu's Add Sub-tag), or Edit Tag for `editing`.
    pub fn new(store: Rc<Store>, editing: Option<String>, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let tag = editing.as_ref().and_then(|name| store.tags().ok()?.into_iter().find(|t| &t.name == name));
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.list_name, cx);
            if let Some(tag) = &tag {
                input.set_text(tag.name.clone(), cx);
            }
            input
        });
        let subscriptions = vec![
            cx.subscribe(&name, |this, _, _: &Submitted, cx| this.save(cx)),
            cx.subscribe(&name, |_, _, _: &Cancelled, cx| cx.emit(TagEvent::Closed)),
            cx.subscribe(&name, |this, _, _: &Changed, cx| {
                this.error = None;
                cx.notify();
            }),
        ];
        window.focus(&name.focus_handle(cx), cx);
        Self {
            color: tag.as_ref().and_then(|t| t.color.clone()),
            parent: tag.as_ref().map_or(parent, |t| t.parent.clone()),
            store,
            editing,
            name,
            parent_open: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().trim_start_matches('#').trim().to_owned();
        if name.is_empty() {
            return;
        }
        let result = match &self.editing {
            Some(old) => self.store.update_tag(old, &name, self.color.as_deref(), self.parent.as_deref()),
            None => self.store.add_tag(&name, self.color.as_deref(), self.parent.as_deref()),
        };
        match result {
            Ok(()) => cx.emit(TagEvent::Saved(name)),
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                cx.notify();
            }
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_open_parent(&mut self, cx: &mut Context<Self>) {
        self.parent_open = true;
        cx.notify();
    }
}

impl Render for TagDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        // Parents are top-level tags other than this one; a tag with sub-tags stays top-level.
        let tags = self.store.tags().unwrap_or_default();
        let editing = self.editing.clone();
        let has_children = editing.as_ref().is_some_and(|name| tags.iter().any(|t| t.parent.as_ref() == Some(name)));
        let parents: Vec<String> = tags
            .iter()
            .filter(|t| t.parent.is_none() && Some(&t.name) != editing.as_ref())
            .map(|t| t.name.clone())
            .collect();
        let current = self.parent.clone();
        let mut items: Vec<AnyElement> = vec![
            choice("tag-parent-none".into(), strings.none_option.into(), current.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    (this.parent, this.parent_open) = (None, false);
                    cx.notify();
                }))
                .into_any_element(),
        ];
        if !has_children {
            for name in parents {
                let on = current.as_ref() == Some(&name);
                let pick = name.clone();
                items.push(
                    choice(SharedString::from(format!("tag-parent-{name}")), name.into(), on)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            (this.parent, this.parent_open) = (Some(pick.clone()), false);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let parent_open = self.parent_open;
        let parent_field = div()
            .relative()
            .child(
                dropdown("tag-parent", current.clone().map_or(SharedString::new_static(strings.none_option), SharedString::from), parent_open, 259., cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.parent_open = !this.parent_open;
                        cx.notify();
                    })),
            )
            .when(parent_open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(260.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.parent_open = false;
                            cx.notify();
                        }))
                        .children(items),
                ))
            });
        let color = self.color.clone();
        let swatches: Vec<_> = std::iter::once(None)
            .chain(LIST_COLORS.iter().map(|c| Some(*c)))
            .map(|value| {
                let selected = color.as_deref() == value;
                div()
                    .id(SharedString::from(format!("tag-color-{}", value.unwrap_or("none"))))
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if selected { theme.colors().accent } else { gpui::transparent_black() })
                    .child(match parse_color(value) {
                        Some(fill) => div().size(px(16.)).rounded_full().bg(fill).into_any_element(),
                        None => Icon::new(IconName::Ban).size(IconSize::Small).color(Color::Muted).into_any_element(),
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.color = value.map(str::to_owned);
                        cx.notify();
                    }))
            })
            .collect();
        let row = |label: &'static str, content: AnyElement| {
            h_flex().h(px(38.)).child(div().w(px(80.)).flex_none().child(Label::new(label))).child(content)
        };
        let body = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .h(px(36.))
                    .px_2p5()
                    .gap_2()
                    .rounded(px(6.))
                    .bg(theme.colors().element_hover)
                    .child(Icon::new(IconName::Tag).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().child(self.name.clone())),
            )
            .child(row(strings.tag_color, h_flex().gap(px(3.)).children(swatches).into_any_element()))
            .child(row(strings.parent_tag, parent_field.into_any_element()))
            .children(self.error.clone().map(|e| Label::new(e).size(LabelSize::Small).color(Color::Error)))
            .into_any_element();
        let empty = self.name.read(cx).text().trim().is_empty();
        let view = cx.entity().downgrade();
        let close = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |_, cx| cx.emit(TagEvent::Closed)).ok();
        };
        let view = cx.entity().downgrade();
        let save = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| this.save(cx)).ok();
        };
        frame(
            "tag-dialog",
            if self.editing.is_some() { strings.edit_tag } else { strings.add_tags },
            body,
            strings.save,
            !empty,
            close,
            save,
            286.,
            cx,
        )
    }
}

/// The original's Merge Tags: the tag's tasks go to the chosen tag, and the tag is deleted.
pub struct MergeDialog {
    store: Rc<Store>,
    from: String,
    into: Option<String>,
    open: bool,
}

impl EventEmitter<TagEvent> for MergeDialog {}

impl MergeDialog {
    pub fn new(store: Rc<Store>, from: String) -> Self {
        Self { store, from, into: None, open: false }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(into) = self.into.clone() else { return };
        if self.store.merge_tag(&self.from, &into).is_ok() {
            cx.emit(TagEvent::Saved(into));
        }
    }

    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        cx.notify();
    }
}

impl Render for MergeDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let others: Vec<String> = self.store.tags().unwrap_or_default().into_iter().map(|t| t.name).filter(|n| *n != self.from).collect();
        let into = self.into.clone();
        let items: Vec<AnyElement> = others
            .into_iter()
            .map(|name| {
                let on = into.as_ref() == Some(&name);
                let pick = name.clone();
                choice(SharedString::from(format!("merge-into-{name}")), name.into(), on)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (this.into, this.open) = (Some(pick.clone()), false);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let open = self.open;
        let field = div()
            .relative()
            .child(
                dropdown("merge-into", into.clone().map_or(SharedString::new_static(strings.select_tag), SharedString::from), open, 220., cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open = !this.open;
                        cx.notify();
                    })),
            )
            .when(open, |el| {
                el.child(PopoverAttachment::new(
                    Placement::BelowLeft,
                    popover(cx)
                        .w(px(220.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.open = false;
                            cx.notify();
                        }))
                        .children(items),
                ))
            });
        let body = v_flex()
            .gap_3()
            .child(Label::new(strings.merge_tags_hint.replace("{tag}", &self.from)))
            .child(field)
            .into_any_element();
        let view = cx.entity().downgrade();
        let close = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |_, cx| cx.emit(TagEvent::Closed)).ok();
        };
        let view = cx.entity().downgrade();
        let confirm = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| this.confirm(cx)).ok();
        };
        frame("merge-dialog", strings.merge_tags, body, strings.confirm, into.is_some(), close, confirm, 262., cx)
    }
}
