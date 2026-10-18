use std::rc::Rc;

use amin_core::Store;
use gpui::{AnyElement, Context, Entity, Window, div, prelude::*, px, svg};

use crate::i18n::{self, CurrentLocale, Locale};
use crate::sidebar::{self, Section};
use crate::theme;
use crate::todo_view::TodoView;
use crate::widgets::{self, Placement};

pub struct AppView {
    store: Rc<Store>,
    section: Section,
    todo: Entity<TodoView>,
    account_menu_open: bool,
}

impl AppView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let todo = cx.new(|cx| TodoView::new(store.clone(), window, cx));
        Self { store, section: Section::Tasks, todo, account_menu_open: false }
    }

    fn select(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }

    fn set_account_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.account_menu_open = open;
        cx.notify();
    }

    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting("locale", locale.code()) {
            eprintln!("failed to save the language: {err:#}");
        }
        cx.set_global(CurrentLocale(locale));
        self.account_menu_open = false;
        cx.refresh_windows();
    }

    fn render_account_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = i18n::locale(cx);
        let strings = i18n::t(cx);
        widgets::popover_frame()
            .w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_account_menu(false, cx)))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme::text_muted())
                    .child(svg().path("icons/languages.svg").size(px(14.)).text_color(theme::text_muted()))
                    .child(strings.language),
            )
            .children(Locale::ALL.into_iter().map(|locale| {
                widgets::menu_item(locale.code())
                    .child(div().flex_1().child(locale.native_name()))
                    .when(locale == current, |el| el.child(div().text_color(theme::accent()).child("✓")))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_locale(locale, cx)))
            }))
            .into_any_element()
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let this = cx.entity().downgrade();
        let on_select = move |section, _: &mut Window, cx: &mut gpui::App| {
            this.update(cx, |this, cx| this.select(section, cx)).ok();
        };
        let on_avatar = cx.listener(|this, _, _, cx| this.set_account_menu(true, cx));
        let account_menu = self
            .account_menu_open
            .then(|| widgets::popover(Placement::RightTop, self.render_account_menu(cx)).into_any_element());

        div()
            .size_full()
            .flex()
            .bg(theme::background())
            .text_color(theme::text())
            .child(sidebar::render(self.section, on_select, on_avatar, account_menu))
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
                    .child(div().text_xl().child(other.title(strings)))
                    .child(div().text_color(theme::text_muted()).child(strings.coming_soon)),
            })
    }
}
