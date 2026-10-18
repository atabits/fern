use gpui::{App, ClickEvent, Window};

pub trait Clickable {
    fn on_click(self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self;
}

pub trait Toggleable {
    /// Marks the element as selected (or checked).
    fn toggle_state(self, selected: bool) -> Self;
}

pub(crate) type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
