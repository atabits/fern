use std::rc::Rc;

use amin_core::Store;
use gpui::{Context, Entity, Window, div, prelude::*};

use crate::sidebar::{self, Section};
use crate::theme;
use crate::todo_view::TodoView;

pub struct AppView {
    section: Section,
    todo: Entity<TodoView>,
}

impl AppView {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let todo = cx.new(|cx| TodoView::new(Rc::new(store), window, cx));
        Self { section: Section::Tasks, todo }
    }

    fn select(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let on_select = move |section, _: &mut Window, cx: &mut gpui::App| {
            this.update(cx, |this, cx| this.select(section, cx)).ok();
        };

        div()
            .size_full()
            .flex()
            .bg(theme::background())
            .text_color(theme::text())
            .child(sidebar::render(self.section, on_select))
            .child(match self.section {
                Section::Tasks => div().flex_1().h_full().child(self.todo.clone()),
                other => div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_xl().child(other.title()))
                    .child(div().text_color(theme::text_muted()).child("Скоро")),
            })
    }
}
