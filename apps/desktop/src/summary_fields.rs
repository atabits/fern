//! Additional fields of Summary, using the same task/session snapshots as the report.
use std::collections::HashMap;

use fern_core::{FocusSession, Task, format_minutes};
use fern_core::summary::{Field, SummaryGroup, SummaryOptions};
use super::Report;

impl super::SummaryView {
    pub(super) fn save_png(&mut self, heading: String, groups: Vec<(String, Vec<String>)>, cx: &mut gpui::Context<Self>) {
        let generate = cx.background_executor().spawn(async move { crate::report_image::png(&heading, &groups) });
        cx.spawn(async move |this, cx| {
            let result = generate.await;
            this.update(cx, |this, cx| match result {
                Ok(bytes) => {
                    let name = format!("fern-summary-{}.png", chrono::Local::now().format("%Y-%m-%d"));
                    crate::export::save_as(name, bytes, cx, |this, result, cx| {
                        if let Some(message) = crate::export::outcome(result, crate::i18n::t(cx)) { this.saved = Some(message); }
                        cx.notify();
                    });
                }
                Err(error) => { this.saved = Some(error.to_string()); cx.notify(); }
            }).ok();
        }).detach();
    }
}

pub(super) fn decorate(report: &mut Report, groups: &[SummaryGroup], options: &SummaryOptions, tasks: &[Task], sessions: &[FocusSession]) {
    let mut focus_by_task = HashMap::new();
    for session in sessions {
        if let Some(id) = session.task_id {
            *focus_by_task.entry(id).or_insert(0) += (session.ended_at - session.started_at).max(0);
        }
    }
    for ((_, lines), group) in report.groups.iter_mut().zip(groups) {
        for (line, task) in lines.iter_mut().zip(&group.tasks) {
            if options.fields.contains(&Field::Progress) {
                let progress = task.checklist.filter(|(_, total)| *total > 0)
                    .map_or(if task.is_completed() { 100 } else { 0 }, |(done, total)| done * 100 / total);
                line.push_str(&format!(" · {progress}%"));
            }
            if options.fields.contains(&Field::Parent)
                && let Some(parent) = task.parent_id.and_then(|id| tasks.iter().find(|t| t.id == id)) {
                line.push_str(&format!(" · {}", parent.title));
            }
            if options.fields.contains(&Field::Focus) {
                let millis = focus_by_task.get(&task.id).copied().unwrap_or_default();
                line.push_str(&format!(" · {}", format_minutes(millis / 60_000)));
            }
            if options.fields.contains(&Field::Detail) && !task.notes.trim().is_empty() {
                for detail in task.notes.lines() { line.push_str(&format!("\n  {detail}")); }
            }
        }
    }
}

/// Preserve an edited document for every export. PDF and image must use the same text as Copy.
pub(super) fn export_report(heading: String, generated: Vec<(String, Vec<String>)>, edited: Option<&str>) -> Report {
    match edited {
        None => Report { heading, groups: generated },
        Some(text) => Report { heading, groups: vec![(String::new(), text.lines().map(str::to_owned).collect())] },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fern_core::{Store, summary::{GroupKey, SummaryOptions}};
    #[test]
    fn complete_fields_and_edited_exports() {
        let store = Store::open_in_memory().unwrap();
        let parent = store.add_task("Parent").unwrap();
        let child = store.add_subtask(parent.id, "Child").unwrap();
        store.set_notes(child.id, "Notes\nMore").unwrap();
        let item = store.add_checklist_item(child.id, "First", None).unwrap();
        store.add_checklist_item(child.id, "Second", None).unwrap();
        store.set_checklist_item_completed(item.id, true).unwrap();
        store.add_focus_session_as(Some(child.id), "Child", 0, 25 * 60_000, false).unwrap();
        let task = store.task(child.id).unwrap().unwrap();
        let groups = vec![SummaryGroup { key: GroupKey::List(None), tasks: vec![task] }];
        let mut report = Report { heading: "Week".into(), groups: vec![("Inbox".into(), vec!["Child".into()])] };
        let options = SummaryOptions { fields: vec![Field::Progress, Field::Parent, Field::Focus, Field::Detail], ..Default::default() };
        decorate(&mut report, &groups, &options, &store.tasks().unwrap(), &store.focus_sessions().unwrap());
        let text = &report.groups[0].1[0];
        assert!(text.contains("50%") && text.contains("Parent") && text.contains("25m") && text.contains("\n  More"));
        let export = export_report(report.heading, report.groups, Some("Edited <report>\nSecond line"));
        assert_eq!(export.groups[0].1, ["Edited <report>", "Second line"]);
    }
}
