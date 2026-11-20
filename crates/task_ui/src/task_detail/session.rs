//! Data shared by the editor and its transient surfaces. No window, focus or
//! subscription belongs here: opening a popup does not transfer the editing session.

use super::*;

pub(super) struct TaskDetailSession {
    pub store: Rc<Store>,
    pub attachments_dir: Option<PathBuf>,
    pub attachments: Vec<fern_core::Attachment>,
    pub attachment_errors: Vec<String>,
    pub comments: Vec<fern_core::Comment>,
    pub task: Option<Task>,
    pub draft: Option<draft::NewTask>,
    pub tag_draft: Option<TagDraft>,
    pub all_tags: Vec<Tag>,
    pub lists: Vec<List>,
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
        Self {
            store,
            attachments_dir: crate::attachments_dir().ok(),
            attachments: Vec::new(),
            attachment_errors: Vec::new(),
            comments: Vec::new(),
            task: None,
            draft: None,
            tag_draft: None,
            all_tags: Vec::new(),
            lists: Vec::new(),
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
