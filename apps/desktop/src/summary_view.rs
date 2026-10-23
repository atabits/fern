//! The Summary smart list, modeled on the original's: a report of the period's tasks on the left,
//! its Filter and Display Options on the right, and "Copy" to take it as Markdown.

use std::rc::Rc;

use chrono::{Local, NaiveDate};
use fern_core::markdown::{self, Format};
use fern_core::summary::{Field, GroupKey, Grouping, Period, StatusFilter, SummaryGroup, SummaryOptions, summarize};
use fern_core::{List, Priority, Store, Task};
use gpui::{ClipboardItem, Entity, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::Tooltip;
use ui::{Placement, PopoverAttachment, SubmenuItem, popover};

use crate::actions as md;
use crate::calendar;
use crate::i18n::{self, Strings};
use crate::task_rows::{local_day, short_date};
use crate::text_input::{Cancelled, Mode, Submitted, TextInput};

/// What picking a dropdown option does.
type Pick = Box<dyn Fn(&mut SummaryView, &mut Window, &mut Context<SummaryView>)>;

/// The open dropdown, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Period,
    Lists,
    /// "More": Tag and Priority, each in a side menu.
    More,
    Status,
    Grouping,
    Fields,
}

pub struct SummaryView {
    store: Rc<Store>,
    options: SummaryOptions,
    menu: Option<Menu>,
    /// "Copied" shows on the button after a copy.
    copied: bool,
    /// Where "Save as → Image" wrote the picture, or why it could not.
    saved: Option<String>,
    /// The "Save as" menu, while open.
    save_menu: bool,
    /// The report as an editable document, the original's way: Markdown text that Copy and
    /// Send Email then use. New options make the report again.
    editor: Option<Entity<TextInput>>,
    /// Whether the editor shows (it keeps its text while hidden).
    editing: bool,
    /// The toolbar's "Heading Options" menu, while open.
    heading_menu: bool,
    /// The name field of "Save as Template", while open.
    naming: Option<Entity<TextInput>>,
    /// The "More" menu's open side menu: 0 Tag, 1 Priority.
    more_side: Option<u8>,
    /// The "Custom" date fields, while open.
    custom: Option<(Entity<TextInput>, Entity<TextInput>)>,
}

/// A rendered summary: the period's heading and each group's heading and lines.
pub struct Report {
    pub heading: String,
    pub groups: Vec<(String, Vec<String>)>,
}

impl Report {
    /// A `mailto:` link with the report as the message, for "Save as → Send Email".
    pub fn mailto(&self, subject: &str) -> String {
        format!("mailto:?subject={}&body={}", percent_encode(subject), percent_encode(&self.markdown()))
    }

    /// The report as Markdown, as "Copy" puts it on the clipboard.
    pub fn markdown(&self) -> String {
        let mut out = format!("# {}\n", self.heading);
        for (title, lines) in &self.groups {
            out.push_str(&format!("\n## {title}\n"));
            for line in lines {
                out.push_str(&format!("- {line}\n"));
            }
        }
        out
    }
}

/// Percent-encodes everything but unreserved characters, for a URL query.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn period_label(period: Period, today: NaiveDate, strings: &Strings) -> String {
    let Some(index) = Period::ALL.iter().position(|p| *p == period) else {
        let (first, last) = period.range(today, calendar::week_start());
        return format!("{} - {}", short_date(first, strings), short_date(last, strings));
    };
    let name = strings.summary_periods[index];
    match period {
        Period::ThisWeek | Period::NextWeek | Period::LastWeek => {
            let (first, last) = period.range(today, calendar::week_start());
            format!("{name} ({} - {})", short_date(first, strings), short_date(last, strings))
        }
        _ => name.to_owned(),
    }
}

fn group_title(key: &GroupKey, lists: &[List], strings: &Strings) -> String {
    match key {
        GroupKey::Completed => strings.completed.to_owned(),
        GroupKey::Uncompleted => strings.summary_statuses[2].to_owned(),
        GroupKey::WontDo => strings.wont_do.to_owned(),
        GroupKey::NextPeriod => strings.summary_next_period.to_owned(),
        GroupKey::List(None) => strings.inbox.to_owned(),
        GroupKey::List(Some(id)) => lists.iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default(),
        GroupKey::Date(Some(date)) => short_date(*date, strings),
        GroupKey::Date(None) => strings.group_no_date.to_owned(),
        GroupKey::Tag(Some(tag)) => format!("#{tag}"),
        GroupKey::Tag(None) => strings.group_no_tags.to_owned(),
        GroupKey::Priority(priority) => match priority {
            Priority::High => strings.priority_high,
            Priority::Medium => strings.priority_medium,
            Priority::Low => strings.priority_low,
            Priority::None => strings.priority_none,
        }
        .to_owned(),
    }
}

/// One task's line with the chosen fields: "[x] [Oct 17] Title · Oct 20 · Work #home".
fn line(task: &Task, fields: &[Field], lists: &[List], strings: &Strings) -> String {
    let mut out = String::new();
    if fields.contains(&Field::Status) {
        out.push_str(if task.is_completed() { "[x] " } else if task.is_wont_do() { "[-] " } else { "[ ] " });
    }
    if fields.contains(&Field::CompletionTime)
        && let Some(day) = task.completed_at.or(task.wont_do_at).and_then(local_day)
    {
        out.push_str(&format!("[{}] ", short_date(day, strings)));
    }
    out.push_str(&task.title);
    if fields.contains(&Field::TaskTime)
        && let Some(due) = task.due_date
    {
        out.push_str(&format!(" · {}", short_date(due, strings)));
        if let Some(time) = task.due_time {
            out.push_str(&format!(" {}", calendar::format_time(time)));
        }
    }
    if fields.contains(&Field::List) {
        let list = task.list_id.and_then(|id| lists.iter().find(|l| l.id == id)).map_or(strings.inbox, |l| l.name.as_str());
        out.push_str(&format!(" · {list}"));
    }
    if fields.contains(&Field::Tag) {
        for tag in &task.tags {
            out.push_str(&format!(" #{tag}"));
        }
    }
    out
}

pub fn report(groups: &[SummaryGroup], options: &SummaryOptions, lists: &[List], today: NaiveDate, strings: &Strings) -> Report {
    let (first, last) = options.period.range(today, calendar::week_start());
    let heading = if first == last {
        short_date(first, strings)
    } else {
        format!("{} - {}", short_date(first, strings), short_date(last, strings))
    };
    let groups = groups
        .iter()
        .map(|g| (group_title(&g.key, lists, strings), g.tasks.iter().map(|t| line(t, &options.fields, lists, strings)).collect()))
        .collect();
    Report { heading, groups }
}

impl SummaryView {
    pub fn new(store: Rc<Store>) -> Self {
        // The original keeps the Summary's choices; so does Fern, in the settings table.
        let options = store.summary_options().unwrap_or_default();
        Self {
            store,
            options,
            menu: None,
            copied: false,
            saved: None,
            custom: None,
            save_menu: false,
            naming: None,
            more_side: None,
            editor: None,
            editing: false,
            heading_menu: false,
        }
    }

    /// "Custom" in the Date menu: Start and End fields with Cancel and Confirm.
    fn open_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let (first, last) = self.options.period.range(today, calendar::week_start());
        let field = |placeholder: &'static str, date: NaiveDate, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, placeholder, cx);
                input.set_text(calendar::format_date(date, strings), cx);
                input
            })
        };
        let start = field(strings.summary_start, first, cx);
        let end = field(strings.summary_end, last, cx);
        window.focus(&start.focus_handle(cx), cx);
        self.menu = None;
        self.custom = Some((start, end));
        cx.notify();
    }

    /// Template "+": a name for the current options.
    fn start_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.summary_template_name, cx));
        cx.subscribe(&input, |this, _, Submitted(name), cx| this.save_template(name.clone(), cx)).detach();
        cx.subscribe(&input, |this, _, _: &Cancelled, cx| {
            this.naming = None;
            cx.notify();
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.naming = Some(input);
        cx.notify();
    }

    fn save_template(&mut self, name: String, cx: &mut Context<Self>) {
        match self.store.save_summary_template(&name, &self.options) {
            Ok(_) => self.naming = None,
            Err(err) => eprintln!("failed to save the summary template: {err:#}"),
        }
        cx.notify();
    }

    fn confirm_custom(&mut self, cx: &mut Context<Self>) {
        let Some((start, end)) = &self.custom else { return };
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let parse = |input: &Entity<TextInput>| calendar::parse_date(input.read(cx).text(), today, strings);
        if let (Some(first), Some(last)) = (parse(start), parse(end)) {
            self.custom = None;
            self.update(cx, |o| o.period = Period::Custom(first, last));
        }
    }

    fn set_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        self.menu = menu;
        cx.notify();
    }

    fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut SummaryOptions)) {
        change(&mut self.options);
        if let Err(err) = self.store.save_summary_options(&self.options) {
            eprintln!("failed to save the summary options: {err:#}");
        }
        self.copied = false;
        // A report made with other options replaces the edited one.
        self.editor = None;
        self.editing = false;
        cx.notify();
    }

    /// The pencil in the report's header: edit the report as text, or show it again.
    fn toggle_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing {
            self.editing = false;
        } else {
            if self.editor.is_none() {
                let markdown = self.current_report(cx).markdown();
                self.editor = Some(cx.new(|cx| {
                    let mut input = TextInput::new(Mode::MultiLine, "", cx);
                    input.set_text(markdown, cx);
                    input
                }));
            }
            if let Some(editor) = &self.editor {
                window.focus(&editor.focus_handle(cx), cx);
            }
            self.editing = true;
        }
        self.copied = false;
        cx.notify();
    }

    /// A toolbar button or its shortcut: formats the editor's selection as Markdown, or for
    /// `None` (Time) types the current time.
    fn format(&mut self, format: Option<Format>, window: &mut Window, cx: &mut Context<Self>) {
        self.heading_menu = false;
        // As in the original, the report is always editable: a button starts editing it.
        if !self.editing {
            self.toggle_editing(window, cx);
        }
        let Some(editor) = self.editor.clone().filter(|_| self.editing) else { return };
        editor.update(cx, |input, cx| {
            let selection = input.selection();
            let (text, selection) = match format {
                Some(format) => markdown::apply(input.text(), selection, format),
                None => markdown::insert(input.text(), selection, &calendar::format_time(Local::now().time())),
            };
            input.edit(text, selection, window, cx);
        });
        window.focus(&editor.focus_handle(cx), cx);
        self.copied = false;
        cx.notify();
    }

    /// The original's toolbar over the edited report (see `markdown_toolbar`).
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.theme().colors().clone();
        let on_action: crate::markdown_toolbar::OnAction<Self> = std::rc::Rc::new(|this, action, window, cx| match action {
            crate::markdown_toolbar::ToolbarAction::Format(format) => this.format(format, window, cx),
            crate::markdown_toolbar::ToolbarAction::ToggleHeadings => {
                this.heading_menu = !this.heading_menu;
                cx.notify();
            }
            crate::markdown_toolbar::ToolbarAction::Attach => {}
        });
        crate::markdown_toolbar::render("summary-tool", self.heading_menu, false, cx, on_action)
            .h(px(36.))
            .px_2()
            .border_b_1()
            .border_color(colors.border_variant)
    }

    /// Opens a dropdown from the debug-build dev state: `summary=<period|lists|status|grouping|fields>`,
    /// `summary=custom` (the Custom date fields) or `summary=next` (Next Period Tasks on).
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, menu: &str, window: &mut Window, cx: &mut Context<Self>) {
        if menu == "edit" || menu == "edit-heading" {
            self.toggle_editing(window, cx);
            self.heading_menu = menu == "edit-heading";
            return;
        }
        if menu == "custom" {
            self.open_custom(window, cx);
            return;
        }
        self.menu = match menu {
            "period" => Some(Menu::Period),
            "lists" => Some(Menu::Lists),
            "more" => Some(Menu::More),
            "status" => Some(Menu::Status),
            "grouping" => Some(Menu::Grouping),
            "fields" => Some(Menu::Fields),
            _ => None,
        };
        if menu == "next" {
            self.options.next_period = true;
        }
        if menu == "habits" {
            self.options.habits = true;
        }
        if menu == "more-tag" {
            self.menu = Some(Menu::More);
            self.more_side = Some(0);
        }
        if menu == "save" {
            self.save_menu = true;
        }
        // `summary=image:<path>`: what "Save as → Image" writes, to a given file.
        if let Some(path) = menu.strip_prefix("image:") {
            let report = self.current_report(cx);
            std::fs::write(path, crate::print::report_svg(&report.heading, &report.groups)).ok();
        }
        if let Some(name) = menu.strip_prefix("template:") {
            self.store.save_summary_template(name, &self.options).ok();
        }
        if menu == "template-new" {
            self.start_template(window, cx);
        }
        cx.notify();
    }

    fn current_report(&self, cx: &App) -> Report {
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let tasks = self.store.tasks().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        let groups = summarize(&tasks, &self.options, today, calendar::week_start());
        let mut report = report(&groups, &self.options, &lists, today, strings);
        // The Lists filter's "Habit": each habit's check-ins in the period.
        let habits = self.store.habits().unwrap_or_default();
        let checkins = habits.iter().map(|h| (h.id, self.store.checkins(h.id).unwrap_or_default())).collect();
        let rows = fern_core::summary::habit_checkins(&habits, &checkins, &self.options, today, calendar::week_start());
        if !rows.is_empty() {
            let lines = rows
                .into_iter()
                .map(|(habit, days)| strings.summary_habit_line.replace("{name}", &habit.name).replace("{n}", &days.to_string()))
                .collect();
            report.groups.push((strings.summary_habits.to_owned(), lines));
        }
        report
    }

    /// A Filter or Display Options row whose value opens a dropdown.
    fn row(
        &self,
        label: &'static str,
        value: String,
        menu: Menu,
        options: Vec<(String, bool, Pick)>,
        // Fields stay open to pick several.
        keep_open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.menu == Some(menu);
        let dropdown = open.then(|| {
            let options = Rc::new(options);
            let items = (0..options.len()).map(|i| {
                let (label, selected, _) = &options[i];
                let options = options.clone();
                ListItem::new(SharedString::from(format!("summary-option-{i}")))
                    .child(Label::new(label.clone()).color(if *selected { Color::Accent } else { Color::Default }))
                    .when(*selected, |item| item.end_slot(Icon::new(IconName::Check).color(Color::Accent).size(IconSize::Small)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        (options[i].2)(this, window, cx);
                        if !keep_open {
                            this.set_menu(None, cx);
                        }
                    }))
            });
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.set_menu(None, cx)))
                    .children(items),
            )
        });
        h_flex()
            .h(px(40.))
            .px_4()
            .child(div().flex_1().child(Label::new(label)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("summary-{label}")))
                    .relative()
                    .gap_1()
                    .cursor_pointer()
                    .child(Label::new(value).color(Color::Muted))
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
                    .children(dropdown),
            )
    }

    /// The original's Template row: saved option sets to apply with a click, and "+" to save the
    /// current ones under a name.
    fn render_templates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let templates = self.store.summary_templates().unwrap_or_default();
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(templates.into_iter().map(|template| {
                        let selected = template.options == self.options;
                        let (id, options) = (template.id, template.options.clone());
                        let group = SharedString::from(format!("template-{id}"));
                        h_flex()
                            .id(group.clone())
                            .group(group.clone())
                            .h(px(26.))
                            .pl_2()
                            .pr_1()
                            .gap_1()
                            .rounded(px(13.))
                            .border_1()
                            .border_color(if selected { colors.accent } else { colors.border })
                            .cursor_pointer()
                            .child(Label::new(template.name).size(LabelSize::Small).color(if selected { Color::Accent } else { Color::Default }))
                            .child(
                                div().when(!crate::touch::active(), |el| el.invisible()).group_hover(group, |s| s.visible()).child(
                                    IconButton::new(SharedString::from(format!("delete-template-{id}")), IconName::X)
                                        .size(px(16.))
                                        .icon_size(IconSize::XSmall)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if let Err(err) = this.store.delete_summary_template(id) {
                                                eprintln!("failed to delete the summary template: {err:#}");
                                            }
                                            cx.notify();
                                        })),
                                ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let options = options.clone();
                                this.update(cx, |o| *o = options);
                            }))
                    }))
                    .child(
                        IconButton::new("add-summary-template", IconName::Plus)
                            .size(px(26.))
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text(strings.summary_save_template))
                            .on_click(cx.listener(|this, _, window, cx| this.start_template(window, cx))),
                    ),
            )
            .children(self.naming.clone().map(|input| {
                v_flex()
                    .gap_1p5()
                    .child(Label::new(strings.summary_template_hint).size(LabelSize::XSmall).color(Color::Muted))
                    .child(h_flex().h(px(28.)).px_2().rounded(px(6.)).bg(colors.surface_background).child(input.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                div()
                                    .id("template-close")
                                    .px_3()
                                    .h(px(26.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(colors.border)
                                    .cursor_pointer()
                                    .child(Label::new(strings.close).size(LabelSize::Small))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.naming = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("template-save")
                                    .px_3()
                                    .h(px(26.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .bg(colors.accent)
                                    .cursor_pointer()
                                    .child(Label::new(strings.save).size(LabelSize::Small).color(Color::Custom(gpui::white())))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let name = input.read(cx).text().to_owned();
                                        this.save_template(name, cx);
                                    })),
                            ),
                    )
            }))
    }

    /// The original's "More" filter: Tag and Priority side menus with checkable entries.
    fn render_more_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let options = &self.options;
        let open = self.menu == Some(Menu::More);
        let priorities = [
            (Priority::High, strings.priority_high),
            (Priority::Medium, strings.priority_medium),
            (Priority::Low, strings.priority_low),
            (Priority::None, strings.priority_none),
        ];
        let mut parts = Vec::new();
        if let Some(tags) = &options.tags {
            parts.push(tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(", "));
        }
        if let Some(chosen) = &options.priorities {
            parts.push(priorities.iter().filter(|(p, _)| chosen.contains(p)).map(|(_, n)| *n).collect::<Vec<_>>().join(", "));
        }
        let value = if parts.is_empty() { strings.none_option.to_owned() } else { parts.join("; ") };
        let check = |selected: bool, item: ListItem| {
            item.when(selected, |item| item.end_slot(Icon::new(IconName::Check).size(IconSize::Small).color(Color::Accent)))
        };
        let dropdown = open.then(|| {
            let tags = self.store.tags().unwrap_or_default();
            let tag_items = std::iter::once(
                check(options.tags.is_none(), ListItem::new("more-tag-all").child(Label::new(strings.summary_all_tags)))
                    .on_click(cx.listener(|this, _, _, cx| this.update(cx, |o| o.tags = None)))
                    .into_any_element(),
            )
            .chain(tags.into_iter().map(|tag| {
                let name = tag.name.clone();
                let selected = options.tags.as_ref().is_some_and(|t| t.contains(&name));
                check(selected, ListItem::new(SharedString::from(format!("more-tag-{name}"))).child(Label::new(format!("#{name}"))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let name = name.clone();
                        this.update(cx, |o| {
                            let mut chosen = o.tags.take().unwrap_or_default();
                            match chosen.iter().position(|t| *t == name) {
                                Some(i) => {
                                    chosen.remove(i);
                                }
                                None => chosen.push(name),
                            }
                            o.tags = (!chosen.is_empty()).then_some(chosen);
                        })
                    }))
                    .into_any_element()
            }))
            .collect::<Vec<_>>();
            let priority_items = std::iter::once(
                check(options.priorities.is_none(), ListItem::new("more-priority-all").child(Label::new(strings.summary_all_priorities)))
                    .on_click(cx.listener(|this, _, _, cx| this.update(cx, |o| o.priorities = None)))
                    .into_any_element(),
            )
            .chain(priorities.into_iter().map(|(priority, name)| {
                let selected = options.priorities.as_ref().is_some_and(|p| p.contains(&priority));
                check(selected, ListItem::new(SharedString::from(format!("more-priority-{}", priority.to_i64()))).child(Label::new(name)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update(cx, |o| {
                            let mut chosen = o.priorities.take().unwrap_or_default();
                            match chosen.iter().position(|p| *p == priority) {
                                Some(i) => {
                                    chosen.remove(i);
                                }
                                None => chosen.push(priority),
                            }
                            o.priorities = (!chosen.is_empty()).then_some(chosen);
                        })
                    }))
                    .into_any_element()
            }))
            .collect::<Vec<_>>();
            let side = |index: u8, id: &'static str, label: &'static str, items: Vec<AnyElement>, cx: &mut Context<Self>| {
                SubmenuItem::new(id, label).open(self.more_side == Some(index)).items(items).on_hover(cx.listener(
                    move |this, hovered: &bool, _, cx| {
                        if *hovered && this.more_side != Some(index) {
                            this.more_side = Some(index);
                            cx.notify();
                        }
                    },
                ))
            };
            PopoverAttachment::new(
                Placement::BelowRight,
                popover(cx)
                    .min_w(px(200.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.more_side = None;
                        this.set_menu(None, cx);
                    }))
                    .child(side(0, "more-tag", strings.summary_tag, tag_items, cx))
                    .child(side(1, "more-priority", strings.summary_priority, priority_items, cx)),
            )
        });
        h_flex()
            .h(px(40.))
            .px_4()
            .child(div().flex_1().child(Label::new(strings.summary_more)))
            .child(
                h_flex()
                    .id("summary-more")
                    .relative()
                    .gap_1()
                    .max_w(px(170.))
                    .cursor_pointer()
                    .child(Label::new(value).color(Color::Muted).truncate())
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(Menu::More), cx)))
                    .children(dropdown),
            )
    }

    fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let today = Local::now().date_naive();
        let options = &self.options;
        let group = || v_flex().py_1().rounded(px(8.)).bg(colors.surface_background);
        let heading = |text: &'static str| div().pt_3().pb_1p5().child(Label::new(text).weight(FontWeight::SEMIBOLD));

        let mut periods: Vec<(String, bool, Pick)> = Period::ALL
            .into_iter()
            .map(|period| {
                let pick: Pick = Box::new(move |this, _, cx| this.update(cx, |o| o.period = period));
                (period_label(period, today, strings), period == options.period, pick)
            })
            .collect();
        let custom = matches!(options.period, Period::Custom(..));
        periods.push((
            strings.summary_custom.to_owned(),
            custom,
            Box::new(|this, window, cx| this.open_custom(window, cx)),
        ));
        let lists = self.store.lists().unwrap_or_default();
        let list_label = match &options.lists {
            None if options.habits => format!("{}, {}", strings.summary_all_lists, strings.summary_habits),
            None => strings.summary_all_lists.to_owned(),
            Some(chosen) => chosen
                .iter()
                .map(|id| match id {
                    None => strings.inbox.to_owned(),
                    Some(id) => lists.iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        };
        let mut list_options: Vec<(String, bool, Pick)> = vec![(
            strings.summary_all_lists.to_owned(),
            options.lists.is_none(),
            Box::new(|this, _, cx| this.update(cx, |o| o.lists = None)),
        )];
        list_options.push((
            strings.summary_habits.to_owned(),
            options.habits,
            Box::new(|this, _, cx| this.update(cx, |o| o.habits = !o.habits)),
        ));
        let choices = std::iter::once((None, strings.inbox.to_owned())).chain(lists.iter().map(|l| (Some(l.id), l.name.clone())));
        list_options.extend(choices.map(|(id, name)| {
            let selected = options.lists.as_ref().is_some_and(|chosen| chosen.contains(&id));
            let pick: Pick = Box::new(move |this, _, cx| {
                this.update(cx, |o| {
                    let mut chosen = o.lists.take().unwrap_or_default();
                    match chosen.iter().position(|c| *c == id) {
                        Some(i) => {
                            chosen.remove(i);
                        }
                        None => chosen.push(id),
                    }
                    o.lists = (!chosen.is_empty()).then_some(chosen);
                })
            });
            (name, selected, pick)
        }));
        let statuses = StatusFilter::ALL
            .into_iter()
            .zip(strings.summary_statuses)
            .map(|(status, name)| {
                let pick: Pick = Box::new(move |this, _, cx| this.update(cx, |o| o.status = status));
                (name.to_owned(), status == options.status, pick)
            })
            .collect();
        let groupings = Grouping::ALL
            .into_iter()
            .zip(strings.summary_groupings)
            .map(|(grouping, name)| {
                let pick: Pick = Box::new(move |this, _, cx| this.update(cx, |o| o.grouping = grouping));
                (name.to_owned(), grouping == options.grouping, pick)
            })
            .collect();
        let fields = Field::ALL
            .into_iter()
            .zip(strings.summary_fields)
            .map(|(field, name)| {
                let pick: Pick = Box::new(move |this, _, cx| {
                    this.update(cx, |o| match o.fields.iter().position(|f| *f == field) {
                        Some(i) => {
                            o.fields.remove(i);
                        }
                        None => o.fields.push(field),
                    })
                });
                (name.to_owned(), options.fields.contains(&field), pick)
            })
            .collect();
        let status_index = StatusFilter::ALL.iter().position(|s| *s == options.status).unwrap_or(0);
        let grouping_index = Grouping::ALL.iter().position(|g| *g == options.grouping).unwrap_or(0);

        v_flex()
            .flex_none()
            .w(px(320.))
            .h_full()
            .px_4()
            .pt_2()
            .border_l_1()
            .border_color(colors.border_variant)
            .child(heading(strings.summary_template))
            .child(self.render_templates(cx))
            .child(heading(strings.summary_filter))
            .child(
                group()
                    .child(self.row(strings.summary_date, period_label(options.period, today, strings), Menu::Period, periods, false, cx))
                    .children(self.custom.clone().map(|(start, end)| {
                        let field = |input: Entity<TextInput>| {
                            h_flex().h(px(28.)).px_2().rounded(px(6.)).bg(colors.background).child(input)
                        };
                        v_flex()
                            .mx_3()
                            .mb_2()
                            .gap_1p5()
                            .child(field(start))
                            .child(field(end))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        div()
                                            .id("summary-custom-cancel")
                                            .px_3()
                                            .h(px(26.))
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(colors.border)
                                            .cursor_pointer()
                                            .child(Label::new(strings.cancel).size(LabelSize::Small))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.custom = None;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("summary-custom-confirm")
                                            .px_3()
                                            .h(px(26.))
                                            .flex()
                                            .items_center()
                                            .rounded(px(6.))
                                            .bg(colors.accent)
                                            .cursor_pointer()
                                            .child(Label::new(strings.confirm).size(LabelSize::Small).color(Color::Custom(gpui::white())))
                                            .on_click(cx.listener(|this, _, _, cx| this.confirm_custom(cx))),
                                    ),
                            )
                    }))
                    .child(self.row(strings.summary_lists, list_label, Menu::Lists, list_options, true, cx))
                    .child(self.row(strings.summary_status, strings.summary_statuses[status_index].to_owned(), Menu::Status, statuses, false, cx))
                    .child(self.render_more_row(cx)),
            )
            .child(heading(strings.summary_display_options))
            .child(
                group()
                    .child(self.row(strings.summary_grouping, strings.summary_groupings[grouping_index].to_owned(), Menu::Grouping, groupings, false, cx))
                    .child(self.row(
                        strings.summary_fields_label,
                        strings.summary_selected.replace("{n}", &(options.fields.len() + 1).to_string()),
                        Menu::Fields,
                        fields,
                        true,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .id("summary-next-period")
                    .pt_3()
                    .cursor_pointer()
                    .child(div().flex_1().child(Label::new(strings.summary_next_period).weight(FontWeight::SEMIBOLD)))
                    .child(Switch::new().toggle_state(options.next_period))
                    .on_click(cx.listener(|this, _, _, cx| this.update(cx, |o| o.next_period = !o.next_period))),
            )
    }
}

impl Render for SummaryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let report = self.current_report(cx);
        let empty = report.groups.is_empty();
        // An edited report is what Copy and Send Email take.
        let edited = self.editor.as_ref().map(|editor| editor.read(cx).text().to_owned());
        let markdown = edited.clone().unwrap_or_else(|| report.markdown());
        let subject = format!("{} {}", strings.summary, report.heading);
        let mailto = match &edited {
            Some(text) => format!("mailto:?subject={}&body={}", percent_encode(&subject), percent_encode(text)),
            None => report.mailto(&subject),
        };
        let (report_heading, report_groups) = (format!("{} {}", strings.summary, report.heading), report.groups.clone());

        let editor = self.editor.clone().filter(|_| self.editing);
        let body = v_flex()
            .id("summary-report")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .gap_1()
            // A click in the report edits it, as the original's always-editable report.
            .when(editor.is_none(), |el| el.cursor_text().on_click(cx.listener(|this, _, window, cx| this.toggle_editing(window, cx))))
            .when_some(editor.clone(), |el, editor| {
                el.child(
                    div()
                        .key_context("SummaryEditor")
                        .p_2()
                        .rounded(px(6.))
                        .bg(colors.surface_background)
                        .text_size(px(14.))
                        .on_action(cx.listener(|this, _: &md::Bold, window, cx| this.format(Some(Format::Bold), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Highlight, window, cx| this.format(Some(Format::Highlight), window, cx)))
                        .on_action(cx.listener(|this, _: &md::CheckItem, window, cx| this.format(Some(Format::Checklist), window, cx)))
                        .on_action(cx.listener(|this, _: &md::BulletedList, window, cx| this.format(Some(Format::Bullets), window, cx)))
                        .on_action(cx.listener(|this, _: &md::NumberedList, window, cx| this.format(Some(Format::Numbered), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Italic, window, cx| this.format(Some(Format::Italic), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Underline, window, cx| this.format(Some(Format::Underline), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Strike, window, cx| this.format(Some(Format::Strike), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Link, window, cx| this.format(Some(Format::Link), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Code, window, cx| this.format(Some(Format::Code), window, cx)))
                        .on_action(cx.listener(|this, _: &md::Quote, window, cx| this.format(Some(Format::Quote), window, cx)))
                        .child(editor),
                )
            })
            .when(editor.is_none(), |el| el.child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).pb_1().child(report.heading.clone())))
            .when(editor.is_none() && empty, |el| el.child(Label::new(strings.summary_empty).color(Color::Muted)))
            .children(report.groups.into_iter().filter(|_| editor.is_none()).map(|(title, lines)| {
                v_flex()
                    .pt_1()
                    .child(div().pb_1().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .children(lines.into_iter().map(|line| {
                        h_flex()
                            .gap_2()
                            .pl_1()
                            .child(div().size(px(5.)).rounded_full().bg(colors.accent))
                            .child(Label::new(line))
                    }))
            }));

        h_flex()
            .size_full()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .h(px(52.))
                            .px_4()
                            .gap_2()
                            .child(div().flex_1().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(strings.summary)),
                    )
                    // The original's report: its toolbar always on top, in a frame with the report.
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .mx_4()
                            .rounded(px(8.))
                            .border_1()
                            .border_color(colors.border_variant)
                            .child(self.render_toolbar(cx))
                            .child(body),
                    )
                    .child(
                        h_flex().h(px(44.)).px_4().gap_5().justify_end()
                        .children(self.saved.clone().map(|message| {
                            div().flex_1().min_w_0().child(Label::new(message).size(LabelSize::XSmall).color(Color::Muted).truncate())
                        })).child({
                            let heading = report_heading.clone();
                            let groups = report_groups.clone();
                            let mail = mailto.clone();
                            let menu = self.save_menu.then(|| {
                                PopoverAttachment::new(
                                    Placement::AboveRight,
                                    popover(cx)
                                        .min_w(px(180.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.save_menu = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("save-pdf")
                                                .start_slot(Icon::new(IconName::FileText).color(Color::Error))
                                                .child(Label::new(strings.summary_pdf))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.save_menu = false;
                                                    let html = crate::print::report_html(&heading, &groups);
                                                    if let Err(err) = crate::print::open("summary", &html) {
                                                        eprintln!("failed to open the summary for printing: {err:#}");
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child({
                                            let (heading, groups) = (report_heading.clone(), report_groups.clone());
                                            ListItem::new("save-image")
                                                .start_slot(Icon::new(IconName::Image).color(Color::Warning))
                                                .child(Label::new(strings.summary_image))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.save_menu = false;
                                                    let name = format!("fern-summary-{}.svg", Local::now().format("%Y-%m-%d"));
                                                    let svg = crate::print::report_svg(&heading, &groups).into_bytes();
                                                    crate::export::save_as(name, svg, cx, |this, result, cx| {
                                                        if let Some(message) = crate::export::outcome(result, i18n::t(cx)) {
                                                            this.saved = Some(message);
                                                        }
                                                        cx.notify();
                                                    });
                                                    cx.notify();
                                                }))
                                        })
                                        .child(
                                            ListItem::new("save-email")
                                                .start_slot(Icon::new(IconName::Mail).color(Color::Accent))
                                                .child(Label::new(strings.summary_send_email))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.save_menu = false;
                                                    cx.open_url(&mail);
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            });
                            h_flex()
                                .id("summary-save-as")
                                .relative()
                                .gap_1()
                                .cursor_pointer()
                                .child(Icon::new(IconName::Share).size(IconSize::Small).color(Color::Muted))
                                .child(Label::new(strings.summary_save_as))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_menu = !this.save_menu;
                                    cx.notify();
                                }))
                                .children(menu)
                        }).child(
                            h_flex()
                                .id("summary-copy")
                                .gap_1()
                                .cursor_pointer()
                                .child(Icon::new(IconName::Copy).size(IconSize::Small).color(Color::Muted))
                                .child(Label::new(if self.copied { strings.summary_copied } else { strings.summary_copy }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(markdown.clone()));
                                    this.copied = true;
                                    cx.notify();
                                })),
                        ),
                    ),
            )
            .child(self.render_options(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;

    #[test]
    fn report_lines_and_markdown() {
        let store = Store::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let done = store.add_task("Ship it").unwrap();
        store.set_tags(done.id, &["work".to_owned()]).unwrap();
        store.set_completed(done.id, true).unwrap();
        let tasks = store.tasks().unwrap();
        let options = SummaryOptions {
            period: Period::Today,
            fields: vec![Field::Status, Field::CompletionTime, Field::Tag],
            ..Default::default()
        };
        let groups = summarize(&tasks, &options, today, calendar::week_start());
        let report = report(&groups, &options, &[], today, &EN);
        let day = short_date(today, &EN);
        assert_eq!(report.groups, [("Completed".to_owned(), vec![format!("[x] [{day}] Ship it #work")])]);
        assert_eq!(report.markdown(), format!("# {day}\n\n## Completed\n- [x] [{day}] Ship it #work\n"));
        assert!(report.mailto("Summary").starts_with("mailto:?subject=Summary&body=%23%20"));
        assert_eq!(percent_encode("a b&c"), "a%20b%26c");
    }
}
