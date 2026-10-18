//! Links to tasks ("Copy Link"): `fern://task/<id>`, which the desktop app opens.

use crate::model::Id;

const TASK_PREFIX: &str = "fern://task/";

pub fn task_link(id: Id) -> String {
    format!("{TASK_PREFIX}{id}")
}

/// The task a link points to, if it is a Fern task link.
pub fn parse_task_link(text: &str) -> Option<Id> {
    text.trim().strip_prefix(TASK_PREFIX)?.trim_end_matches('/').parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let id = uuid::Uuid::now_v7();
        assert_eq!(parse_task_link(&task_link(id)), Some(id));
        assert_eq!(parse_task_link(&format!(" {}/ ", task_link(id))), Some(id));
        assert_eq!(parse_task_link("https://example.com/task/1"), None);
        assert_eq!(parse_task_link("fern://task/not-an-id"), None);
    }
}
