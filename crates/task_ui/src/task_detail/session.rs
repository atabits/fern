//! Data shared by the editor and its transient surfaces. No window, focus or
//! subscription belongs here: opening a popup does not transfer the editing session.

use std::collections::HashMap;

use super::*;

pub(super) struct TaskDetailSession {
    pub store: Rc<Store>,
    pub preferences: fern_core::Preferences,
    pub attachments_dir: Option<PathBuf>,
    pub attachments: Vec<fern_core::Attachment>,
    pub attachment_errors: Vec<String>,
    pub comments: Vec<fern_core::Comment>,
    pub task: Option<Task>,
    pub task_depth: usize,
    pub draft: Option<draft::NewTask>,
    pub tag_draft: Option<TagDraft>,
    pub all_tags: Vec<Tag>,
    pub lists: Vec<List>,
    pub list_sections: HashMap<Option<Id>, Vec<fern_core::ListSection>>,
    pub link_parent_candidates: Vec<Task>,
    pub draft_templates: Vec<fern_core::Template>,
    pub subtasks: Vec<Task>,
    pub parent: Option<Task>,
    pub popup: Option<Popup>,
    pub due_draft: Option<DueDraft>,
    pub specific_month: Option<NaiveDate>,
    pub estimate_draft: EstimateDraft,
    pub activity: Option<Vec<fern_core::Activity>>,
}

impl TaskDetailSession {
    pub fn new(store: Rc<Store>) -> Self {
        let preferences = store.preferences().unwrap_or_default();
        Self {
            store,
            preferences,
            attachments_dir: crate::attachments_dir().ok(),
            attachments: Vec::new(),
            attachment_errors: Vec::new(),
            comments: Vec::new(),
            task: None,
            task_depth: 0,
            draft: None,
            tag_draft: None,
            all_tags: Vec::new(),
            lists: Vec::new(),
            list_sections: HashMap::new(),
            link_parent_candidates: Vec::new(),
            draft_templates: Vec::new(),
            subtasks: Vec::new(),
            parent: None,
            popup: None,
            due_draft: None,
            specific_month: None,
            estimate_draft: EstimateDraft { minutes: false, amount: 0 },
            activity: None,
        }
    }
}
