//! The same detail editor operates on an unsaved task before committing it to Store.
use super::*;
use fern_core::{ParsedTask, Recognition, TaskDraft, parse_quick_add_on_day, parse_quick_add_with};

pub(super) struct NewTask {
    pub checklist: Vec<ChecklistItem>,
    pub attachments: Vec<PreparedAttachment>,
    pub pending_attachments: usize,
    pub save_failed: bool,
    pub finished: bool,
    pub recognize_date: bool,
    pub date_changed: bool,
    pub reminders_changed: bool,
    pub kind_changed: bool,
    timed_reminder: Option<i64>,
    all_day_reminder: Option<i64>,
    recognition: Recognition,
    parsed_title: ParsedTask,
    pub priority_changed: bool,
    pub list_changed: bool,
}

impl NewTask {
    fn parse_title(&self, task: &Task, text: &str, lists: &[List], today: NaiveDate) -> ParsedTask {
        let names: Vec<_> = lists.iter().map(|list| list.name.clone()).collect();
        let mut recognition = self.recognition;
        recognition.dates &= !self.date_changed;
        if self.recognize_date {
            parse_quick_add_with(text, today, &names, recognition)
        } else {
            parse_quick_add_on_day(text, task.due_date.unwrap_or(today), &names, recognition)
        }
    }

    pub fn default_reminder(&self, timed: bool) -> Option<i64> {
        if timed {
            self.timed_reminder
        } else {
            self.all_day_reminder
        }
    }

    fn due(&self, task: &Task, parsed: &ParsedTask, today: NaiveDate) -> DueDraft {
        let mut due = DueDraft::from_task(task, today);
        if !self.date_changed {
            due.date = parsed
                .due_date
                .filter(|_| parsed.explicit_date)
                .or(task.due_date);
            due.time = parsed.due_time.or(task.due_time);
            due.repeat = parsed.repeat.clone().or_else(|| task.repeat.clone());
            due.month = calendar::month_start(due.date.unwrap_or(today));
        }
        if due.date.is_none() {
            due.reminders.clear();
        } else if !self.reminders_changed {
            due.reminders = self
                .default_reminder(due.time.is_some())
                .into_iter()
                .collect();
        }
        due.reminders_changed = self.reminders_changed;
        due
    }

    fn list_id(&self, task: &Task, parsed: &ParsedTask, lists: &[List]) -> Option<Id> {
        if !self.list_changed
            && let Some(name) = parsed.list.as_ref()
            && let Some(list) = lists.iter().find(|list| &list.name == name)
        {
            return Some(list.id);
        }
        task.list_id
    }

    fn kind(&self, task: &Task, list_id: Option<Id>, lists: &[List]) -> Kind {
        if self.kind_changed {
            return task.kind;
        }
        lists
            .iter()
            .find(|list| Some(list.id) == list_id)
            .map_or(Kind::Task, |list| list.kind)
    }
}

/// `None` means an empty draft was closed; otherwise the task was committed once.
pub struct DraftClosed(pub Option<Id>);
impl EventEmitter<DraftClosed> for TaskDetail {}

impl Focusable for TaskDetail {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.title.focus_handle(cx)
    }
}

impl TaskDetail {
    pub fn begin_draft(
        &mut self,
        date: NaiveDate,
        list: Option<Id>,
        recognize_date: bool,
        cx: &mut Context<Self>,
    ) {
        let preferences = self.session.store.preferences().unwrap_or_default();
        let defaults = preferences.task_defaults;
        self.session.lists = self.session
            .store
            .lists()
            .unwrap_or_default()
            .into_iter()
            .filter(|list| !list.archived)
            .collect();
        let list = list.and_then(|id| self.session.lists.iter().find(|list| list.id == id));
        let task = Task {
            id: Id::now_v7(),
            list_id: list.map(|list| list.id),
            parent_id: None,
            kind: list.map_or(Kind::Task, |list| list.kind),
            title: String::new(),
            notes: String::new(),
            priority: defaults.priority,
            due_date: Some(date),
            due_time: None,
            repeat: None,
            tags: defaults.tag.into_iter().collect(),
            checklist: None,
            completed_at: None,
            deleted_at: None,
            wont_do_at: None,
            end_date: None,
            end_time: None,
            pinned_at: None,
            section_id: None,
            modified_at: None,
            reminders: defaults.all_day_reminder.into_iter().collect(),
            estimate: None,
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        self.show(Some(task), cx);
        self.session.draft = Some(NewTask {
            checklist: Vec::new(),
            attachments: Vec::new(),
            pending_attachments: 0,
            save_failed: false,
            finished: false,
            recognize_date,
            date_changed: false,
            reminders_changed: false,
            kind_changed: false,
            timed_reminder: defaults.timed_reminder,
            all_day_reminder: defaults.all_day_reminder,
            recognition: preferences.recognition,
            parsed_title: ParsedTask::default(),
            priority_changed: false,
            list_changed: false,
        });
        self.title.update(cx, |input, cx| {
            input.set_placeholder(i18n::t(cx).new_task_prompt, cx)
        });
        cx.notify();
    }

    pub(super) fn refresh_draft_title(&mut self, cx: &mut Context<Self>) {
        let (Some(task), Some(draft)) = (self.session.task.as_ref(), self.session.draft.as_mut()) else {
            return;
        };
        draft.parsed_title = draft.parse_title(
            task,
            self.title.read(cx).text(),
            &self.session.lists,
            Local::now().date_naive(),
        );
        self.title.update(cx, |input, cx| {
            input.set_highlights(draft.parsed_title.spans.clone(), cx)
        });
        cx.notify();
    }

    pub(super) fn due_preview(&self, task: &Task, today: NaiveDate) -> DueDraft {
        match &self.session.draft {
            Some(draft) => draft.due(task, &draft.parsed_title, today),
            None => DueDraft::from_task(task, today),
        }
    }

    pub(super) fn list_preview(&self, task: &Task) -> Option<Id> {
        self.session.draft.as_ref().map_or(task.list_id, |draft| {
            draft.list_id(task, &draft.parsed_title, &self.session.lists)
        })
    }

    pub(super) fn kind_preview(&self, task: &Task) -> Kind {
        self.session.draft.as_ref().map_or(task.kind, |draft| {
            draft.kind(task, self.list_preview(task), &self.session.lists)
        })
    }

    /// Returns true when the edit was applied to the in-memory draft.
    pub(super) fn edit_draft(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Task, &mut NewTask),
    ) -> bool {
        let (Some(task), Some(draft)) = (&mut self.session.task, &mut self.session.draft) else {
            return false;
        };
        if !draft.finished {
            edit(task, draft);
            draft.save_failed = false;
            cx.notify();
        }
        true
    }

    pub fn finish_draft(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(draft) = &self.session.draft else {
            return true;
        };
        if draft.finished {
            return true;
        }
        if draft.pending_attachments > 0 {
            return false;
        }
        let result = self.commit_draft(cx);
        match result {
            Ok(id) => {
                let draft = self.session.draft.as_mut().unwrap();
                draft.finished = true;
                draft.attachments.clear();
                cx.emit(DraftClosed(id));
                true
            }
            Err(error) => {
                eprintln!("failed to create task: {error:#}");
                self.session.draft.as_mut().unwrap().save_failed = true;
                cx.notify();
                false
            }
        }
    }

    fn commit_draft(&mut self, cx: &App) -> anyhow::Result<Option<Id>> {
        let draft = self.session.draft.as_ref().unwrap();
        let task = self.session.task.as_ref().unwrap();
        let text = self.title.read(cx).text().trim();
        let notes = self.notes.read(cx).text();
        let mut checklist: Vec<_> = self
            .checklist
            .iter()
            .map(|row| {
                (
                    row.input.read(cx).text().trim().to_owned(),
                    row.item.completed,
                )
            })
            .filter(|(text, _)| !text.is_empty())
            .collect();
        let pending_item = self.new_item.read(cx).text().trim();
        if self.show_checklist && !pending_item.is_empty() {
            checklist.push((pending_item.to_owned(), false));
        }
        if text.is_empty()
            && notes.trim().is_empty()
            && checklist.is_empty()
            && draft.attachments.is_empty()
        {
            return Ok(None);
        }
        let preferences = self.session.store.preferences()?;
        let lists: Vec<_> = self.session
            .store
            .lists()?
            .into_iter()
            .filter(|list| !list.archived)
            .collect();
        let today = Local::now().date_naive();
        let parsed = draft.parse_title(task, text, &lists, today);
        let due = draft.due(task, &parsed, today);
        let mut list_id = draft.list_id(task, &parsed, &lists);
        let title = if parsed.title.is_empty() {
            text.to_owned()
        } else {
            parsed.title
        };
        if list_id.is_some_and(|id| !lists.iter().any(|list| list.id == id)) {
            list_id = preferences
                .default_list
                .filter(|id| lists.iter().any(|list| list.id == *id));
        }
        let section_id = task.section_id.filter(|id| {
            self.session.store
                .list_sections(list_id)
                .is_ok_and(|sections| sections.iter().any(|section| section.id == *id))
        });
        let mut tags = task.tags.clone();
        for tag in parsed.tags {
            if !tags.iter().any(|existing| same_tag(existing, &tag)) {
                tags.push(tag);
            }
        }
        let fields = TaskDraft {
            title,
            notes: notes.to_owned(),
            list_id,
            kind: draft.kind(task, list_id, &lists),
            priority: if draft.priority_changed {
                task.priority
            } else {
                parsed.priority.unwrap_or(task.priority)
            },
            due_date: due.date,
            due_time: due.time,
            repeat: due.repeat,
            reminders: due.reminders,
            tags,
            end_date: task.end_date,
            end_time: task.end_time,
            section_id,
            checklist,
        };
        let top = !preferences.task_defaults.add_to_bottom;
        let task = if draft.attachments.is_empty() {
            self.session.store.create_task(fields, top)?
        } else {
            let dir = self.session
                .attachments_dir
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("attachment directory unavailable"))?;
            let attachments = &mut self.session.draft.as_mut().unwrap().attachments;
            self.session.store
                .create_task_with_attachments(fields, top, attachments, dir)?
        };
        Ok(Some(task.id))
    }

    pub(super) fn add_checklist_item(
        &mut self,
        text: &str,
        after: Option<Id>,
        cx: &mut Context<Self>,
    ) -> Option<ChecklistItem> {
        let task = self.task_id()?;
        if let Some(draft) = &mut self.session.draft {
            let item = ChecklistItem {
                id: Id::now_v7(),
                task_id: task,
                title: text.to_owned(),
                completed: false,
            };
            let index = after
                .and_then(|id| draft.checklist.iter().position(|item| item.id == id))
                .map_or(draft.checklist.len(), |index| index + 1);
            draft.checklist.insert(index, item.clone());
            draft.save_failed = false;
            cx.notify();
            Some(item)
        } else {
            match self.session.store.add_checklist_item(task, text, after) {
                Ok(item) => {
                    cx.emit(TaskUpdated);
                    Some(item)
                }
                Err(error) => {
                    eprintln!("failed to add checklist item: {error:#}");
                    None
                }
            }
        }
    }

    pub(super) fn render_draft_menu(
        &self,
        strings: &'static Strings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_note = self.session
            .task
            .as_ref()
            .is_some_and(|task| self.kind_preview(task) == Kind::Note);
        let item = |id, icon, text| {
            ListItem::new(id)
                .height(px(34.))
                .start_slot(Icon::new(icon).color(Color::Muted))
                .child(Label::new(text))
        };
        popover(cx)
            .min_w(px(178.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .child(item("draft-tags", IconName::Tag, strings.tags).on_click(
                cx.listener(|this, _, window, cx| this.open_tags(Popup::Tags, window, cx)),
            ))
            .child(
                item(
                    "draft-attachment",
                    IconName::Paperclip,
                    strings.upload_attachment,
                )
                .on_click(cx.listener(|this, _, _, cx| this.upload_attachment(cx))),
            )
            .child(
                item(
                    "draft-template",
                    IconName::LayoutTemplate,
                    strings.add_from_template,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_popup(Popup::Templates, cx))),
            )
            .child(
                item(
                    "draft-convert",
                    if is_note {
                        IconName::SquareCheck
                    } else {
                        IconName::FileText
                    },
                    if is_note {
                        strings.convert_to_task
                    } else {
                        strings.convert_to_note
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_kind(cx))),
            )
            .into_any_element()
    }

    pub(super) fn render_draft_templates(&self, cx: &mut Context<Self>) -> AnyElement {
        let templates = self.session.store.templates().unwrap_or_default();
        popover(cx)
            .w(px(260.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popup(cx)))
            .when(templates.is_empty(), |el| {
                el.child(Label::new(i18n::t(cx).no_templates).color(Color::Muted))
            })
            .child(
                v_scroll(
                    "draft-templates-scroll",
                    v_flex()
                        .id("draft-templates")
                        .children(templates.into_iter().map(|template| {
                        ListItem::new(SharedString::from(format!(
                            "draft-template-{}",
                            template.id
                        )))
                        .child(Label::new(template.title.clone()).truncate())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.edit_draft(cx, |task, draft| {
                                    task.title = template.title.clone();
                                    task.notes = template.notes.clone();
                                    task.priority = template.priority;
                                    task.tags = template.tags.clone();
                                    draft.checklist = template
                                        .checklist
                                        .iter()
                                        .map(|text| ChecklistItem {
                                            id: Id::now_v7(),
                                            task_id: task.id,
                                            title: text.clone(),
                                            completed: false,
                                        })
                                        .collect();
                                    draft.priority_changed = true;
                                });
                                this.title.update(cx, |input, cx| {
                                    input.set_text(template.title.clone(), cx)
                                });
                                this.notes.update(cx, |input, cx| {
                                    input.set_text(template.notes.clone(), cx)
                                });
                                this.sync_checklist(cx);
                                this.refresh_draft_title(cx);
                                this.show_checklist = !this.checklist.is_empty();
                                this.dismiss_popup(window, cx);
                                window.focus(&this.focus_handle(cx), cx);
                            },
                        ))
                    })),
                )
                .max_h(px(240.))
                .compact(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn shared_draft_preserves_picker_values_checklist_and_notes_until_one_commit(
        cx: &mut TestAppContext,
    ) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let list = store.create_list("Work").unwrap();
        store.create_list("Home").unwrap();
        let day = NaiveDate::from_ymd_opt(2028, 7, 6).unwrap();
        let saved = Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            theme::init(cx);
            assert!(
                crate::keymap::load_default(&assets::Assets, cx)
                    .unwrap()
                    .is_empty()
            );
        });
        let (detail, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(true);
            detail.begin_draft(day, None, false, cx);
            window.focus(&detail.focus_handle(cx), cx);
            detail
        });
        cx.update(|_, cx| {
            let saved = saved.clone();
            cx.subscribe(&detail, move |_, _: &DraftClosed, _| {
                saved.set(saved.get() + 1)
            })
            .detach();
        });
        cx.simulate_input("Release !1 ^Home #parsed");
        cx.update(|window, cx| {
            detail.update(cx, |detail, cx| {
                detail.set_list(Some(list.id), window, cx);
                detail.set_priority(Priority::Low, cx);
                detail.open_due(cx);
                detail.update_draft(cx, |draft| {
                    draft.date = Some(day);
                    draft.time = chrono::NaiveTime::from_hms_opt(10, 0, 0);
                    draft.tab = DueTab::Duration;
                    draft.end_date = day.succ_opt();
                    draft.end_time = chrono::NaiveTime::from_hms_opt(11, 0, 0);
                    draft.toggle_reminder(15, day);
                    draft.repeat = Some(Recurrence::new(Frequency::Weekly, 1));
                });
                detail.apply_due(window, cx);
                detail.open_tags(Popup::Tags, window, cx);
                detail
                    .tag_input
                    .update(cx, |input, cx| input.set_text("picked", cx));
                detail.pick_typed_tag(window, cx);
                detail.apply_tags(window, cx);
                let item = detail.add_checklist_item("First", None, cx).unwrap();
                detail.toggle_checklist_item(item.id, true, cx);
                detail.show_checklist = true;
                detail
                    .new_item
                    .update(cx, |input, cx| input.set_text("Pending item", cx));
                detail
                    .notes
                    .update(cx, |input, cx| input.set_text("**Full** notes", cx));
                window.focus(&detail.notes.focus_handle(cx), cx);
                detail.open_popup(Popup::Priority, cx);
            })
        });
        assert!(store.tasks().unwrap().is_empty());
        assert!(
            store.tags().unwrap().is_empty(),
            "draft tags must not leak into Store"
        );
        cx.simulate_keystrokes("escape");
        assert!(
            store.tasks().unwrap().is_empty(),
            "Escape closes the nested picker first"
        );
        cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" });
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.title, "Release");
        assert_eq!(task.notes, "**Full** notes");
        assert_eq!(task.priority, Priority::Low);
        assert_eq!(task.list_id, Some(list.id));
        assert_eq!(task.due_date, Some(day));
        assert_eq!(task.end_date, day.succ_opt());
        assert_eq!(task.due_time, chrono::NaiveTime::from_hms_opt(10, 0, 0));
        assert_eq!(task.end_time, chrono::NaiveTime::from_hms_opt(11, 0, 0));
        assert_eq!(task.reminders, [15]);
        assert_eq!(task.repeat, Some(Recurrence::new(Frequency::Weekly, 1)));
        assert_eq!(task.tags, ["parsed", "picked"]);
        let items = store.checklist(task.id).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| (item.title.as_str(), item.completed))
                .collect::<Vec<_>>(),
            [("First", true), ("Pending item", false)]
        );
        cx.update(|_, cx| {
            detail.update(cx, |detail, cx| {
                assert!(detail.finish_draft(cx));
            })
        });
        assert_eq!(store.tasks().unwrap().len(), 1);
        assert_eq!(saved.get(), 1);
    }

    #[gpui::test]
    fn opening_and_confirming_short_window_picker_keeps_live_recognition(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let list = store.create_list("Notebook").unwrap();
        let mut list_draft = fern_core::ListDraft::of(&list);
        list_draft.kind = Kind::Note;
        store.update_list(list.id, &list_draft).unwrap();
        let today = Local::now().date_naive();
        let (detail, cx) = cx.add_window_view(|window, cx| {
            theme::init(cx);
            crate::keymap::load_default(&assets::Assets, cx).unwrap();
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(true);
            detail.begin_draft(today, None, true, cx);
            window.focus(&detail.focus_handle(cx), cx);
            detail
        });
        cx.simulate_resize(gpui::size(px(900.), px(400.)));
        cx.simulate_input("Review tomorrow 9am ^Notebook");
        cx.update(|_, cx| {
            let editor = detail.read(cx);
            let task = editor.session.task.as_ref().unwrap();
            let due = editor.due_preview(task, today);
            assert_eq!(due.date, today.succ_opt());
            assert_eq!(due.time, chrono::NaiveTime::from_hms_opt(9, 0, 0));
            assert_eq!(editor.kind_preview(task), Kind::Note);
        });
        let due = cx.debug_bounds("task-detail-due-date").unwrap();
        cx.simulate_click(due.center(), gpui::Modifiers::default());
        let ok = cx.debug_bounds("due-popup-ok").unwrap();
        assert!(ok.bottom() < px(400.));
        cx.simulate_click(ok.center(), gpui::Modifiers::default());
        assert!(cx.debug_bounds("due-popup-ok").is_none());
        cx.update(|_, cx| {
            detail.update(cx, |editor, cx| {
                assert!(editor.finish_draft(cx));
            })
        });
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks[0].title, "Review");
        assert_eq!(tasks[0].list_id, Some(list.id));
        assert_eq!(tasks[0].kind, Kind::Note);
        assert_eq!(tasks[0].due_date, today.succ_opt());
    }

    #[gpui::test]
    fn enter_confirms_the_date_picker_before_saving_a_task(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let day = Local::now().date_naive();
        let picked = day.succ_opt().unwrap();
        let (detail, cx) = cx.add_window_view(|window, cx| {
            theme::init(cx);
            crate::keymap::load_default(&assets::Assets, cx).unwrap();
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.set_card(true);
            detail.begin_draft(day, None, true, cx);
            window.focus(&detail.focus_handle(cx), cx);
            detail
        });
        cx.simulate_input("Calendar task");
        let due = cx.debug_bounds("task-detail-due-date").unwrap();
        cx.simulate_click(due.center(), gpui::Modifiers::default());
        cx.update(|_, cx| {
            detail.update(cx, |editor, cx| {
                editor.update_draft(cx, |due| due.pick_date(picked));
            });
        });

        cx.simulate_keystrokes("enter");
        assert!(cx.debug_bounds("due-popup-ok").is_none());
        assert!(store.tasks().unwrap().is_empty());

        cx.simulate_keystrokes("enter");
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].due_date, Some(picked));
    }

    #[gpui::test]
    fn picker_time_gets_timed_default_and_manual_removal_survives_commit(cx: &mut TestAppContext) {
        let store = Rc::new(Store::open_in_memory().unwrap());
        let mut preferences = store.preferences().unwrap();
        preferences.task_defaults.all_day_reminder = Some(24 * 60);
        preferences.task_defaults.timed_reminder = Some(15);
        store.save_preferences(&preferences).unwrap();
        let day = Local::now().date_naive();
        let time = chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        cx.update(theme::init);
        let (detail, cx) =
            cx.add_window_view(|window, cx| TaskDetail::new(store.clone(), window, cx));
        cx.update(|window, cx| {
            detail.update(cx, |editor, cx| {
                for (title, remove_reminder) in
                    [("Default reminder", false), ("Manual removal", true)]
                {
                    editor.begin_draft(day, None, true, cx);
                    editor
                        .title
                        .update(cx, |input, cx| input.set_text(title, cx));
                    editor.open_due(cx);
                    editor.update_draft(cx, |due| due.pick_time(Some(time), day));
                    assert_eq!(editor.session.due_draft.as_ref().unwrap().reminders, [15]);
                    if remove_reminder {
                        editor.update_draft(cx, |due| due.toggle_reminder(15, day));
                    }
                    editor.apply_due(window, cx);
                    assert!(editor.finish_draft(cx));
                }
            });
        });
        let tasks = store.tasks().unwrap();
        let default = tasks
            .iter()
            .find(|task| task.title == "Default reminder")
            .unwrap();
        let removed = tasks
            .iter()
            .find(|task| task.title == "Manual removal")
            .unwrap();
        assert_eq!(default.due_time, Some(time));
        assert_eq!(default.reminders, [15]);
        assert_eq!(removed.due_time, Some(time));
        assert!(removed.reminders.is_empty());
    }

    #[gpui::test]
    fn draft_files_are_staged_and_a_deleted_list_falls_back_before_commit(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("fern-draft-test-{}", Id::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("draft.txt");
        std::fs::write(&source, "Attached before saving").unwrap();
        let attachments_dir = dir.join("attachments");
        let store = Rc::new(Store::open_in_memory().unwrap());
        let list = store.create_list("Temporary").unwrap();
        cx.update(theme::init);
        let (detail, cx) = cx.add_window_view(|window, cx| {
            let mut detail = TaskDetail::new(store.clone(), window, cx);
            detail.session.attachments_dir = Some(attachments_dir.clone());
            detail.begin_draft(
                NaiveDate::from_ymd_opt(2028, 7, 6).unwrap(),
                Some(list.id),
                false,
                cx,
            );
            detail
        });
        cx.update(|_, cx| {
            detail.update(cx, |detail, cx| {
                detail.attach_paths(detail.task_id().unwrap(), vec![source], cx);
                assert!(
                    !detail.finish_draft(cx),
                    "wait for pending file copies before saving"
                );
            })
        });
        cx.run_until_parked();
        assert!(store.tasks().unwrap().is_empty());
        assert!(cx.debug_bounds("task-attachment").is_some());
        store.delete_list(list.id).unwrap();
        cx.update(|_, cx| {
            detail.update(cx, |detail, cx| {
                detail
                    .title
                    .update(cx, |input, cx| input.set_text("Draft with file", cx));
                assert!(detail.finish_draft(cx));
            })
        });
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].list_id, None);
        let files = store.attachments(tasks[0].id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            std::fs::read_to_string(files[0].path(&attachments_dir)).unwrap(),
            "Attached before saving"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
