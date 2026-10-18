use std::rc::Rc;

use fern_core::Store;
use gpui::Focusable as _;
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions::{NewTask, Search, ShowInbox, ShowNext7Days, ShowToday, ShowTomorrow, ToggleSidebar};
use crate::calendar_view::{self, CalendarView};
use crate::i18n::{self, CurrentLocale, Locale};
use crate::search_modal::{SearchEvent, SearchModal};
use crate::sidebar::{self, Section};
use crate::todo_view::TodoView;

pub struct AppView {
    store: Rc<Store>,
    section: Section,
    todo: gpui::Entity<TodoView>,
    calendar: gpui::Entity<CalendarView>,
    search: Option<gpui::Entity<SearchModal>>,
    account_menu_open: bool,
}

impl AppView {
    pub fn new(store: Rc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let todo = cx.new(|cx| TodoView::new(store.clone(), window, cx));
        let calendar = cx.new(|cx| CalendarView::new(store.clone(), cx));
        // A task clicked in the calendar opens in the task list with its details.
        cx.subscribe_in(&calendar, window, |this, _, calendar_view::OpenTask(id), window, cx| {
            let id = *id;
            this.select(Section::Tasks, window, cx);
            this.todo.update(cx, |todo, cx| {
                todo.set_view(fern_core::View::All, cx);
                todo.select(id, cx);
            });
        })
        .detach();
        let mut this = Self { store, section: Section::Tasks, todo, calendar, search: None, account_menu_open: false };
        #[cfg(debug_assertions)]
        if let Ok(state) = std::env::var("FERN_DEV_STATE") {
            if state.contains("section=calendar") {
                this.select(Section::Calendar, window, cx);
            }
            if let Some(query) = state.split(';').find_map(|part| part.strip_prefix("search-popover=")) {
                this.open_search(window, cx);
                if let Some(search) = &this.search {
                    search.update(cx, |search, cx| search.set_query(query, cx));
                }
            }
        }
        this
    }

    fn select(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        // Search is a popover over the current screen, as in the original.
        if section == Section::Search {
            self.open_search(window, cx);
            return;
        }
        self.todo.update(cx, |todo, cx| todo.stop_search(cx));
        if section == Section::Calendar {
            self.calendar.update(cx, |calendar, cx| calendar.reload(cx));
        }
        self.section = section;
        cx.notify();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| SearchModal::new(self.store.clone(), cx));
        cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            this.search = None;
            match event {
                SearchEvent::OpenTask(id) => {
                    let id = *id;
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| {
                        todo.set_view(fern_core::View::All, cx);
                        todo.select(id, cx);
                    });
                }
                SearchEvent::OpenView(fern_core::View::Search(query)) => {
                    let query = query.clone();
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| todo.show_search_results(query, cx));
                }
                SearchEvent::OpenView(view) => {
                    let view = view.clone();
                    this.select(Section::Tasks, window, cx);
                    this.todo.update(cx, |todo, cx| todo.set_view(view, cx));
                }
                SearchEvent::Dismissed => {}
            }
            cx.notify();
        })
        .detach();
        window.focus(&search.focus_handle(cx), cx);
        self.search = Some(search);
        cx.notify();
    }

    fn new_task(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| todo.focus_new_task(window, cx));
    }

    fn search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Search, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.todo.update(cx, |todo, cx| todo.toggle_lists(cx));
    }

    fn show_view(&mut self, view: fern_core::View, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Section::Tasks, window, cx);
        self.todo.update(cx, |todo, cx| todo.set_view(view, cx));
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
        popover(cx)
            .w(px(200.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_account_menu(false, cx)))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .child(Icon::new(IconName::Languages).size(IconSize::Small))
                    .child(Label::new(strings.language).size(LabelSize::Small).color(Color::Muted)),
            )
            .children(Locale::ALL.into_iter().map(|locale| {
                ListItem::new(locale.code())
                    .child(Label::new(locale.native_name()))
                    .when(locale == current, |item| item.end_slot(Label::new("✓").color(Color::Accent)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_locale(locale, cx)))
            }))
            .into_any_element()
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let this = cx.entity().downgrade();
        let on_select = move |section, window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.select(section, window, cx)).ok();
        };
        let on_avatar = cx.listener(|this, _, _, cx| this.set_account_menu(true, cx));
        let account_menu = self.account_menu_open.then(|| {
            PopoverAttachment::new(Placement::RightTop, self.render_account_menu(cx)).into_any_element()
        });

        h_flex()
            .relative()
            .key_context("Workspace")
            .on_action(cx.listener(Self::new_task))
            .on_action(cx.listener(Self::search))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(|this, _: &ShowInbox, window, cx| this.show_view(fern_core::View::Inbox, window, cx)))
            .on_action(cx.listener(|this, _: &ShowToday, window, cx| this.show_view(fern_core::View::Today, window, cx)))
            .on_action(cx.listener(|this, _: &ShowTomorrow, window, cx| {
                this.show_view(fern_core::View::Tomorrow, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowNext7Days, window, cx| {
                this.show_view(fern_core::View::Next7Days, window, cx)
            }))
            .size_full()
            .items_start()
            .bg(theme.colors().background)
            .text_color(theme.colors().text)
            .child(sidebar::render(self.section, on_select, on_avatar, account_menu, cx))
            .child(match self.section {
                Section::Tasks | Section::Search => div().flex_1().h_full().child(self.todo.clone()),
                Section::Calendar => div().flex_1().h_full().child(self.calendar.clone()),
                other => v_flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Label::new(other.title(strings)).size(LabelSize::Large))
                    .child(Label::new(strings.coming_soon).color(Color::Muted)),
            })
            .when_some(self.search.clone(), |el, search| {
                el.child(div().absolute().top(px(160.)).left_0().right_0().flex().justify_center().child(search))
            })
    }
}
