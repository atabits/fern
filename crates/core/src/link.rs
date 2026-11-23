//! Deep links into Fern (`fern://task/<id>`, `fern://plugin/<plugin_id>/<command>?k=v`).

use crate::model::Id;

const TASK_PREFIX: &str = "fern://task/";
const PLUGIN_PREFIX: &str = "fern://plugin/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeepLink {
    Task(Id),
    Plugin {
        plugin_id: String,
        command: String,
        params: Vec<(String, String)>,
    },
}

pub fn task_link(id: Id) -> String {
    format!("{TASK_PREFIX}{id}")
}

/// The task a link points to, if it is a Fern task link.
pub fn parse_task_link(text: &str) -> Option<Id> {
    let without_fragment = text.trim().split('#').next()?;
    let without_query = without_fragment.split('?').next()?;
    without_query.strip_prefix(TASK_PREFIX)?.trim_end_matches('/').parse().ok()
}

pub fn plugin_link(plugin_id: &str, command: &str) -> String {
    format!("{PLUGIN_PREFIX}{plugin_id}/{command}")
}

/// Parses any supported `fern://` deep link (`task` or `plugin`).
pub fn parse_deep_link(text: &str) -> Option<DeepLink> {
    let trimmed = text.trim();
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    if let Some(id) = parse_task_link(without_fragment) {
        return Some(DeepLink::Task(id));
    }
    let rest = without_fragment.strip_prefix(PLUGIN_PREFIX)?;
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let path = path.trim_matches('/');
    let (plugin_id, raw_command) = path.split_once('/')?;
    if !is_valid_plugin_id(plugin_id) || raw_command.is_empty() || raw_command.contains('/') {
        return None;
    }
    let command = percent_decode(raw_command);
    if command.trim().is_empty() || command.contains('/') {
        return None;
    }
    let params = query.map(parse_query_params).unwrap_or_default();
    Some(DeepLink::Plugin {
        plugin_id: plugin_id.to_owned(),
        command,
        params,
    })
}

fn is_valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && id != "."
        && id != ".."
}

fn parse_query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(part), String::new()),
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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

        assert_eq!(parse_deep_link(&task_link(id)), Some(DeepLink::Task(id)));
        assert_eq!(
            parse_deep_link("fern://plugin/com.example.gh/sync?repo=owner%2Frepo&full=1&flag"),
            Some(DeepLink::Plugin {
                plugin_id: "com.example.gh".into(),
                command: "sync".into(),
                params: vec![
                    ("repo".into(), "owner/repo".into()),
                    ("full".into(), "1".into()),
                    ("flag".into(), String::new()),
                ],
            })
        );
        assert_eq!(parse_deep_link("fern://plugin/../escape/cmd"), None);
        assert_eq!(parse_deep_link("fern://plugin/com.example.gh/"), None);
        assert_eq!(parse_deep_link("fern://plugin/com.example.gh"), None);

        // Fragment stripping & post-decode `/` rejection.
        assert_eq!(parse_deep_link(&format!("{}#details", task_link(id))), Some(DeepLink::Task(id)));
        assert_eq!(
            parse_deep_link("fern://plugin/com.example.gh/sync#top"),
            Some(DeepLink::Plugin {
                plugin_id: "com.example.gh".into(),
                command: "sync".into(),
                params: vec![],
            })
        );
        assert_eq!(
            parse_deep_link("fern://plugin/com.example.gh/sync?full=1#frag"),
            Some(DeepLink::Plugin {
                plugin_id: "com.example.gh".into(),
                command: "sync".into(),
                params: vec![("full".into(), "1".into())],
            })
        );
        assert_eq!(parse_deep_link("fern://plugin/com.example.gh/a%2Fb"), None);
        assert_eq!(parse_deep_link("fern://plugin/com.example.gh/a%2fb?x=1"), None);
    }
}
