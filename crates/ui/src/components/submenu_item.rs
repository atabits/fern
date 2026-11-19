use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _,
};

use crate::{Color, Icon, IconName, IconSize, Label, ListItem, Placement, PopoverAttachment, Toggleable, popover};

type HoverHandler = Box<dyn Fn(&bool, &mut Window, &mut App)>;

/// A menu item with a chevron that opens a side menu while hovered, as menus do in the
/// original. The owner keeps which side menu is open and passes it as `open`; hovering another
/// item of the menu should close it.
#[derive(IntoElement)]
pub struct SubmenuItem {
    id: ElementId,
    icon: Option<IconName>,
    label: SharedString,
    open: bool,
    placement: Placement,
    on_hover: Option<HoverHandler>,
    items: Vec<AnyElement>,
    height: Option<gpui::Pixels>,
    bounds: Option<Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>>,
}

impl SubmenuItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon: None,
            label: label.into(),
            open: false,
            placement: Placement::LeftSide,
            on_hover: None,
            items: Vec::new(),
            height: None,
            bounds: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// `LeftSide` (the default, for menus at the right of the window) or `RightSide`.
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
    }

    /// The side menu's entries, usually `ListItem`s.
    /// The row's height, as `ListItem::height` sets it.
    pub fn height(mut self, height: gpui::Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Lets the parent menu include this deferred surface in its outside-click check.
    pub fn track_bounds(mut self, bounds: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>) -> Self {
        self.bounds = Some(bounds);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.items.extend(items.into_iter().map(IntoElement::into_any_element));
        self
    }
}

impl RenderOnce for SubmenuItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let debug_selector = format!("submenu-{}", self.id);
        let item = ListItem::new(self.id.clone())
            .toggle_state(self.open)
            .when_some(self.icon, |item, icon| item.start_slot(Icon::new(icon).color(Color::Icon)))
            .child(Label::new(self.label))
            .end_slot(Icon::new(IconName::ChevronRight).size(IconSize::Small).color(Color::Muted))
            .when_some(self.height, |item, height| item.height(height));
        let menu_selector = format!("submenu-content-{}", self.id);
        let menu = self.open.then(|| PopoverAttachment::new(self.placement, popover(cx)
            .debug_selector(move || menu_selector)
            .relative()
            .min_w(gpui::px(180.))
            .children(self.items)
            .when_some(self.bounds, |menu, bounds| menu.child(
                gpui::canvas(move |area, _, _| bounds.set(Some(area)), |_, _, _, _| {})
                    .absolute().inset_0(),
            ))));
        div()
            .id(self.id)
            .debug_selector(move || debug_selector)
            .relative()
            .when_some(self.on_hover, |el, handler| el.on_hover(handler))
            .child(item)
            .children(menu)
    }
}
