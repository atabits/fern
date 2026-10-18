//! The Summary smart list, modeled on the original's: a report of the period's tasks on the left,
//! its Filter and Display Options on the right, and "Copy" to take it as Markdown.

use std::rc::Rc;

use chrono::{Local, NaiveDate};
use fern_core::summary::{Field, GroupKey, Grouping, Period, StatusFilter, SummaryGroup, SummaryOptions, summarize};
use fern_core::{List, Priority, Store, Task};
use gpui::{ClipboardItem, FontWeight};
use ui::prelude::*;
use ui::{Placement, PopoverAttachment, popover};

use crate::calendar;
use crate::i18n::{self, Strings};
use crate::task_rows::{local_day, short_date};

/// The open dropdown, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Period,
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
}

/// A rendered summary: the period's heading and each group's heading and lines.
pub struct Report {
    pub heading: String,
    pub groups: Vec<(String, Vec<String>)>,
}

impl Report {
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

pub fn period_label(period: Period, today: NaiveDate, strings: &Strings) -> String {
    let index = Period::ALL.iter().position(|p| *p == period).unwrap_or(0);
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
        Self { store, options: SummaryOptions::default(), menu: None, copied: false }
    }

    fn set_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        self.menu = menu;
        cx.notify();
    }

    fn update(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut SummaryOptions)) {
        change(&mut self.options);
        self.copied = false;
        cx.notify();
    }

    /// Opens a dropdown from the debug-build dev state: `summary=<period|status|grouping|fields>`.
    #[cfg(debug_assertions)]
    pub fn dev_open(&mut self, menu: &str, cx: &mut Context<Self>) {
        self.menu = match menu {
            "period" => Some(Menu::Period),
            "status" => Some(Menu::Status),
            "grouping" => Some(Menu::Grouping),
            "fields" => Some(Menu::Fields),
            _ => None,
        };
        cx.notify();
    }

    fn current_report(&self, cx: &App) -> Report {
        let strings = i18n::t(cx);
        let today = Local::now().date_naive();
        let tasks = self.store.tasks().unwrap_or_default();
        let lists = self.store.lists().unwrap_or_default();
        let groups = summarize(&tasks, &self.options, today, calendar::week_start());
        report(&groups, &self.options, &lists, today, strings)
    }

    /// A Filter or Display Options row whose value opens a dropdown.
    fn row(
        &self,
        label: &'static str,
        value: String,
        menu: Menu,
        options: Vec<(String, bool, Box<dyn Fn(&mut Self, &mut Context<Self>)>)>,
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (options[i].2)(this, cx);
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
            .h(px(32.))
            .px_3()
            .child(div().flex_1().child(Label::new(label).size(LabelSize::Small)))
            .child(
                h_flex()
                    .id(SharedString::from(format!("summary-{label}")))
                    .relative()
                    .gap_1()
                    .cursor_pointer()
                    .child(Label::new(value).size(LabelSize::Small).color(Color::Muted))
                    .child(Icon::new(IconName::ChevronsUpDown).size(IconSize::XSmall).color(Color::Muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_menu((!open).then_some(menu), cx)))
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

        let periods = Period::ALL
            .into_iter()
            .map(|period| {
                let pick: Box<dyn Fn(&mut Self, &mut Context<Self>)> = Box::new(move |this, cx| this.update(cx, |o| o.period = period));
                (period_label(period, today, strings), period == options.period, pick)
            })
            .collect();
        let statuses = StatusFilter::ALL
            .into_iter()
            .zip(strings.summary_statuses)
            .map(|(status, name)| {
                let pick: Box<dyn Fn(&mut Self, &mut Context<Self>)> = Box::new(move |this, cx| this.update(cx, |o| o.status = status));
                (name.to_owned(), status == options.status, pick)
            })
            .collect();
        let groupings = Grouping::ALL
            .into_iter()
            .zip(strings.summary_groupings)
            .map(|(grouping, name)| {
                let pick: Box<dyn Fn(&mut Self, &mut Context<Self>)> = Box::new(move |this, cx| this.update(cx, |o| o.grouping = grouping));
                (name.to_owned(), grouping == options.grouping, pick)
            })
            .collect();
        let fields = Field::ALL
            .into_iter()
            .zip(strings.summary_fields)
            .map(|(field, name)| {
                let pick: Box<dyn Fn(&mut Self, &mut Context<Self>)> = Box::new(move |this, cx| {
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
            .child(heading(strings.summary_filter))
            .child(
                group()
                    .child(self.row(strings.summary_date, period_label(options.period, today, strings), Menu::Period, periods, false, cx))
                    .child(self.row(strings.summary_status, strings.summary_statuses[status_index].to_owned(), Menu::Status, statuses, false, cx)),
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
    }
}

impl Render for SummaryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strings = i18n::t(cx);
        let colors = cx.theme().colors().clone();
        let report = self.current_report(cx);
        let empty = report.groups.is_empty();
        let markdown = report.markdown();

        let body = v_flex()
            .id("summary-report")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .gap_1()
            .child(div().text_size(px(18.)).font_weight(FontWeight::BOLD).pb_1().child(report.heading))
            .when(empty, |el| el.child(Label::new(strings.summary_empty).color(Color::Muted)))
            .children(report.groups.into_iter().map(|(title, lines)| {
                v_flex()
                    .pt_1()
                    .child(div().pb_1().font_weight(FontWeight::BOLD).child(title))
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
                            .child(Icon::new(IconName::FileText).color(Color::Muted))
                            .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).child(strings.summary)),
                    )
                    .child(body)
                    .child(
                        h_flex().h(px(44.)).px_4().justify_end().border_t_1().border_color(colors.border_variant).child(
                            h_flex()
                                .id("summary-copy")
                                .gap_1()
                                .cursor_pointer()
                                .child(Icon::new(IconName::Copy).size(IconSize::Small).color(Color::Muted))
                                .child(Label::new(if self.copied { strings.summary_copied } else { strings.summary_copy }).size(LabelSize::Small))
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
    }
}
