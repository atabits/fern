//! The original's Add List / Edit List dialog: the name with an emoji, List Color, View Type,
//! Folder (with New Folder), List Type and "Show in Smart List", and on the right a sketch of how
//! the list will look. Also the New Folder / Edit Folder dialog it opens, which the lists column
//! uses too.

use std::rc::Rc;

use fern_core::{Id, Kind, ListDraft, ListFolder, Store};
use gpui::{Entity, EventEmitter, Focusable, FontWeight, Subscription};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::i18n::{self, Strings};
use crate::lists_panel::{LIST_COLORS, parse_color};
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};

/// How the list is shown: the original's View Type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    List,
    Kanban,
    Timeline,
}

impl Layout {
    /// The layout a list was given, from the settings the list header's View switch keeps.
    pub fn of(store: &Store, list: Id) -> Self {
        let on = |name: &str| store.setting(&format!("{name}:list:{list}")).ok().flatten().is_some_and(|v| v == "1");
        if on("kanban") {
            Layout::Kanban
        } else if on("timeline") {
            Layout::Timeline
        } else {
            Layout::List
        }
    }

    fn save(self, store: &Store, list: Id) -> anyhow::Result<()> {
        store.set_setting(&format!("kanban:list:{list}"), if self == Layout::Kanban { "1" } else { "0" })?;
        store.set_setting(&format!("timeline:list:{list}"), if self == Layout::Timeline { "1" } else { "0" })
    }
}

/// The dialog's dropdowns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Icon,
    Folder,
    Type,
    Smart,
}

/// What the dialogs report to the view that opened them.
pub enum DialogEvent {
    /// The list was added or saved.
    Saved(Id),
    /// The folder was added or renamed.
    FolderSaved(Id),
    Closed,
}

pub struct ListDialog {
    store: Rc<Store>,
    /// The list being edited; `None` adds one.
    editing: Option<Id>,
    draft: ListDraft,
    layout: Layout,
    name: Entity<TextInput>,
    folders: Vec<ListFolder>,
    open: Option<Menu>,
    emoji_search: Entity<TextInput>,
    emoji_tab: usize,
    /// The New Folder dialog over this one.
    new_folder: Option<Entity<FolderDialog>>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for ListDialog {}

impl ListDialog {
    /// Opens the dialog for a new list (in `folder` when given), or for the list `editing`.
    pub fn new(store: Rc<Store>, editing: Option<Id>, folder: Option<Id>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let list = editing.and_then(|id| store.lists().ok()?.into_iter().find(|l| l.id == id));
        let draft = list.as_ref().map_or_else(|| ListDraft { folder, ..ListDraft::default() }, ListDraft::of);
        let layout = editing.map_or(Layout::List, |id| Layout::of(&store, id));
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.list_name, cx);
            input.set_text(draft.name.clone(), cx);
            input
        });
        let emoji_search = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.emoji_search, cx));
        let subscriptions = vec![
            cx.subscribe(&name, |this, _, _: &Submitted, cx| this.save(cx)),
            cx.subscribe(&name, |_, _, _: &Cancelled, cx| cx.emit(DialogEvent::Closed)),
            cx.subscribe(&name, |this, _, _: &Changed, cx| {
                this.error = None;
                cx.notify();
            }),
            cx.subscribe(&emoji_search, |_, _, _: &Changed, cx| cx.notify()),
            cx.subscribe(&emoji_search, |this, _, _: &Cancelled, cx| {
                this.open = None;
                cx.notify();
            }),
        ];
        window.focus(&name.focus_handle(cx), cx);
        Self {
            folders: store.list_folders().unwrap_or_default(),
            store,
            editing,
            draft,
            layout,
            name,
            open: None,
            emoji_search,
            emoji_tab: 0,
            new_folder: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.draft.name = self.name.read(cx).text().trim().to_owned();
        if self.draft.name.is_empty() {
            return;
        }
        let result = match self.editing {
            Some(id) => self.store.update_list(id, &self.draft).map(|_| id),
            None => self.store.add_list(&self.draft).map(|list| list.id),
        }
        .and_then(|id| self.layout.save(&self.store, id).map(|_| id));
        match result {
            Ok(id) => cx.emit(DialogEvent::Saved(id)),
            Err(err) => {
                self.error = Some(format!("{err:#}"));
                cx.notify();
            }
        }
    }

    fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Self)) {
        change(self);
        cx.notify();
    }

    fn toggle(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.open = if self.open == Some(menu) { None } else { Some(menu) };
        if self.open == Some(Menu::Icon) {
            self.emoji_search.update(cx, |input, cx| input.set_text("", cx));
            window.focus(&self.emoji_search.focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn start_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = None;
        let dialog = cx.new(|cx| FolderDialog::new(self.store.clone(), None, window, cx));
        cx.subscribe(&dialog, |this, _, event: &DialogEvent, cx| {
            if let DialogEvent::FolderSaved(id) = event {
                this.folders = this.store.list_folders().unwrap_or_default();
                this.draft.folder = Some(*id);
            }
            this.new_folder = None;
            cx.notify();
        })
        .detach();
        self.new_folder = Some(dialog);
        cx.notify();
    }

    /// For the dev state: opens one of the dropdowns.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            "icon" => self.toggle(Menu::Icon, window, cx),
            "folder" => self.toggle(Menu::Folder, window, cx),
            "type" => self.toggle(Menu::Type, window, cx),
            "smart" => self.toggle(Menu::Smart, window, cx),
            "new-folder" => self.start_new_folder(window, cx),
            "kanban" => self.layout = Layout::Kanban,
            "timeline" => self.layout = Layout::Timeline,
            _ => {}
        }
    }

    fn render_icon_picker(&self, strings: &'static Strings, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let query = self.emoji_search.read(cx).text().to_owned();
        let (title, emoji): (&str, Vec<&'static str>) = if query.trim().is_empty() {
            let (_, items) = crate::emoji::CATEGORIES[self.emoji_tab];
            (strings.emoji_categories[self.emoji_tab], items.iter().map(|(e, _)| *e).collect())
        } else {
            (strings.emoji_search, crate::emoji::search(&query))
        };
        let tab = |i: usize, glyph: &'static str, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(format!("emoji-tab-{i}")))
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .when(i == self.emoji_tab && query.trim().is_empty(), |el| el.bg(theme.colors().element_selected))
                .hover(|s| s.bg(theme.colors().element_hover))
                .child(div().text_size(px(13.)).child(glyph))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.emoji_tab = i;
                    this.emoji_search.update(cx, |input, cx| input.set_text("", cx));
                    cx.notify();
                }))
        };
        let text_button = |id: &'static str, label: &'static str| {
            div().id(id).px_1().cursor_pointer().child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
        };
        let tabs: Vec<_> = crate::emoji::CATEGORIES.iter().enumerate().map(|(i, (glyph, _))| tab(i, glyph, cx)).collect();
        popover(cx)
            .w(px(342.))
            .p_3()
            .gap_2()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update(cx, |this| this.open = None)))
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .rounded(px(6.))
                    .bg(theme.colors().element_hover)
                    .child(Icon::new(IconName::Search).size(IconSize::Small).color(Color::Muted))
                    .child(div().flex_1().child(self.emoji_search.clone())),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .children(tabs)
                    .child(div().mx_1().w(px(1.)).h(px(14.)).bg(theme.colors().border))
                    .child(div().flex_1())
                    .child(text_button("emoji-random", strings.emoji_random).on_click(cx.listener(|this, _, _, cx| {
                        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
                        this.update(cx, |this| this.draft.icon = Some(crate::emoji::pick(seed / 1000).to_owned()))
                    })))
                    .child(text_button("emoji-reset", strings.emoji_reset).on_click(cx.listener(|this, _, _, cx| {
                        this.update(cx, |this| this.draft.icon = None)
                    }))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                    .child(Label::new(title).size(LabelSize::Small)),
            )
            .child(
                div().id("emoji-grid").h(px(180.)).overflow_y_scroll().child(h_flex().flex_wrap().children(emoji.into_iter().map(|e| {
                    div()
                        .id(SharedString::from(format!("emoji-{e}")))
                        .w(px(39.))
                        .h(px(34.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.colors().element_hover))
                        .child(div().text_size(px(20.)).child(e))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update(cx, |this| {
                                this.draft.icon = Some(e.to_owned());
                                this.open = None;
                            })
                        }))
                }))),
            )
            .into_any_element()
    }

    /// The sketch on the right, as the original's: the list, its kanban columns or its timeline
    /// bars in grey, with the name on top and the chosen color on the first items.
    fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let line = theme.colors().text_muted.opacity(0.18);
        let accent = parse_color(self.draft.color.as_deref()).unwrap_or(theme.colors().accent);
        let name = self.name.read(cx).text().trim().to_owned();
        let name = if name.is_empty() { i18n::t(cx).list_name.to_owned() } else { name };
        let bar = |w: f32| div().w(px(w)).h(px(6.)).rounded(px(2.)).bg(line);
        let row = |w: f32, marked: bool| {
            h_flex()
                .gap_1p5()
                .child(div().size(px(7.)).rounded(px(1.)).border_1().border_color(if marked { accent } else { theme.colors().text_muted.opacity(0.5) }))
                .child(bar(w))
        };
        let body = match self.layout {
            Layout::List => v_flex()
                .gap_2()
                .child(div().w(px(212.)).h(px(12.)).rounded(px(2.)).bg(line))
                .child(bar(34.))
                .children([(80., true), (48., true), (115., true), (68., false), (95., false)].map(|(w, m)| row(w, m)))
                .child(div().h_2())
                .child(bar(34.))
                .children([(115., true), (165., false)].map(|(w, m)| row(w, m)))
                .into_any_element(),
            Layout::Kanban => h_flex()
                .items_start()
                .gap_2()
                .children([5usize, 2, 3].into_iter().enumerate().map(|(column, cards)| {
                    let card_bg = theme.colors().text_muted.opacity(0.08);
                    v_flex().w(px(94.)).gap_1().child(bar(28.)).children((0..cards).map(move |i| {
                        v_flex()
                            .p_1p5()
                            .gap_1()
                            .rounded(px(3.))
                            .bg(card_bg)
                            .child(row(56., column == 0 && i < 3 || column == 1 && i == 0))
                            .child(div().pl(px(12.)).child(bar(20.)))
                    }))
                }))
                .into_any_element(),
            Layout::Timeline => v_flex()
                .gap_2()
                .child(h_flex().gap_3().children((0..7).map(|_| bar(18.))))
                .children([(0., 90., true), (40., 70., true), (70., 120., false), (20., 60., true), (110., 80., false)].map(|(x, w, m)| {
                    div().pl(px(x)).child(div().w(px(w)).h(px(14.)).rounded(px(3.)).bg(if m { accent.opacity(0.5) } else { line }))
                }))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .pt(px(70.))
            .pl(px(48.))
            .gap_3()
            .bg(theme.colors().background)
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(IconName::Menu).size(IconSize::XSmall).color(Color::Muted))
                    .child(div().text_size(px(9.)).font_weight(FontWeight::MEDIUM).child(name)),
            )
            .child(body)
            .into_any_element()
    }
}

impl Render for ListDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let open = self.open;
        let picker = (open == Some(Menu::Icon)).then(|| self.render_icon_picker(strings, cx));
        let check = |on: bool| on.then(|| Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent));
        let choice = |id: SharedString, label: SharedString, on: bool| {
            ListItem::new(id)
                .height(px(34.))
                .child(Label::new(label).color(if on { Color::Accent } else { Color::Default }))
                .when_some(check(on), |item, icon| item.end_slot(icon))
        };
        let dropdown = |which: Menu, text: SharedString, items: Vec<AnyElement>, cx: &mut Context<Self>| -> AnyElement {
            let on = open == Some(which);
            div()
                .relative()
                .w(px(213.))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("list-dialog-{which:?}")))
                        .h(px(32.))
                        .px_2p5()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if on { theme.colors().accent } else { theme.colors().border })
                        .cursor_pointer()
                        .child(div().flex_1().min_w_0().child(Label::new(text).truncate()))
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(Color::Muted))
                        .on_click(cx.listener(move |this, _, window, cx| this.toggle(which, window, cx))),
                )
                .when(on, |el| {
                    el.child(PopoverAttachment::new(
                        Placement::BelowLeft,
                        popover(cx)
                            .w(px(213.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.update(cx, |this| this.open = None)))
                            .children(items),
                    ))
                })
                .into_any_element()
        };

        let folder = self.draft.folder;
        let folder_text: SharedString = folder
            .and_then(|id| self.folders.iter().find(|f| f.id == id))
            .map_or(SharedString::new_static(strings.none_option), |f| f.name.clone().into());
        let mut folder_items: Vec<AnyElement> = vec![
            choice("list-folder-none".into(), strings.none_option.into(), folder.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.update(cx, |this| (this.draft.folder, this.open) = (None, None))))
                .into_any_element(),
        ];
        for f in &self.folders {
            let id = f.id;
            folder_items.push(
                choice(SharedString::from(format!("list-folder-{id}")), f.name.clone().into(), folder == Some(id))
                    .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |this| (this.draft.folder, this.open) = (Some(id), None))))
                    .into_any_element(),
            );
        }
        folder_items.push(
            ListItem::new("list-folder-new")
                .height(px(34.))
                .start_slot(Icon::new(IconName::Plus))
                .child(Label::new(strings.new_folder))
                .on_click(cx.listener(|this, _, window, cx| this.start_new_folder(window, cx)))
                .into_any_element(),
        );
        let kind = self.draft.kind;
        let type_items: Vec<AnyElement> = [(Kind::Task, strings.task_list), (Kind::Note, strings.note_list)]
            .into_iter()
            .map(|(k, label)| {
                choice(SharedString::from(format!("list-type-{}", k.as_str())), label.into(), kind == k)
                    .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |this| (this.draft.kind, this.open) = (k, None))))
                    .into_any_element()
            })
            .collect();
        let smart = self.draft.in_smart_lists;
        let smart_items: Vec<AnyElement> = [(true, strings.smart_all_tasks), (false, strings.smart_do_not_show)]
            .into_iter()
            .map(|(on, label)| {
                choice(SharedString::from(format!("list-smart-{on}")), label.into(), smart == on)
                    .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |this| (this.draft.in_smart_lists, this.open) = (on, None))))
                    .into_any_element()
            })
            .collect();
        let folder_menu = dropdown(Menu::Folder, folder_text, folder_items, cx);
        let type_menu = dropdown(Menu::Type, (if kind == Kind::Note { strings.note_list } else { strings.task_list }).into(), type_items, cx);
        let smart_menu = dropdown(Menu::Smart, (if smart { strings.smart_all_tasks } else { strings.smart_do_not_show }).into(), smart_items, cx);

        let row = |label: &'static str, height: f32, content: AnyElement| {
            h_flex()
                .h(px(height))
                .flex_none()
                .child(div().w(px(120.)).flex_none().child(Label::new(label)))
                .child(content)
        };
        let color = self.draft.color.clone();
        let swatch = |id: SharedString, value: Option<&'static str>| {
            let selected = color.as_deref() == value;
            div()
                .id(id)
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .border_1()
                .border_color(if selected { theme.colors().accent } else { gpui::transparent_black() })
                .child(match parse_color(value) {
                    Some(fill) => div().size(px(15.)).rounded_full().bg(fill).into_any_element(),
                    // "No color": a ring with a slash, as the original's first swatch.
                    None => Icon::new(IconName::Ban).size(IconSize::Small).color(Color::Muted)
                        .into_any_element(),
                })
                .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |this| this.draft.color = value.map(str::to_owned))))
        };
        let layout = self.layout;
        let layout_button = |id: &'static str, icon: IconName, tip: &'static str, value: Layout| {
            let on = layout == value;
            div()
                .id(id)
                .w(px(51.))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .border_1()
                .border_color(if on { theme.colors().accent } else { gpui::transparent_black() })
                .bg(if on { theme.colors().accent.opacity(0.12) } else { theme.colors().element_hover })
                .child(Icon::new(icon).color(if on { Color::Accent } else { Color::Muted }))
                .tooltip(ui::Tooltip::text(tip))
                .on_click(cx.listener(move |this, _, _, cx| this.update(cx, |this| this.layout = value)))
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(30.))
                .px_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |el| el.bg(theme.colors().accent).child(Label::new(label).color(Color::Custom(gpui::white()))))
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).child(Label::new(label)))
        };
        let icon_cell = div()
            .relative()
            .child(
                div()
                    .id("list-dialog-icon")
                    .w(px(34.))
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_r_1()
                    .border_color(theme.colors().border)
                    .cursor_pointer()
                    .map(|el| match &self.draft.icon {
                        Some(icon) => el.child(div().text_size(px(16.)).child(icon.clone())),
                        None => el.child(
                            Icon::new(if open == Some(Menu::Icon) { IconName::Smile } else { IconName::Menu })
                                .size(IconSize::Small)
                                .color(if open == Some(Menu::Icon) { Color::Accent } else { Color::Muted }),
                        ),
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle(Menu::Icon, window, cx))),
            )
            .children(picker.map(|picker| PopoverAttachment::new(Placement::BelowLeft, picker)));
        let swatches: Vec<_> = std::iter::once(swatch("list-color-none".into(), None))
            .chain(LIST_COLORS.iter().map(|c| swatch(SharedString::from(format!("list-color-{c}")), Some(*c))))
            .collect();

        let form = v_flex()
            .w(px(382.))
            .flex_none()
            .h_full()
            .px(px(25.))
            .child(
                div()
                    .h(px(54.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(if self.editing.is_some() { strings.edit_list } else { strings.add_list })),
            )
            .child(
                h_flex().h(px(54.)).child(
                    h_flex()
                        .w_full()
                        .h(px(34.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.colors().border)
                        .child(icon_cell)
                        .child(div().flex_1().px_2().child(self.name.clone())),
                ),
            )
            .child(row(strings.list_color, 48., h_flex().gap(px(4.)).children(swatches).into_any_element()))
            .child(row(
                strings.list_view_type,
                48.,
                h_flex()
                    .gap(px(12.))
                    .child(layout_button("list-layout-list", IconName::Rows3, strings.layout_list, Layout::List))
                    .child(layout_button("list-layout-kanban", IconName::Kanban, strings.layout_kanban, Layout::Kanban))
                    .child(layout_button("list-layout-timeline", IconName::ChartGantt, strings.layout_timeline, Layout::Timeline))
                    .into_any_element(),
            ))
            .child(row(strings.list_folder, 54., folder_menu))
            .child(row(strings.list_type, 44., type_menu))
            .child(row(strings.show_in_smart_list, 44., smart_menu))
            .children(self.error.clone().map(|e| Label::new(e).size(LabelSize::Small).color(Color::Error)))
            .child(div().flex_1())
            .child(
                h_flex()
                    .pb(px(17.))
                    .justify_end()
                    .gap(px(12.))
                    .child(button("list-dialog-cancel", strings.cancel, false).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Closed))))
                    .child(
                        button("list-dialog-save", if self.editing.is_some() { strings.save } else { strings.add }, true)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            );

        // The original's dialog: 742 × 423 over a dimmed background, centered on the window
        // (the view starts after the 50 px sections bar) and 43 px above the middle, as there.
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
                h_flex()
                    .id("list-dialog")
                    .relative()
                    .w(px(742.))
                    .h(px(423.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .bg(theme.colors().elevated_surface_background)
                    .border_1()
                    .border_color(theme.colors().border)
                    .shadow_lg()
                    .child(form)
                    .child(div().flex_1().h_full().child(self.render_preview(cx)))
                    .child(
                        div().absolute().top(px(14.)).right(px(14.)).child(
                            IconButton::new("list-dialog-x", IconName::X)
                                .icon_color(Color::Muted)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Closed))),
                        ),
                    ),
            )
            .children(self.new_folder.clone())
    }
}

/// The original's New Folder dialog (and Edit Folder for an existing one): a name with a folder
/// icon, Close and Save.
pub struct FolderDialog {
    store: Rc<Store>,
    editing: Option<Id>,
    name: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for FolderDialog {}

impl FolderDialog {
    pub fn new(store: Rc<Store>, editing: Option<Id>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strings = i18n::t(cx);
        let current = editing.and_then(|id| store.list_folders().ok()?.into_iter().find(|f| f.id == id));
        let name = cx.new(|cx| {
            let mut input = TextInput::new(Mode::SingleLine, strings.list_name, cx);
            if let Some(folder) = &current {
                input.set_text(folder.name.clone(), cx);
            }
            input
        });
        let subscriptions = vec![
            cx.subscribe(&name, |this, _, _: &Submitted, cx| this.save(cx)),
            cx.subscribe(&name, |_, _, _: &Cancelled, cx| cx.emit(DialogEvent::Closed)),
            cx.subscribe(&name, |_, _, _: &Changed, cx| cx.notify()),
        ];
        window.focus(&name.focus_handle(cx), cx);
        Self { store, editing, name, _subscriptions: subscriptions }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let result = match self.editing {
            Some(id) => self.store.rename_list_folder(id, &name).map(|_| id),
            None => self.store.add_list_folder(&name).map(|f| f.id),
        };
        if let Ok(id) = result {
            cx.emit(DialogEvent::FolderSaved(id));
        }
    }
}

impl Render for FolderDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let theme = cx.theme().clone();
        let empty = self.name.read(cx).text().trim().is_empty();
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
                    el.bg(theme.colors().accent.opacity(if empty { 0.5 } else { 1. }))
                        .child(Label::new(label).color(Color::Custom(gpui::white())))
                })
                .when(!primary, |el| el.border_1().border_color(theme.colors().border).child(Label::new(label)))
        };
        // 441 × 181, as the original's, where the list dialog is (see there).
        div()
            .absolute()
            .inset_0()
            .pr(px(50.))
            .pb(px(86.))
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .child(
                v_flex()
                    .id("folder-dialog")
                    .w(px(441.))
                    .h(px(181.))
                    .px(px(20.))
                    .rounded(px(10.))
                    .bg(theme.colors().elevated_surface_background)
                    .border_1()
                    .border_color(theme.colors().border)
                    .shadow_lg()
                    .child(
                        h_flex()
                            .h(px(56.))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if self.editing.is_some() { strings.edit_folder } else { strings.new_folder }),
                            )
                            .child(
                                IconButton::new("folder-dialog-x", IconName::X)
                                    .icon_color(Color::Muted)
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Closed))),
                            ),
                    )
                    .child(
                        h_flex()
                            .h(px(36.))
                            .px_2p5()
                            .gap_2()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.colors().border)
                            .child(Icon::new(IconName::Folder).size(IconSize::Small).color(Color::Muted))
                            .child(div().flex_1().child(self.name.clone())),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .pb(px(18.))
                            .justify_end()
                            .gap(px(10.))
                            .child(button("folder-dialog-close", strings.close, false).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Closed))))
                            .child(button("folder-dialog-save", strings.save, true).on_click(cx.listener(|this, _, _, cx| this.save(cx)))),
                    ),
            )
    }
}
