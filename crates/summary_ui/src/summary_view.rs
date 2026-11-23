//! The Summary smart list, modeled on the original's: a report of the period's tasks on the left,
//! its Filter and Display Options on the right, and "Copy" to take it as Markdown.

use std::rc::Rc;

use chrono::{Local, NaiveDate};
use fern_core::markdown::{self, Format};
use fern_core::summary::{Field, GroupKey, Period, SummaryGroup, SummaryOptions, summarize};
use fern_core::{List, Priority, Store, Task};
use gpui::{ClipboardItem, Entity, FocusHandle, Focusable as _, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::actions as md;
use crate::calendar;
use crate::i18n::{self, Strings};
use crate::task_rows::{local_day, short_date};
use crate::text_input::{Cancelled, Changed, Mode, Submitted, TextInput};
mod menus;
#[path = "summary_fields.rs"]
mod summary_fields;

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

/// Cached summary report & filter metadata so `SummaryView::render()` performs zero SQLite reads.
struct CachedSummary {
    report: Report,
    lists: Vec<List>,
    tags: Vec<fern_core::Tag>,
    templates: Vec<fern_core::summary::SummaryTemplate>,
    today: NaiveDate,
    locale: i18n::Locale,
}

pub struct SummaryView {
    store: Rc<Store>,
    focus_handle: FocusHandle,
    options: SummaryOptions,
    menu: Option<Menu>,
    /// "Copied" shows on the button after a copy.
    copied: bool,
    /// Pending timer that clears `copied`, stored so rapid copies cancel the previous timer.
    copied_reset: Option<gpui::Task<()>>,
    /// Cached generated report, lists, tags, and templates.
    cache: Option<CachedSummary>,
    /// Set whenever options, templates, locale, or `DataVersion` change.
    cache_dirty: bool,
    /// Lazily registers global `DataVersion` and `CurrentLocale` observers on first context access.
    observers_registered: bool,
    /// Whether this view is currently visible; hidden views defer `cx.notify()` until shown.
    active: bool,
    #[cfg(test)]
    rebuild_count: usize,
    /// Where "Save as → Image" wrote the picture, or why it could not.
    saved: Option<String>,
    /// The "Save as" menu, while open.
    save_menu: bool,
    /// The "Copy" format menu (Markdown / Plain Text), while open.
    copy_menu: bool,
    /// The report as an editable document, the original's way: Markdown text that Copy and
    /// Send Email then use. New options make the report again.
    editor: Option<Entity<TextInput>>,
    /// The generated Markdown when `editor` was opened, to detect manual edits.
    editor_initial: String,
    /// Pending option change awaiting confirmation when manual edits would be overwritten.
    pending_options: Option<SummaryOptions>,
    /// Pending summary template deletion awaiting user confirmation: `(id, name)`.
    confirm_delete_template: Option<(fern_core::Id, String)>,
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
    /// Restores focus to `focus_handle` after `dismiss_transient` closes an inline input.
    pending_focus: bool,
}

/// A rendered summary: the period's heading and each group's heading and lines.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    if fields.contains(&Field::Title) { out.push_str(&task.title); }
    if fields.contains(&Field::TaskTime)
        && let Some(due) = task.due_date
    {
        out.push_str(&format!(" · {}", short_date(due, strings)));
        if let Some(time) = task.due_time {
            out.push_str(&format!(" {}", calendar::format_time(time, strings)));
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

impl gpui::Focusable for SummaryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SummaryView {
    pub fn new(store: Rc<Store>, cx: &mut Context<Self>) -> Self {
        // The original keeps the Summary's choices; so does Fern, in the settings table.
        let options = store.summary_options().unwrap_or_default();
        Self {
            store,
            focus_handle: cx.focus_handle(),
            options,
            menu: None,
            copied: false,
            copied_reset: None,
            cache: None,
            cache_dirty: true,
            observers_registered: false,
            active: true,
            #[cfg(test)]
            rebuild_count: 0,
            saved: None,
            custom: None,
            save_menu: false,
            copy_menu: false,
            naming: None,
            more_side: None,
            editor: None,
            editor_initial: String::new(),
            pending_options: None,
            confirm_delete_template: None,
            editing: false,
            heading_menu: false,
            pending_focus: false,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.invalidate_cache(cx);
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active
            && (self.cache_dirty
                || self
                    .cache
                    .as_ref()
                    .is_none_or(|c| c.today != Local::now().date_naive()))
        {
            cx.notify();
        }
    }

    fn ensure_observers(&mut self, cx: &mut Context<Self>) {
        if self.observers_registered {
            return;
        }
        self.observers_registered = true;
        cx.observe_global::<desktop_common::data_version::DataVersion>(|this, cx| {
            if desktop_common::data_version::changed_elsewhere(cx.entity_id(), cx) {
                this.invalidate_cache(cx);
            }
        })
        .detach();
        cx.observe_global::<i18n::CurrentLocale>(|this, cx| {
            this.invalidate_cache(cx);
        })
        .detach();
    }

    fn invalidate_cache(&mut self, cx: &mut Context<Self>) {
        self.cache_dirty = true;
        if !self.editing && !self.has_manual_edits(cx) {
            self.editor = None;
            self.editor_initial.clear();
        }
        if self.active {
            cx.notify();
        }
    }

    fn refresh_templates(&mut self, cx: &mut Context<Self>) {
        if let Some(cache) = self.cache.as_mut() {
            cache.templates = self.store.summary_templates().unwrap_or_default();
        } else {
            self.cache_dirty = true;
        }
        cx.notify();
    }

    /// "Custom" in the Date menu: Start and End fields with Cancel and Confirm.
    fn open_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let (first, last) = self.options.period.range(today, calendar::week_start());
        let field = |placeholder: &'static str, date: NaiveDate, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(Mode::SingleLine, placeholder, cx);
                input.set_text(calendar::format_date(date, strings), cx);
                input
            });
            cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| {
                this.confirm_custom(window, cx);
            })
            .detach();
            cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
                this.cancel_custom(window, cx);
            })
            .detach();
            input
        };
        let start = field(strings.summary_start, first, cx);
        let end = field(strings.summary_end, last, cx);
        window.focus(&start.focus_handle(cx), cx);
        self.menu = None;
        self.custom = Some((start, end));
        cx.notify();
    }

    fn cancel_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.custom = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Template "+": a name for the current options.
    fn start_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = i18n::t(cx);
        let input = cx.new(|cx| TextInput::new(Mode::SingleLine, strings.summary_template_name, cx));
        cx.subscribe(&input, |_, _, _: &Changed, cx| cx.notify()).detach();
        cx.subscribe_in(&input, window, |this, _, Submitted(name), window, cx| {
            this.save_template(name.clone(), window, cx);
        })
        .detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.cancel_template(window, cx);
        })
        .detach();
        window.focus(&input.focus_handle(cx), cx);
        self.naming = Some(input);
        cx.notify();
    }

    fn cancel_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.naming = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn save_template(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        match self.store.save_summary_template(&name, &self.options) {
            Ok(_) => {
                self.naming = None;
                window.focus(&self.focus_handle, cx);
                self.refresh_templates(cx);
            }
            Err(err) => {
                eprintln!("failed to save the summary template: {err:#}");
                cx.notify();
            }
        }
    }

    fn confirm_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((start, end)) = &self.custom else { return };
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let parse = |input: &Entity<TextInput>| calendar::parse_date(input.read(cx).text(), today, strings);
        if let (Some(first), Some(last)) = (parse(start), parse(end)) {
            self.custom = None;
            self.update(cx, |o| o.period = Period::Custom(first, last));
            window.focus(&self.focus_handle, cx);
        }
    }

    fn set_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        self.menu = menu;
        if menu != Some(Menu::More) {
            self.more_side = None;
        }
        cx.notify();
    }

    /// Closes the top-most open dropdown or inline prompt on Escape. Returns `true` if anything
    /// was dismissed so callers can stop propagation.
    pub fn dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        let dismissed = if self.pending_options.take().is_some()
            || self.confirm_delete_template.take().is_some()
            || self.more_side.take().is_some()
            || self.menu.take().is_some()
            || std::mem::take(&mut self.save_menu)
            || std::mem::take(&mut self.copy_menu)
            || std::mem::take(&mut self.heading_menu)
        {
            true
        } else if self.naming.take().is_some() || self.custom.take().is_some() {
            self.pending_focus = true;
            true
        } else {
            false
        };
        if dismissed {
            cx.notify();
        }
        dismissed
    }

    fn copy_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copy_menu = false;
        self.copied = true;
        cx.notify();
        // Store the timer task so rapid copies cancel any previous reset timer.
        self.copied_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                this.copied_reset = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn has_manual_edits(&self, cx: &App) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.read(cx).text() != self.editor_initial)
    }

    fn edited_markdown(&self, cx: &App) -> Option<String> {
        let editor = self.editor.as_ref()?;
        let text = editor.read(cx).text();
        (text != self.editor_initial).then(|| text.to_owned())
    }

    fn apply_options(&mut self, options: SummaryOptions, cx: &mut Context<Self>) {
        self.options = options;
        if let Err(err) = self.store.save_summary_options(&self.options) {
            eprintln!("failed to save the summary options: {err:#}");
        }
        self.copied = false;
        self.copied_reset = None;
        // A report made with other options replaces the edited one.
        self.editor = None;
        self.editor_initial.clear();
        self.editing = false;
        self.pending_options = None;
        self.invalidate_cache(cx);
    }

    fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut SummaryOptions)) {
        let mut next = self.options.clone();
        change(&mut next);
        if next == self.options {
            return;
        }
        if self.has_manual_edits(cx) {
            self.pending_options = Some(next);
            self.menu = None;
            self.more_side = None;
            cx.notify();
            return;
        }
        self.apply_options(next, cx);
    }

    /// The pencil in the report's header: edit the report as text, or show it again.
    fn toggle_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing {
            self.editing = false;
            // Drop an untouched editor snapshot so re-entering edit mode after data/options
            // changes rebuilds from the fresh cached report instead of showing stale text.
            if !self.has_manual_edits(cx) {
                self.editor = None;
                self.editor_initial.clear();
            }
        } else {
            if self.editor.is_none() {
                let markdown = self.ensure_cache(cx).report.markdown();
                self.editor_initial = markdown.clone();
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
        self.copied_reset = None;
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
                None => markdown::insert(input.text(), selection, &calendar::format_time(Local::now().time(), i18n::t(cx))),
            };
            input.edit(text, selection, window, cx);
        });
        window.focus(&editor.focus_handle(cx), cx);
        self.copied = false;
        self.copied_reset = None;
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
            self.cache_dirty = true;
        }
        if menu == "habits" {
            self.options.habits = true;
            self.cache_dirty = true;
        }
        if menu == "more-tag" {
            self.menu = Some(Menu::More);
            self.more_side = Some(0);
        }
        if menu == "save" {
            self.save_menu = true;
        }
        // `summary=image:<path>`: what "Save as → Image" writes, to a given file (off the UI thread).
        if let Some(path) = menu.strip_prefix("image:") {
            let path = path.to_owned();
            let (export, _, _) = self.export_snapshot(cx);
            let fonts = crate::report_image::load_report_fonts(cx);
            let task = cx.background_executor().spawn(async move {
                let fonts = fonts.map_err(|err| err.to_string())?;
                let png = crate::report_image::png_with_fonts(&export.heading, &export.groups, fonts)
                    .map_err(|err| err.to_string())?;
                std::fs::write(&path, png).map_err(|err| err.to_string())
            });
            cx.spawn(async move |this, cx| {
                if let Err(err) = task.await {
                    eprintln!("failed to export summary image: {err}");
                    this.update(cx, |this, cx| {
                        this.saved = Some(err);
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        if let Some(name) = menu.strip_prefix("template:") {
            self.store.save_summary_template(name, &self.options).ok();
            self.refresh_templates(cx);
        }
        if menu == "template-new" {
            self.start_template(window, cx);
        }
        cx.notify();
    }

    fn ensure_cache(&mut self, cx: &mut Context<Self>) -> &CachedSummary {
        self.ensure_observers(cx);
        let today = Local::now().date_naive();
        let locale = cx.try_global::<i18n::CurrentLocale>().map_or(i18n::Locale::En, |l| l.0);
        let needs_rebuild = self.cache_dirty
            || self
                .cache
                .as_ref()
                .is_none_or(|c| c.today != today || c.locale != locale);
        if needs_rebuild {
            let strings = i18n::t(cx);
            let tasks = self.store.tasks().unwrap_or_default();
            let lists = self.store.lists().unwrap_or_default();
            let tags = self.store.tags().unwrap_or_default();
            let templates = self.store.summary_templates().unwrap_or_default();
            let groups = summarize(&tasks, &self.options, today, calendar::week_start());
            let mut report = report(&groups, &self.options, &lists, today, strings);
            let sessions = if self.options.fields.contains(&Field::Focus) {
                self.store.focus_sessions().unwrap_or_default()
            } else {
                Vec::new()
            };
            summary_fields::decorate(&mut report, &groups, &self.options, &tasks, &sessions);
            // The Lists filter's "Habit": each habit's check-ins in the period.
            if self.options.habits {
                let habits = self.store.habits().unwrap_or_default();
                let checkins = self.store.all_habit_done_dates().unwrap_or_default();
                let rows = fern_core::summary::habit_checkins(&habits, &checkins, &self.options, today, calendar::week_start());
                if !rows.is_empty() {
                    let lines = rows
                        .into_iter()
                        .map(|(habit, days)| strings.summary_habit_line.replace("{name}", &habit.name).replace("{n}", &days.to_string()))
                        .collect();
                    report.groups.push((strings.summary_habits.to_owned(), lines));
                }
            }
            // Keep an untouched editor in sync with rebuilt data (never overwrite manual edits).
            if let Some(editor) = self.editor.clone()
                && !self.has_manual_edits(cx)
            {
                if self.editing {
                    let markdown = report.markdown();
                    self.editor_initial = markdown.clone();
                    editor.update(cx, |input, cx| input.set_text(markdown, cx));
                } else {
                    self.editor = None;
                    self.editor_initial.clear();
                }
            }
            self.cache = Some(CachedSummary { report, lists, tags, templates, today, locale });
            self.cache_dirty = false;
            #[cfg(test)]
            {
                self.rebuild_count += 1;
            }
        }
        self.cache.as_ref().expect("summary cache populated")
    }

    /// Builds the exportable `(Report, markdown, mailto)` from the cached report and any manual
    /// editor modifications, ensuring PNG, PDF, Copy, and Email always export the same content.
    fn export_snapshot(&mut self, cx: &mut Context<Self>) -> (Report, String, String) {
        let strings = i18n::t(cx);
        let edited = self.edited_markdown(cx);
        let report = &self.ensure_cache(cx).report;
        let subject = format!("{} {}", strings.summary, report.heading);
        let markdown = edited.clone().unwrap_or_else(|| report.markdown());
        let mailto = match &edited {
            Some(text) => format!("mailto:?subject={}&body={}", percent_encode(&subject), percent_encode(text)),
            None => report.mailto(&subject),
        };
        let export = summary_fields::export_report(subject, report.groups.clone(), edited.as_deref());
        (export, markdown, mailto)
    }
}

impl Render for SummaryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.pending_focus) {
            window.focus(&self.focus_handle, cx);
        }
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        self.ensure_cache(cx);
        let report = self.cache.as_ref().expect("summary cache populated").report.clone();
        let empty = report.groups.is_empty();
        let edited = self.edited_markdown(cx);
        let has_manual_edits = edited.is_some();

        let editor = self.editor.clone().filter(|_| self.editing);
        let body = v_flex()
            .id("summary-report")
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
            .when_some(edited.filter(|_| editor.is_none()), |el, text| {
                el.child(crate::task_detail::render_markdown(&text, None, cx))
            })
            .when(editor.is_none() && !has_manual_edits, |el| {
                el.child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).pb_1().child(report.heading.clone()))
            })
            .when(editor.is_none() && !has_manual_edits && empty, |el| {
                el.child(Label::new(strings.summary_empty).color(Color::Muted))
            })
            .children(report.groups.into_iter().filter(|_| editor.is_none() && !has_manual_edits).map(|(title, lines)| {
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
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .items_start()
            .on_action(cx.listener(|this, _: &md::DismissTransient, window, cx| {
                if !this.dismiss_transient(cx) {
                    cx.propagate();
                } else if std::mem::take(&mut this.pending_focus) {
                    window.focus(&this.focus_handle, cx);
                }
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .mt(crate::title_bar::header_inset())
                            .h(px(52.))
                            .px_4()
                            .gap_2()
                            .child(div().flex_1().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(strings.summary))
                            .when(has_manual_edits, |el| {
                                el.child(
                                    div()
                                        .when(cfg!(test), |el| el.debug_selector(|| "summary-reset-edits".to_owned()))
                                        .child(
                                            Button::new("summary-reset-edits", strings.habit_reset)
                                                .variant(ButtonVariant::Ghost)
                                                .size(ButtonSize::Sm)
                                                .icon(IconName::RotateCcw)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.pending_options = Some(this.options.clone());
                                                    window.focus(&this.focus_handle, cx);
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            }),
                    )
                    // The original's report: its toolbar always on top, in a frame with the report.
                    .child(
                        v_flex()
                            .flex_1()
                            .min_h_0()
                            .mx_4()
                            .rounded(px(8.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(colors.border_variant)
                            .child(self.render_toolbar(cx))
                            .child(v_scroll("summary-report-scroll", body).flex_1().min_h_0()),
                    )
                    .child(
                        h_flex().h(px(44.)).px_4().gap_5().justify_end()
                        .children(self.saved.clone().map(|message| {
                            div().flex_1().min_w_0().child(Label::new(message).size(LabelSize::XSmall).color(Color::Muted).truncate())
                        })).child({
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
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_menu = false;
                                                    let (export, _, _) = this.export_snapshot(cx);
                                                    let html = crate::print::report_html(&export.heading, &export.groups);
                                                    if let Err(err) = crate::print::open("summary", &html) {
                                                        eprintln!("failed to open the summary for printing: {err:#}");
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            ListItem::new("save-image")
                                                .start_slot(Icon::new(IconName::Image).color(Color::Warning))
                                                .child(Label::new(strings.summary_image))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_menu = false;
                                                    let (export, _, _) = this.export_snapshot(cx);
                                                    this.save_png(export.heading, export.groups, cx);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            ListItem::new("save-email")
                                                .start_slot(Icon::new(IconName::Mail).color(Color::Accent))
                                                .child(Label::new(strings.summary_send_email))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_menu = false;
                                                    let (_, _, mail) = this.export_snapshot(cx);
                                                    cx.open_url(&mail);
                                                    cx.notify();
                                                })),
                                        ),
                                )
                            });
                            h_flex()
                                .id("summary-save-as")
                                .relative()
                                .px_2()
                                .py_1()
                                .gap_1()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.ghost_element_hover))
                                .child(Icon::new(IconName::Share).size(IconSize::Small).color(Color::Muted))
                                .child(Label::new(strings.summary_save_as))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.copy_menu = false;
                                    this.save_menu = !this.save_menu;
                                    cx.notify();
                                }))
                                .children(menu)
                        }).child({
                            let copy_popover = self.copy_menu.then(|| {
                                PopoverAttachment::new(
                                    Placement::AboveRight,
                                    popover(cx)
                                        .min_w(px(180.))
                                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                            this.copy_menu = false;
                                            cx.notify();
                                        }))
                                        .child(
                                            ListItem::new("summary-copy-markdown")
                                                .start_slot(Icon::new(IconName::FileText).color(Color::Muted))
                                                .child(Label::new(strings.copy_markdown))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let (_, md_text, _) = this.export_snapshot(cx);
                                                    this.copy_text(md_text, cx);
                                                })),
                                        )
                                        .child(
                                            ListItem::new("summary-copy-plain")
                                                .start_slot(Icon::new(IconName::Copy).color(Color::Muted))
                                                .child(Label::new(strings.copy_plain_text))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let (_, md_text, _) = this.export_snapshot(cx);
                                                    this.copy_text(markdown::plain_text(&md_text), cx);
                                                })),
                                        ),
                                )
                            });
                            h_flex()
                                .id("summary-copy")
                                .relative()
                                .px_2()
                                .py_1()
                                .gap_1()
                                .rounded(px(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.ghost_element_hover))
                                .child(
                                    Icon::new(if self.copied { IconName::Check } else { IconName::Copy })
                                        .size(IconSize::Small)
                                        .color(if self.copied { Color::Accent } else { Color::Muted }),
                                )
                                .child(
                                    Label::new(if self.copied { strings.summary_copied } else { strings.summary_copy })
                                        .color(if self.copied { Color::Accent } else { Color::Default }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_menu = false;
                                    this.copy_menu = !this.copy_menu;
                                    cx.notify();
                                }))
                                .children(copy_popover)
                        }),
                    ),
            )
            .child(self.render_options(cx))
            .when_some(self.pending_options.clone(), |el, next| {
                el.child(
                    ConfirmDialog::new("summary-overwrite", strings.summary_overwrite_title)
                        .width(px(360.))
                        .description(strings.summary_overwrite_message)
                        .cancel_label(strings.cancel)
                        .confirm_label(strings.confirm)
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.pending_options = None;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(move |this, _, _, cx| {
                            this.apply_options(next.clone(), cx);
                        })),
                )
            })
            .when_some(self.confirm_delete_template.clone(), |el, (id, name)| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .when(cfg!(test), |el| el.debug_selector(|| "summary-template-delete-modal".to_owned()))
                        .child(
                            ConfirmDialog::new("summary-template-delete", strings.confirm_delete_summary_template)
                                .width(px(340.))
                                .description(name)
                                .cancel_label(strings.cancel)
                                .confirm_label(strings.delete)
                                .destructive(true)
                                .on_cancel(cx.listener(|this, _, _, cx| {
                                    this.confirm_delete_template = None;
                                    cx.notify();
                                }))
                                .on_confirm(cx.listener(move |this, _, _, cx| {
                                    this.confirm_delete_template = None;
                                    if let Err(err) = this.store.delete_summary_template(id) {
                                        eprintln!("failed to delete the summary template: {err:#}");
                                    }
                                    this.refresh_templates(cx);
                                    desktop_common::data_version::data_changed(cx.entity_id(), cx);
                                })),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests;
