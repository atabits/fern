//! "Print": a list or a task as a plain HTML page, opened in the browser to print from there.

use chrono::NaiveDate;
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
            body.push_str(&format!("<li>{}</li>", escape(line)));
        }
        body.push_str("</ul>");
    }
    page(heading, &body)
}

fn page(title: &str, body: &str) -> String {
    let title = escape(title);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title><style>{STYLE}</style></head>\
         <body><h1>{title}</h1>{body}<script>window.print()</script></body></html>"
    )
}

/// Writes the page to a temporary file and opens it in the default browser.
pub fn open(name: &str, html: &str) -> std::io::Result<()> {
    let path = std::env::temp_dir().join(format!("fern-print-{name}.html"));
    std::fs::write(&path, html)?;
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener).arg(&path).spawn().map(|_| ())
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
}
