//! Diagnostics ring buffer, secret redaction, and sanitized bug-report generation.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use chrono::{Local, Utc};
use fern_core::Store;
use fern_core::store::list_backups;
use fern_core::sync::SYNC_PROTOCOL_VERSION;

use crate::sync::SyncStatus;

const MAX_EVENTS: usize = 200;

static EVENTS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Database integrity and recovery status captured at startup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DbHealth {
    pub healthy: bool,
    pub messages: Vec<String>,
    pub recovered_corrupt_backup: Option<PathBuf>,
}

impl gpui::Global for DbHealth {}

/// Records a timestamped diagnostic event in the in-memory ring buffer (capped at 200 entries).
///
/// Embedded newlines are collapsed and sensitive tokens/URLs/passphrases are redacted before storage.
pub fn log_event(category: &str, message: impl AsRef<str>) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let single_line = message
        .as_ref()
        .replace(['\r', '\n'], " ");
    let sanitized = redact_sensitive(single_line.trim());
    let entry = format!("{timestamp} [{category}] {sanitized}");
    let mut guard = EVENTS.lock().unwrap_or_else(PoisonError::into_inner);
    while guard.len() >= MAX_EVENTS {
        guard.pop_front();
    }
    guard.push_back(entry);
}

/// Returns a snapshot of recent diagnostic events, oldest first.
pub fn recent_events() -> Vec<String> {
    let guard = EVENTS.lock().unwrap_or_else(PoisonError::into_inner);
    guard.iter().cloned().collect()
}

/// Clears all buffered diagnostic events. Available in tests only.
#[cfg(test)]
pub fn clear_events() {
    let mut guard = EVENTS.lock().unwrap_or_else(PoisonError::into_inner);
    guard.clear();
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Redacts credentials, passphrases, tokens, and HTTP(S) URLs (preserving only scheme +
/// `<redacted-host>`) so error strings and diagnostic reports never leak secrets or private hosts.
pub fn redact_sensitive(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

    while i < text.len() {
        let rest = &text[i..];

        // 1. HTTP(S) URLs: keep scheme + "<redacted-host>", strip host/credentials/path/query.
        if let Some((scheme, after_scheme)) = match_http_scheme(text, i) {
            out.push_str(scheme);
            out.push_str("<redacted-host>");
            let consumed = consume_url_rest(&text[after_scheme..]);
            i = after_scheme + consumed;
            continue;
        }

        // 2. "Bearer <token>" authorization values.
        if let Some((consumed, replacement)) = match_bearer(text, i) {
            out.push_str(&replacement);
            i += consumed;
            continue;
        }

        // 3. Key-value / JSON secrets: passphrase=..., token=..., "passphrase": "...", "token": "...".
        if let Some((consumed, replacement)) = match_secret_kv(text, i) {
            out.push_str(&replacement);
            i += consumed;
            continue;
        }

        let Some(ch) = rest.chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

fn starts_with_ascii_ci(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len()
        && haystack.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
}

fn prev_byte(text: &str, i: usize) -> Option<u8> {
    if i == 0 {
        None
    } else {
        Some(text.as_bytes()[i - 1])
    }
}

fn match_http_scheme(text: &str, i: usize) -> Option<(&'static str, usize)> {
    if prev_byte(text, i).is_some_and(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let rest = &text[i..];
    if starts_with_ascii_ci(rest, "https://") {
        Some(("https://", i + "https://".len()))
    } else if starts_with_ascii_ci(rest, "http://") {
        Some(("http://", i + "http://".len()))
    } else {
        None
    }
}

fn consume_url_rest(rest: &str) -> usize {
    const PLACEHOLDER: &str = "<redacted-host>";
    if let Some(after) = rest.strip_prefix(PLACEHOLDER) {
        return PLACEHOLDER.len() + consume_url_body(after, false);
    }
    consume_url_body(rest, rest.starts_with('['))
}

fn consume_url_body(rest: &str, mut in_ipv6_brackets: bool) -> usize {
    let mut end = 0;
    for (idx, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            break;
        }
        match ch {
            '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '{' | '}' | ',' | ';' => break,
            '[' => {
                if idx == 0 && in_ipv6_brackets {
                    end = idx + 1;
                } else {
                    break;
                }
            }
            ']' => {
                if in_ipv6_brackets {
                    in_ipv6_brackets = false;
                    end = idx + 1;
                } else {
                    break;
                }
            }
            _ => {
                end = idx + ch.len_utf8();
            }
        }
    }
    // Leave trailing sentence punctuation (e.g. `https://host/sync: connection refused` or
    // `at https://host.`) outside the redacted URL span for readable error prose.
    let mut trimmed = &rest[..end];
    while let Some(last) = trimmed.chars().next_back() {
        if matches!(last, ':' | '.' | '!' | '?') {
            trimmed = &trimmed[..trimmed.len() - last.len_utf8()];
        } else {
            break;
        }
    }
    trimmed.len()
}

fn match_bearer(text: &str, i: usize) -> Option<(usize, String)> {
    if prev_byte(text, i).is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let rest = &text[i..];
    if !starts_with_ascii_ci(rest, "bearer") {
        return None;
    }
    let after_kw = "bearer".len();
    let ws_len = rest[after_kw..]
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count();
    if ws_len == 0 {
        return None;
    }
    let val_start = after_kw + ws_len;
    let val_rest = &rest[val_start..];
    if val_rest.is_empty() {
        return None;
    }

    if val_rest.starts_with("<redacted>") {
        return Some((val_start + "<redacted>".len(), "Bearer <redacted>".to_owned()));
    }
    if let Some((quote, quoted_len)) = consume_quoted_value(val_rest) {
        return Some((val_start + quoted_len, format!("Bearer {quote}<redacted>{quote}")));
    }

    let tok_len = consume_unquoted_token(val_rest);
    if tok_len == 0 {
        return None;
    }
    Some((val_start + tok_len, "Bearer <redacted>".to_owned()))
}

fn match_secret_kv(text: &str, i: usize) -> Option<(usize, String)> {
    if prev_byte(text, i).is_some_and(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let rest = &text[i..];
    let kw_len = if starts_with_ascii_ci(rest, "passphrase") {
        "passphrase".len()
    } else if starts_with_ascii_ci(rest, "token") {
        "token".len()
    } else {
        return None;
    };
    let mut pos = kw_len;
    let key_quoted = prev_byte(text, i).filter(|b| *b == b'"' || *b == b'\'');

    // JSON / quoted key style (`"passphrase": "secret"`) or Debug/YAML style (`passphrase: "secret"`).
    if let Some(q) = key_quoted {
        if rest.as_bytes().get(pos) == Some(&q) {
            pos += 1;
            pos += count_hws(&rest[pos..]);
            if rest.as_bytes().get(pos) == Some(&b':') {
                pos += 1;
                pos += count_hws(&rest[pos..]);
                if let Some((val_q, quoted_len)) = consume_quoted_value(&rest[pos..]) {
                    let prefix = &rest[..pos];
                    return Some((pos + quoted_len, format!("{prefix}{val_q}<redacted>{val_q}")));
                }
            }
        }
        return None;
    }

    pos += count_hws(&rest[pos..]);
    if rest.as_bytes().get(pos) == Some(&b':') {
        pos += 1;
        pos += count_hws(&rest[pos..]);
        if let Some((val_q, quoted_len)) = consume_quoted_value(&rest[pos..]) {
            let prefix = &rest[..pos];
            return Some((pos + quoted_len, format!("{prefix}{val_q}<redacted>{val_q}")));
        }
        return None;
    }

    // Standard `key=value` style (with optional horizontal whitespace around `=`).
    if rest.as_bytes().get(pos) != Some(&b'=') {
        return None;
    }
    pos += 1;
    pos += count_hws(&rest[pos..]);
    let val_rest = &rest[pos..];
    if val_rest.is_empty() {
        return None;
    }

    let prefix = &rest[..pos];
    if val_rest.starts_with("<redacted>") {
        return Some((pos + "<redacted>".len(), format!("{prefix}<redacted>")));
    }
    if let Some((val_q, quoted_len)) = consume_quoted_value(val_rest) {
        return Some((pos + quoted_len, format!("{prefix}{val_q}<redacted>{val_q}")));
    }
    let val_len = consume_unquoted_kv_value(val_rest);
    if val_len == 0 {
        return None;
    }
    Some((pos + val_len, format!("{prefix}<redacted>")))
}

fn count_hws(s: &str) -> usize {
    s.bytes().take_while(|b| *b == b' ' || *b == b'\t').count()
}

fn consume_quoted_value(s: &str) -> Option<(char, usize)> {
    let mut chars = s.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut escaped = false;
    for (idx, ch) in chars {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some((quote, idx + ch.len_utf8()));
        }
        if ch == '\n' || ch == '\r' {
            break;
        }
    }
    None
}

fn consume_unquoted_token(s: &str) -> usize {
    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        if ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ';' | ')' | ']' | '}' | '>' | '<') {
            break;
        }
        end = idx + ch.len_utf8();
    }
    end
}

fn consume_unquoted_kv_value(s: &str) -> usize {
    let mut end = 0;
    for (idx, ch) in s.char_indices() {
        if ch.is_whitespace()
            || matches!(ch, '&' | ',' | ';' | ')' | ']' | '}' | '"' | '\'' | '>' | '<')
        {
            break;
        }
        end = idx + ch.len_utf8();
    }
    end
}

/// Inputs gathered from the running application to build a diagnostic bug report.
pub struct DiagnosticInput<'a> {
    pub app_version: &'a str,
    pub locale: &'a str,
    pub db_path: Option<&'a Path>,
    pub backups_dir: Option<&'a Path>,
    pub sync_status: &'a SyncStatus,
    pub db_health: Option<&'a DbHealth>,
}

/// Generates a sanitized plain-text diagnostic report suitable for attaching to bug reports.
///
/// The report intentionally excludes private user content (task titles, notes, tags), home
/// directory usernames, sync server URLs, bearer tokens, and E2EE passphrases.
pub fn build_diagnostic_report(store: &Store, input: &DiagnosticInput<'_>) -> String {
    let mut out = String::with_capacity(1024);

    // Header
    let _ = writeln!(out, "Fern Diagnostic Report");
    let _ = writeln!(out, "======================");
    let _ = writeln!(out, "generated_utc: {}", Utc::now().format("%Y-%m-%d %H:%M:%S UTC"));
    let _ = writeln!(out, "generated_local: {}", Local::now().format("%Y-%m-%d %H:%M:%S"));
    let _ = writeln!(out, "app_version: {}", input.app_version);
    let _ = writeln!(out, "os: {}", std::env::consts::OS);
    let _ = writeln!(out, "arch: {}", std::env::consts::ARCH);
    let _ = writeln!(out, "locale: {}", input.locale);

    // Database section
    let _ = writeln!(out);
    let _ = writeln!(out, "[Database]");
    match input.db_path {
        Some(path) => {
            let name = file_basename(path);
            match std::fs::metadata(path) {
                Ok(meta) => {
                    let _ = writeln!(out, "file: {name} (exists, {} bytes)", meta.len());
                }
                Err(_) => {
                    let _ = writeln!(out, "file: {name} (missing)");
                }
            }
        }
        None => {
            let _ = writeln!(out, "file: unavailable");
        }
    }
    match store.schema_version() {
        Ok(version) => {
            let _ = writeln!(out, "schema_version: {version}");
        }
        Err(err) => {
            let _ = writeln!(out, "schema_version: error ({})", redact_sensitive(&format!("{err:#}")));
        }
    }
    match store.integrity_check() {
        Ok(messages) => {
            let joined = if messages.is_empty() {
                "empty".to_owned()
            } else {
                messages
                    .iter()
                    .map(|m| redact_sensitive(m))
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let _ = writeln!(out, "integrity_check: {joined}");
        }
        Err(err) => {
            let _ = writeln!(out, "integrity_check: error ({})", redact_sensitive(&format!("{err:#}")));
        }
    }
    if let Some(health) = input.db_health {
        let state = if health.healthy { "healthy" } else { "unhealthy" };
        let _ = writeln!(out, "startup_health: {state}");
        if !health.messages.is_empty() {
            let joined = health
                .messages
                .iter()
                .map(|m| redact_sensitive(m))
                .collect::<Vec<_>>()
                .join("; ");
            let _ = writeln!(out, "startup_messages: {joined}");
        }
        if let Some(backup) = &health.recovered_corrupt_backup {
            let _ = writeln!(out, "recovered_corrupt_backup: {}", file_basename(backup));
        }
    }
    match store.table_counts() {
        Ok(counts) => {
            let _ = writeln!(
                out,
                "table_counts: tasks={}, lists={}, habits={}, attachments={}, countdowns={}",
                counts.tasks, counts.lists, counts.habits, counts.attachments, counts.countdowns,
            );
        }
        Err(err) => {
            let _ = writeln!(out, "table_counts: error ({})", redact_sensitive(&format!("{err:#}")));
        }
    }

    // Sync section (NEVER include server URL, bearer token, or E2EE passphrase)
    let _ = writeln!(out);
    let _ = writeln!(out, "[Sync]");
    match store.sync_config() {
        Ok(Some(config)) => {
            let configured = if config.is_configured() { "yes" } else { "no" };
            let e2ee = if config.passphrase.is_empty() { "disabled" } else { "enabled" };
            let _ = writeln!(out, "configured: {configured}");
            let _ = writeln!(out, "e2ee: {e2ee}");
        }
        Ok(None) => {
            let _ = writeln!(out, "configured: no");
            let _ = writeln!(out, "e2ee: disabled");
        }
        Err(err) => {
            let _ = writeln!(out, "configured: unknown ({})", redact_sensitive(&format!("{err:#}")));
            let _ = writeln!(out, "e2ee: unknown");
        }
    }
    let _ = writeln!(out, "protocol_version: {SYNC_PROTOCOL_VERSION}");
    let status_text = match input.sync_status {
        SyncStatus::Off => "off".to_owned(),
        SyncStatus::Running => "running".to_owned(),
        SyncStatus::Done(at) => format!("done ({})", at.format("%Y-%m-%d %H:%M:%S")),
        SyncStatus::Failed(msg) => format!("failed ({})", redact_sensitive(msg)),
    };
    let _ = writeln!(out, "status: {status_text}");

    // Auto-backups section
    let _ = writeln!(out);
    let _ = writeln!(out, "[Auto-Backups]");
    match input.backups_dir {
        Some(dir) => {
            let backups = list_backups(dir);
            if backups.is_empty() {
                let _ = writeln!(out, "(none)");
            } else {
                for entry in backups {
                    let date = entry
                        .date
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "unknown".to_owned());
                    let _ = writeln!(
                        out,
                        "- {} (date: {date}, size_bytes: {})",
                        entry.filename, entry.size_bytes
                    );
                }
            }
        }
        None => {
            let _ = writeln!(out, "(unavailable)");
        }
    }

    // Recent events section
    let _ = writeln!(out);
    let _ = writeln!(out, "[Recent Events]");
    let events = recent_events();
    if events.is_empty() {
        let _ = writeln!(out, "(none)");
    } else {
        for event in events {
            let _ = writeln!(out, "- {}", redact_sensitive(&event));
        }
    }

    out
}

fn file_basename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| "<unknown>".to_owned())
}

#[cfg(test)]
mod tests {
    use fern_core::CountdownKind;
    use fern_core::sync::SyncConfig;

    use super::*;

    #[test]
    fn ring_buffer_caps_at_200_entries_and_formats_timestamps() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        clear_events();

        for idx in 0..250 {
            log_event("test-cap", format!("ring-event-{idx}"));
        }

        let events = recent_events();
        assert_eq!(events.len(), MAX_EVENTS);
        assert!(
            !events.iter().any(|e| e.contains("ring-event-0")),
            "oldest entries beyond 200 must be evicted"
        );
        assert!(
            !events.iter().any(|e| e.contains("ring-event-49")),
            "entry 49 must be evicted after 250 pushes"
        );
        let newest = events
            .iter()
            .find(|e| e.contains("[test-cap] ring-event-249"))
            .expect("newest entry must be retained");
        // Verify YYYY-MM-DD HH:MM:SS prefix length and separators
        let bytes = newest.as_bytes();
        assert!(bytes.len() > 20);
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b' ');
        assert_eq!(bytes[13], b':');
        assert_eq!(bytes[16], b':');
    }

    #[test]
    fn redact_sensitive_strips_urls_bearer_tokens_and_passphrases() {
        let input = concat!(
            "Failed POST https://user:pw@sync.private.example:8443/sync?token=abc123: ",
            "Authorization: Bearer tok_super_secret_42, ",
            "passphrase=correct-horse-battery ",
            "token=\"quoted secret token\" ",
            "sync_passphrase='single quoted' ",
            "http://[::1]:9000/files/test.pdf ",
            r#"{"token":"json-tok","passphrase":"json-pass"} "#,
            r#"SyncConfig { token: "dbg-tok", passphrase: "dbg pass" }"#,
        );

        let redacted = redact_sensitive(input);
        assert!(redacted.contains("https://<redacted-host>:"));
        assert!(redacted.contains("http://<redacted-host>"));
        assert!(redacted.contains("Bearer <redacted>"));
        assert!(redacted.contains("passphrase=<redacted>"));
        assert!(redacted.contains("token=\"<redacted>\""));
        assert!(redacted.contains("sync_passphrase='<redacted>'"));
        assert!(redacted.contains(r#""token":"<redacted>""#));
        assert!(redacted.contains(r#""passphrase":"<redacted>""#));
        assert!(redacted.contains(r#"token: "<redacted>""#));
        assert!(redacted.contains(r#"passphrase: "<redacted>""#));

        for leaked in [
            "sync.private.example",
            "user:pw",
            "8443",
            "abc123",
            "tok_super_secret_42",
            "correct-horse-battery",
            "quoted secret token",
            "single quoted",
            "::1",
            "9000",
            "test.pdf",
            "json-tok",
            "json-pass",
            "dbg-tok",
            "dbg pass",
        ] {
            assert!(!redacted.contains(leaked), "leaked {leaked:?} in: {redacted}");
        }

        // Redaction must be idempotent.
        assert_eq!(redact_sensitive(&redacted), redacted);

        // Non-secret prose is preserved intact.
        assert_eq!(
            redact_sensitive("database integrity ok, ringbearer walked safely"),
            "database integrity ok, ringbearer walked safely"
        );
    }

    #[test]
    fn build_diagnostic_report_includes_metadata_and_never_leaks_secrets_or_user_data()
    -> anyhow::Result<()> {
        let _guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        clear_events();

        let tmp = std::env::temp_dir().join(format!("fern-diag-test-{}", uuid::Uuid::now_v7()));
        let backups = tmp.join("backups");
        let attachments = tmp.join("attachments");
        std::fs::create_dir_all(&backups)?;
        std::fs::create_dir_all(&attachments)?;
        let db_path = tmp.join("tasks.db");

        let store = Store::open(&db_path)?;
        let list = store.create_list("SECRET_LIST_NAME")?;
        let task = store.add_task("SUPER_SECRET_TASK_TITLE")?;
        store.set_list(task.id, Some(list.id))?;
        store.set_notes(task.id, "TOP_SECRET_TASK_NOTES_BODY")?;
        store.set_tags(task.id, &["secret-private-tag".to_owned()])?;

        let sample_file = tmp.join("secret-medical-report.txt");
        std::fs::write(&sample_file, b"private attachment content")?;
        store.add_attachment(task.id, &sample_file, &attachments)?;

        store.add_habit("SECRET_HABIT_NAME")?;
        store.add_countdown(
            "SECRET_COUNTDOWN_NAME",
            chrono::NaiveDate::from_ymd_opt(2027, 1, 1).expect("valid date"),
            CountdownKind::Countdown,
        )?;

        let secret_server = "https://private-fern-host.internal.example:8443";
        let secret_token = "tok_ultra_secret_bearer_999111";
        let secret_passphrase = "ultra-secret-e2ee-passphrase-777";
        store.set_sync_config(&SyncConfig {
            server: secret_server.to_owned(),
            token: secret_token.to_owned(),
            passphrase: secret_passphrase.to_owned(),
        })?;

        std::fs::write(backups.join("fern-2026-11-20.db"), vec![0u8; 128])?;
        std::fs::write(backups.join("fern-2026-11-21.db"), vec![0u8; 256])?;

        log_event(
            "sync",
            format!(
                "diag-marker failed posting to {secret_server}/sync with Bearer {secret_token} passphrase={secret_passphrase}"
            ),
        );

        let health = DbHealth {
            healthy: false,
            messages: vec!["recovered from corrupt page".to_owned()],
            recovered_corrupt_backup: Some(PathBuf::from(
                "/home/alice_secret_user/fern/tasks.corrupt-2026-11-21.db",
            )),
        };
        let sync_status = SyncStatus::Failed(format!(
            "connect {secret_server}/sync failed (Bearer {secret_token}, token={secret_token}, passphrase={secret_passphrase})"
        ));
        let input = DiagnosticInput {
            app_version: "0.2.0-diagtest",
            locale: "ru",
            db_path: Some(&db_path),
            backups_dir: Some(&backups),
            sync_status: &sync_status,
            db_health: Some(&health),
        };

        let report = build_diagnostic_report(&store, &input);
        drop(store);
        let _ = std::fs::remove_dir_all(&tmp);

        // Expected sections and metadata
        assert!(report.contains("Fern Diagnostic Report"));
        assert!(report.contains("app_version: 0.2.0-diagtest"));
        assert!(report.contains(&format!("os: {}", std::env::consts::OS)));
        assert!(report.contains(&format!("arch: {}", std::env::consts::ARCH)));
        assert!(report.contains("locale: ru"));
        assert!(report.contains("file: tasks.db (exists,"));
        assert!(report.contains(&format!("schema_version: {}", fern_core::SCHEMA_VERSION)));
        assert!(report.contains("integrity_check: ok"));
        assert!(report.contains("startup_health: unhealthy"));
        assert!(report.contains("recovered_corrupt_backup: tasks.corrupt-2026-11-21.db"));
        assert!(report.contains("table_counts: tasks=1, lists=1, habits=1, attachments=1, countdowns=1"));
        assert!(report.contains("configured: yes"));
        assert!(report.contains("e2ee: enabled"));
        assert!(report.contains(&format!("protocol_version: {SYNC_PROTOCOL_VERSION}")));
        assert!(report.contains("fern-2026-11-21.db (date: 2026-11-21, size_bytes: 256)"));
        assert!(report.contains("fern-2026-11-20.db (date: 2026-11-20, size_bytes: 128)"));
        assert!(report.contains("[sync] diag-marker failed posting to https://<redacted-host>"));
        assert!(report.contains("Bearer <redacted>"));

        // Strictly NEVER leak secrets, user content, or home username
        for forbidden in [
            secret_server,
            "private-fern-host.internal.example",
            secret_token,
            secret_passphrase,
            "SUPER_SECRET_TASK_TITLE",
            "TOP_SECRET_TASK_NOTES_BODY",
            "secret-private-tag",
            "SECRET_LIST_NAME",
            "SECRET_HABIT_NAME",
            "SECRET_COUNTDOWN_NAME",
            "secret-medical-report",
            "alice_secret_user",
        ] {
            assert!(!report.contains(forbidden), "report must not leak {forbidden:?}:\n{report}");
        }

        Ok(())
    }
}
