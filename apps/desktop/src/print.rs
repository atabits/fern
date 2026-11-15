//! "Print": a list or a task as a plain HTML page, opened in the browser to print from there.

use chrono::{Datelike, NaiveDate};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use fern_core::{ChecklistItem, Task};

use crate::i18n::Strings;
use crate::task_rows;

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const STYLE: &str = "body{font-family:system-ui,sans-serif;max-width:720px;margin:32px auto;color:#222}\
h1{font-size:22px}ul{list-style:none;padding:0}li{padding:6px 0;border-bottom:1px solid #eee}\
.box{display:inline-block;width:1.2em}.meta{color:#888;font-size:12px;margin-left:8px}\
.done{color:#999;text-decoration:line-through}.notes{white-space:pre-wrap;color:#444}\
.sub{margin-left:1.6em}";

fn task_line(task: &Task, today: NaiveDate, strings: &Strings, nested: bool) -> String {
    let done = task.is_completed();
    let mut meta = Vec::new();
    if let Some(date) = task.due_date {
        meta.push(task_rows::due_label(date, task.due_time, today, strings).0);
    }
    meta.extend(task.tags.iter().map(|t| format!("#{t}")));
    format!(
        "<li{}><span class=\"box\">{}</span><span{}>{}</span>{}</li>",
        if nested { " class=\"sub\"" } else { "" },
        if done { "☑" } else { "☐" },
        if done { " class=\"done\"" } else { "" },
        escape(if task.title.is_empty() { strings.untitled } else { &task.title }),
        if meta.is_empty() { String::new() } else { format!("<span class=\"meta\">{}</span>", escape(&meta.join(" · "))) },
    )
}

/// A list page: its title and tasks, subtasks indented under their parents.
pub fn list_html(title: &str, tasks: &[Task], today: NaiveDate, strings: &Strings) -> String {
    let ids: std::collections::HashSet<_> = tasks.iter().map(|t| t.id).collect();
    let mut body = String::new();
    for task in tasks.iter().filter(|t| t.parent_id.is_none_or(|p| !ids.contains(&p))) {
        body.push_str(&task_line(task, today, strings, false));
        for sub in tasks.iter().filter(|t| t.parent_id == Some(task.id)) {
            body.push_str(&task_line(sub, today, strings, true));
        }
    }
    page(title, &format!("<ul>{body}</ul>"))
}

/// A task page: title, date and tags, notes and checklist.
pub fn task_html(task: &Task, checklist: &[ChecklistItem], today: NaiveDate, strings: &Strings) -> String {
    let mut body = format!("<ul>{}</ul>", task_line(task, today, strings, false));
    if !task.notes.trim().is_empty() {
        body.push_str(&format!("<p class=\"notes\">{}</p>", escape(&task.notes)));
    }
    if !checklist.is_empty() {
        body.push_str("<ul>");
        for item in checklist {
            body.push_str(&format!(
                "<li><span class=\"box\">{}</span><span{}>{}</span></li>",
                if item.completed { "☑" } else { "☐" },
                if item.completed { " class=\"done\"" } else { "" },
                escape(&item.title)
            ));
        }
        body.push_str("</ul>");
    }
    page(&task.title, &body)
}

/// The Summary's report: a section per group with its lines, for "Save as → PDF".
pub fn report_html(heading: &str, groups: &[(String, Vec<String>)]) -> String {
    let mut body = String::new();
    for (title, lines) in groups {
        body.push_str(&format!("<h2>{}</h2><ul>", escape(title)));
        for line in lines {
            body.push_str(&format!("<li class=\"notes\">{}</li>", escape(line)));
        }
        body.push_str("</ul>");
    }
    page(heading, &body)
}

/// The Summary's report as an SVG picture, for "Save as → Image": the heading, then each group's
/// title and lines, dark text on white.
pub fn report_svg(heading: &str, groups: &[(String, Vec<String>)]) -> String {
    const WIDTH: usize = 720;
    const LINE: usize = 26;
    let mut lines: Vec<(usize, &str, &str)> = vec![(40, "22", heading)];
    for (title, items) in groups {
        lines.push((LINE + 10, "16", title));
        lines.extend(items.iter().map(|item| (LINE, "14", item.as_str())));
    }
    let mut y = 0;
    let mut body = String::new();
    for (gap, size, text) in &lines {
        y += gap;
        let weight = if *size == "14" { "normal" } else { "bold" };
        let x = if *size == "14" { 44 } else { 32 };
        if *size == "14" {
            body.push_str(&format!("<circle cx=\"34\" cy=\"{}\" r=\"2.5\" fill=\"#4772fa\"/>", y - 5));
        }
        body.push_str(&format!(
            "<text x=\"{x}\" y=\"{y}\" font-size=\"{size}\" font-weight=\"{weight}\">{}</text>",
            escape(text)
        ));
    }
    let height = y + 32;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{height}\" viewBox=\"0 0 {WIDTH} {height}\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\
         <g font-family=\"system-ui, sans-serif\" fill=\"#222222\">{body}</g></svg>"
    )
}

fn page(title: &str, body: &str) -> String {
    let title = escape(title);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title><style>{STYLE}</style></head>\
         <body><h1>{title}</h1>{body}<script>window.print()</script></body></html>"
    )
}

/// A complete year on one landscape page, using the current week and heatmap settings.
pub fn year_html(
    year: i32,
    today: NaiveDate,
    counts: &BTreeMap<NaiveDate, usize>,
    week_numbers: bool,
    strings: &Strings,
) -> String {
    use crate::calendar;
    let mut months = String::new();
    for month in 1..=12 {
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
        let _ = write!(months, "<section><h2>{}</h2><table><thead><tr>", escape(strings.months[month as usize - 1]));
        if week_numbers {
            months.push_str("<th></th>");
        }
        for label in calendar::weekday_labels(&strings.weekdays_year) {
            let _ = write!(months, "<th>{}</th>", escape(label));
        }
        months.push_str("</tr></thead><tbody>");
        for week in calendar::month_grid(first).chunks_exact(7) {
            months.push_str("<tr>");
            if week_numbers {
                let _ = write!(months, "<th>{}</th>", calendar::week_number(week.try_into().unwrap()));
            }
            for date in week {
                let class = if date.month() != month {
                    "outside".to_owned()
                } else if *date == today {
                    "today".to_owned()
                } else {
                    format!("heat-{}", calendar::heatmap_level(counts.get(date).copied().unwrap_or(0)))
                };
                let _ = write!(months, "<td class=\"{class}\">{}</td>", date.day());
            }
            months.push_str("</tr>");
        }
        months.push_str("</tbody></table></section>");
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{year}</title><style>\
        @page{{size:landscape;margin:10mm}}\
        body{{font:11pt system-ui,sans-serif;color:#222;print-color-adjust:exact;margin:0}}\
        h1{{font-size:19pt;margin:0 0 3mm}}h2{{font-size:12pt;margin:0 0 1mm}}\
        main{{display:grid;grid-template-columns:repeat(4,1fr);gap:3mm 5mm}}section{{break-inside:avoid}}\
        table{{border-collapse:collapse;table-layout:fixed;width:100%;text-align:center;font-size:9pt}}\
        td,th{{padding:0;height:4.5mm}}th,.outside{{color:#666;font-weight:normal}}\
        .today{{background:#4772fa;color:#fff}}.heat-1{{background:#e3ebf9}}.heat-2{{background:#ceddfa}}\
        .heat-3{{background:#b7ccfa}}.heat-4{{background:#a0bbf8}}</style></head>\
        <body><h1>{year}</h1><main>{months}</main><script>window.print()</script></body></html>"
    )
}

/// Writes the page to a temporary file and opens it in the default browser.
pub fn open(name: &str, html: &str) -> std::io::Result<()> {
    let path = crate::scratch_dir()?.join(format!("fern-print-{name}.html"));
    std::fs::write(&path, html)?;
    open_print_file(&path)
}

#[cfg(windows)]
fn open_print_file(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    use windows::core::{PCWSTR, w};

    // Pass the path directly so the executable's folder can contain shell metacharacters.
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let result = unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    let status = result.0 as isize;
    if status <= 32 {
        return Err(std::io::Error::other(format!("could not open the print page (ShellExecuteW: {status})")));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_print_file(path: &std::path::Path) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(program).arg(path).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::EN;

    #[test]
    fn pages() {
        let store = fern_core::Store::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 18).unwrap();
        let parent = store.add_task("Fix <the> tap").unwrap();
        let child = store.add_subtask(parent.id, "buy a washer").unwrap();
        store.set_completed(child.id, true).unwrap();
        store.set_notes(parent.id, "Kitchen & bath").unwrap();
        store.add_checklist_item(parent.id, "turn off water", None).unwrap();
        let tasks = store.tasks().unwrap();

        let list = list_html("Home", &tasks, today, &EN);
        assert!(list.contains("<h1>Home</h1>") && list.contains("Fix &lt;the&gt; tap"));
        assert!(list.contains("class=\"sub\"><span class=\"box\">☑</span><span class=\"done\">buy a washer"));

        let parent = store.task(parent.id).unwrap().unwrap();
        let page = task_html(&parent, &store.checklist(parent.id).unwrap(), today, &EN);
        assert!(page.contains("Kitchen &amp; bath") && page.contains("☐</span><span>turn off water"));
    }

    #[test]
    fn report_picture() {
        let svg = report_svg("Oct 12 - Oct 18", &[("Completed".into(), vec!["Fix <the> tap".into()])]);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("Fix &lt;the&gt; tap") && svg.contains(">Completed<"));
    }

    #[test]
    fn prints_twelve_months_and_the_heatmap() {
        let today = NaiveDate::from_ymd_opt(2032, 2, 29).unwrap();
        let busy = today.succ_opt().unwrap();
        let html = year_html(2032, today, &BTreeMap::from([(busy, 7)]), true, &EN);
        assert_eq!(html.matches("<section>").count(), 12);
        assert_eq!(html.matches("<td class=\"today\">29</td>").count(), 1);
        assert_eq!(html.matches("<td class=\"heat-3\">1</td>").count(), 1);
        assert_eq!(html.matches("<th></th>").count(), 12);
    }
}
