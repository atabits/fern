//! Syncing with the user's own `fern-server`: the request and the answer travel as JSON over
//! HTTP(S); what they hold is the store's business (`fern_core::sync`).

use chrono::NaiveDateTime;
use fern_core::attachment::{AttachmentTooLarge, MAX_ATTACHMENT_BYTES, check_attachment_size, save_download};
use fern_core::e2ee::{self, E2eeError, E2eeKey};
use fern_core::sync::{
    IncompatibleProtocol, MAX_FILE_KEY_BYTES, MIN_SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION,
    SyncConfig, encode_file_key, file_key,
};
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

/// How the last sync went, for the sidebar's sync button.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SyncStatus {
    /// No server set.
    #[default]
    Off,
    Running,
    Done(NaiveDateTime),
    Failed(String),
}

/// How often the app syncs even without local changes, to fetch other devices' changes.
pub const PULL_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// How often it checks for local changes to send.
pub const PUSH_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// A request must finish reading its body within this time, even if the peer keeps sending bytes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// Maximum JSON sync response size (32 MiB) so a corrupt or hostile server cannot OOM the client.
const MAX_SYNC_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
/// Extra byte budget on encrypted attachment downloads (9-byte header + 12-byte nonce + 16-byte tag).
const E2EE_FILE_OVERHEAD: u64 = 64;
/// Maximum error response body bytes read when reporting non-2xx HTTP sync failures.
const MAX_ERROR_BODY_BYTES: u64 = 64 * 1024;

fn sync_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).build().into()
}

/// Returns the underlying [`IncompatibleProtocol`] error if `err` (or its source chain) represents
/// a sync protocol version mismatch.
pub fn is_incompatible_protocol(err: &anyhow::Error) -> Option<&IncompatibleProtocol> {
    err.chain().find_map(|cause| cause.downcast_ref::<IncompatibleProtocol>())
}

/// Returns the underlying [`E2eeError`] if `err` (or its source chain) represents a client-side
/// end-to-end encryption error.
pub fn is_e2ee_error(err: &anyhow::Error) -> Option<&E2eeError> {
    err.chain().find_map(|cause| cause.downcast_ref::<E2eeError>())
}

pub fn attachment_error(error: &anyhow::Error, strings: &fern_i18n::Strings) -> String {
    if error.is::<AttachmentTooLarge>() {
        strings.attachment_size_limit.replace("{limit}", &(MAX_ATTACHMENT_BYTES / (1024 * 1024)).to_string())
    } else {
        format!("{error:#}")
    }
}

/// Sends one sync request and returns the server's answer. Blocking: run it off the UI thread.
pub fn post(config: &SyncConfig, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    post_with_limit(&sync_agent(REQUEST_TIMEOUT), config, body, MAX_SYNC_RESPONSE_BYTES)
}

fn post_with_limit(
    agent: &ureq::Agent,
    config: &SyncConfig,
    body: &serde_json::Value,
    max_bytes: u64,
) -> anyhow::Result<serde_json::Value> {
    let url = format!("{}/sync", config.server.trim().trim_end_matches('/'));
    let mut response = match agent
        .post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("Authorization", &format!("Bearer {}", config.token))
        .header("Content-Type", "application/json")
        .send(body.to_string())
    {
        Ok(response) => response,
        Err(err) => {
            crate::diagnostics::log_event(
                "sync",
                crate::diagnostics::redact_sensitive(&format!("POST /sync failed: {err}")),
            );
            return Err(err.into());
        }
    };

    let status = response.status();
    let status_code = status.as_u16();

    if status_code == 426 {
        let error_text = read_error_body(&mut response, max_bytes.min(MAX_ERROR_BODY_BYTES));
        let parsed = serde_json::from_str::<serde_json::Value>(&error_text).ok();
        let server = parsed
            .as_ref()
            .and_then(|v| {
                v.get("server_protocol")
                    .or_else(|| v.get("protocol"))
                    .and_then(serde_json::Value::as_u64)
            })
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let min_server = parsed
            .as_ref()
            .and_then(|v| v.get("min_protocol").and_then(serde_json::Value::as_u64))
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(MIN_SYNC_PROTOCOL_VERSION);
        let mismatch = IncompatibleProtocol {
            client: SYNC_PROTOCOL_VERSION,
            server,
            min_server,
        };
        crate::diagnostics::log_event("sync", format!("protocol mismatch (HTTP 426): {mismatch}"));
        return Err(anyhow::Error::new(mismatch));
    }

    if !status.is_success() {
        let error_text = read_error_body(&mut response, max_bytes.min(MAX_ERROR_BODY_BYTES));
        let json_error = serde_json::from_str::<serde_json::Value>(&error_text)
            .ok()
            .and_then(|v| v.get("error").and_then(serde_json::Value::as_str).map(str::to_owned));
        let detail = match json_error {
            Some(msg) if !msg.trim().is_empty() => format!("HTTP {status_code}: {}", msg.trim()),
            _ => {
                let reason = status.canonical_reason().unwrap_or("error");
                let trimmed = error_text.trim();
                if trimmed.is_empty() {
                    format!("HTTP {status_code} {reason}")
                } else {
                    let snippet: String = trimmed.chars().take(200).collect();
                    format!("HTTP {status_code} {reason}: {snippet}")
                }
            }
        };
        crate::diagnostics::log_event(
            "sync",
            crate::diagnostics::redact_sensitive(&format!("POST /sync error: {detail}")),
        );
        anyhow::bail!("{detail}");
    }

    let text = match response
        .body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_string()
    {
        Ok(text) => text,
        Err(err) => {
            crate::diagnostics::log_event(
                "sync",
                crate::diagnostics::redact_sensitive(&format!("POST /sync body read error: {err}")),
            );
            return Err(err.into());
        }
    };
    match serde_json::from_str(&text) {
        Ok(value) => Ok(value),
        Err(err) => {
            crate::diagnostics::log_event(
                "sync",
                crate::diagnostics::redact_sensitive(&format!("POST /sync invalid JSON: {err}")),
            );
            Err(err.into())
        }
    }
}

fn read_error_body(response: &mut ureq::http::Response<ureq::Body>, limit: u64) -> String {
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

#[derive(Default)]
pub struct FileTransfers {
    pub synced: Vec<String>,
    pub downloaded: bool,
    pub failed: Vec<(String, anyhow::Error)>,
}

/// Moves attached files after a sync. Successful transfers are recorded even if others fail.
/// Blocking: run it off the UI thread.
pub fn transfer_files(config: &SyncConfig, dir: &Path, upload: &[String], download: &[String]) -> FileTransfers {
    transfer_files_with_agent(&sync_agent(REQUEST_TIMEOUT), config, dir, upload, download)
}

/// Best-effort deletion of remote attachment blobs via `DELETE /files/<key>` so removed
/// attachments are reclaimed promptly on servers that support file deletion.
/// Blocking: run it off the UI thread.
pub fn delete_remote_files(config: &SyncConfig, names: &[String]) {
    delete_remote_files_with_agent(&sync_agent(REQUEST_TIMEOUT), config, names);
}

fn delete_remote_files_with_agent(agent: &ureq::Agent, config: &SyncConfig, names: &[String]) {
    if !config.is_configured() || names.is_empty() {
        return;
    }
    let files_url = format!("{}/files", config.server.trim().trim_end_matches('/'));
    let auth = format!("Bearer {}", config.token);
    for name in names {
        if validate_file_name(name).is_err() {
            continue;
        }
        let url = format!("{files_url}/{}", file_key(name));
        match agent
            .delete(&url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("Authorization", &auth)
            .call()
        {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if !resp.status().is_success() && code != 404 && code != 405 {
                    crate::diagnostics::log_event(
                        "sync",
                        format!("DELETE /files returned HTTP {code}"),
                    );
                }
            }
            Err(err) => {
                crate::diagnostics::log_event(
                    "sync",
                    crate::diagnostics::redact_sensitive(&format!("DELETE /files failed: {err}")),
                );
            }
        }
    }
}

fn transfer_files_with_agent(
    agent: &ureq::Agent,
    config: &SyncConfig,
    dir: &Path,
    upload: &[String],
    download: &[String],
) -> FileTransfers {
    let files_url = format!("{}/files", config.server.trim().trim_end_matches('/'));
    let auth = format!("Bearer {}", config.token);
    let mut outcome = FileTransfers::default();
    let e2ee_key = if config.passphrase.is_empty() {
        None
    } else {
        match E2eeKey::from_passphrase(&config.passphrase) {
            Ok(key) => Some(key),
            Err(err) => {
                crate::diagnostics::log_event("sync", format!("E2EE key derivation failed: {err}"));
                for name in upload.iter().chain(download.iter()) {
                    outcome.failed.push((name.clone(), anyhow::Error::new(err.clone())));
                }
                return outcome;
            }
        }
    };
    for name in upload {
        let sent = (|| -> anyhow::Result<()> {
            validate_file_name(name)?;
            let file = std::fs::File::open(dir.join(name))?;
            check_attachment_size(file.metadata()?.len())?;
            let mut data = Vec::new();
            file.take(MAX_ATTACHMENT_BYTES + 1).read_to_end(&mut data)?;
            check_attachment_size(data.len() as u64)?;
            let payload = match &e2ee_key {
                Some(key) => key.encrypt_file(name, &data)?,
                None => data,
            };
            let url = format!("{files_url}/{}", file_key(name));
            agent.put(&url).header("Authorization", &auth).send(&payload[..])?;
            Ok(())
        })();
        match sent {
            Ok(()) => outcome.synced.push(name.clone()),
            Err(err) => {
                crate::diagnostics::log_event(
                    "sync",
                    crate::diagnostics::redact_sensitive(&format!("attachment upload failed: {err:#}")),
                );
                outcome.failed.push((name.clone(), err));
            }
        }
    }
    for name in download {
        let fetched = download_file(agent, &files_url, &auth, name).and_then(|data| {
            let plain = match (&e2ee_key, e2ee::is_encrypted_file(&data)) {
                (Some(key), _) => key.decrypt_file(name, &data)?,
                (None, true) => return Err(anyhow::Error::new(E2eeError::PassphraseRequired)),
                (None, false) => data,
            };
            check_attachment_size(plain.len() as u64)?;
            save_download(dir, name, &plain)
        });
        match fetched {
            Ok(()) => {
                outcome.downloaded = true;
                outcome.synced.push(name.clone());
            }
            Err(err) => {
                crate::diagnostics::log_event(
                    "sync",
                    crate::diagnostics::redact_sensitive(&format!("attachment download failed: {err:#}")),
                );
                outcome.failed.push((name.clone(), err));
            }
        }
    }
    outcome
}

fn validate_file_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.contains(['/', '\\', '\0'])
            && !(cfg!(windows) && name.contains(':'))
            && Path::new(name).file_name().is_some_and(|part| part == name),
        "invalid attachment name"
    );
    Ok(())
}

fn download_file(agent: &ureq::Agent, files_url: &str, auth: &str, name: &str) -> anyhow::Result<Vec<u8>> {
    validate_file_name(name)?;
    let key = file_key(name);
    let strict = encode_file_key(name, false);
    let mut candidates = vec![key.as_str()];
    if strict.len() <= MAX_FILE_KEY_BYTES && !candidates.contains(&strict.as_str()) {
        candidates.push(&strict);
    }
    // Older releases sent the literal path. The server recorded it verbatim, so a miss on the
    // canonical key can be an attachment already uploaded by another client.
    if name.is_ascii()
        && name.len() <= MAX_FILE_KEY_BYTES
        && !name.contains(['?', '#', ':'])
        && !name.bytes().any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        && !candidates.contains(&name)
    {
        candidates.push(name);
    }
    let download_limit = MAX_ATTACHMENT_BYTES + E2EE_FILE_OVERHEAD;
    for candidate in candidates {
        let url = format!("{files_url}/{candidate}");
        match agent.get(&url).header("Authorization", auth).call() {
            Ok(mut response) => return Ok(response.body_mut().with_config().limit(download_limit).read_to_vec()?),
            Err(ureq::Error::StatusCode(404)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ureq::Error::StatusCode(404).into())
}

#[cfg(test)]
mod tests {
    use fern_core::Store;

    use super::*;

    #[test]
    fn attachment_keys_round_trip_with_previous_clients() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-sync-compat-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let http = tiny_http::Server::http("127.0.0.1:0").map_err(anyhow::Error::msg)?;
        let port = http.server_addr().to_ip().unwrap().port();
        let token = "attachment-compatibility-test";
        let server = fern_server::Server::open(&dir.join("server.db"))?;
        std::thread::spawn(move || fern_server::serve(&http, server, token));
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}"), token: token.to_owned(), ..Default::default() };
        let agent = sync_agent(REQUEST_TIMEOUT);
        let legacy_url = |key: &str| format!("{}/files/{key}", config.server);
        let auth = format!("Bearer {token}");
        let existing = "old-Plan(v2)+notes.txt";
        let escaped = "existing-Plan(v2)+notes.txt";
        let created = "new-Plan(v2)+notes.txt";
        let long = format!("{}.txt", "Ж".repeat(55));
        assert!(encode_file_key(&long, true).len() > MAX_FILE_KEY_BYTES);

        agent.put(legacy_url(existing)).header("Authorization", &auth).send(&b"old"[..])?;
        let escaped_key = encode_file_key(escaped, false);
        agent.put(legacy_url(&escaped_key)).header("Authorization", &auth).send(&b"escaped"[..])?;
        let sender = dir.join("sender");
        let receiver = dir.join("receiver");
        std::fs::create_dir(&sender)?;
        std::fs::write(sender.join(created), "new")?;
        std::fs::write(sender.join(&long), "long name")?;

        let sent = transfer_files(&config, &sender, &[created.into(), long.clone()], &[]);
        assert!(sent.failed.is_empty(), "{:?}", sent.failed);
        assert_eq!(sent.synced.len(), 2);
        let legacy_read = agent.get(legacy_url(created)).header("Authorization", &auth).call()?.body_mut().read_to_string()?;
        assert_eq!(legacy_read, "new");

        let received = transfer_files(&config, &receiver, &[], &[existing.into(), escaped.into(), long.clone()]);
        assert!(received.failed.is_empty(), "{:?}", received.failed);
        assert_eq!(std::fs::read_to_string(receiver.join(existing))?, "old");
        assert_eq!(std::fs::read_to_string(receiver.join(escaped))?, "escaped");
        assert_eq!(std::fs::read_to_string(receiver.join(long))?, "long name");
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn stalled_attachment_body_times_out_without_blocking_the_batch() -> anyhow::Result<()> {
        use std::io::{BufRead as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let config = SyncConfig { server: format!("http://{}", listener.local_addr()?), token: "test".into(), ..Default::default() };
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || -> std::io::Result<()> {
            let (mut stalled, _) = listener.accept()?;
            stalled.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut request = std::io::BufReader::new(&mut stalled);
            loop {
                let mut line = String::new();
                if request.read_line(&mut line)? == 0 || line == "\r\n" {
                    break;
                }
            }
            // The server sends response headers and one byte but never completes the body.
            stalled.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\na")?;
            let _ = wait.recv_timeout(Duration::from_secs(5));
            Ok(())
        });
        let dir = std::env::temp_dir().join(format!("fern-sync-timeout-{}", uuid::Uuid::now_v7()));
        let agent = sync_agent(Duration::from_millis(250));
        let start = std::time::Instant::now();
        let result = transfer_files_with_agent(&agent, &config, &dir, &[], &["incomplete".into()]);
        release.send(()).ok();
        server.join().unwrap()?;
        assert!(start.elapsed() < Duration::from_secs(4), "the client must return before the server closes the body");
        assert_eq!(result.failed.len(), 1);
        let error = &result.failed[0].1;
        assert!(
            matches!(error.downcast_ref::<ureq::Error>(), Some(ureq::Error::Timeout(_)))
                || error.chain().any(|cause| cause.downcast_ref::<std::io::Error>().is_some_and(|error|
                    matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))),
            "expected a timeout, got {error:#}"
        );
        assert!(result.synced.is_empty());
        assert!(!dir.join("incomplete").exists());
        Ok(())
    }

    /// Two stores sync through a real fern-server over HTTP, the way the app does.
    #[test]
    fn sync_over_http() {
        let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = http.server_addr().to_ip().unwrap().port();
        let token = "a-token-for-the-test-only";
        std::thread::spawn(move || fern_server::serve(&http, fern_server::Server::open_in_memory().unwrap(), token));
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}/"), token: token.to_owned(), ..Default::default() };

        let sync = |store: &Store| {
            let request = store.sync_request().unwrap();
            let answer = post(&config, &request.body).unwrap();
            store.finish_sync(&request, &answer).unwrap()
        };
        let laptop = Store::open_in_memory().unwrap();
        let phone = Store::open_in_memory().unwrap();
        let task = laptop.add_task("Renew the passport").unwrap();
        sync(&laptop);
        assert_eq!(sync(&phone), 1);
        assert_eq!(phone.task(task.id).unwrap().unwrap().title, "Renew the passport");

        let wrong = SyncConfig { token: "wrong".into(), ..config.clone() };
        assert!(post(&wrong, &laptop.sync_request().unwrap().body).is_err(), "a wrong token is refused");
    }

    #[test]
    fn independent_filter_rules_roundtrip_through_current_server() -> anyhow::Result<()> {
        use fern_core::{Condition, ConditionValue as V, DateRule, FilterRules, SmartFilter, View};

        let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = http.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            fern_server::serve(&http, fern_server::Server::open_in_memory().unwrap(), "filter-test")
        });
        let config = SyncConfig {
            server: format!("http://127.0.0.1:{port}"),
            token: "filter-test".into(),
            ..Default::default()
        };
        let sync = |store: &Store| -> anyhow::Result<_> {
            let request = store.sync_request()?;
            store.finish_sync(&request, &post(&config, &request.body)?)
        };
        let a = Store::open_in_memory()?;
        let b = Store::open_in_memory()?;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let task = a.add_task("Both tags")?;
        a.set_tags(task.id, &["work".into(), "urgent".into()])?;
        a.set_due(task.id, Some(today), None)?;
        let tags = |name: &str| Condition::new(V::Tags {
            names: vec![name.into()],
            untagged: false,
        });
        let mut filter = SmartFilter {
            name: "Synced filter".into(),
            rules: FilterRules {
                advanced: true,
                conditions: vec![
                    tags("work"),
                    tags("urgent"),
                    Condition::new(V::Date(vec![
                        DateRule::Overdue,
                        DateRule::Span { from: Some(0), to: Some(13) },
                    ])),
                ],
                ..FilterRules::default()
            },
            ..SmartFilter::new()
        };
        let view = View::Filter(filter.id);
        a.save_filter(&filter)?;
        sync(&a)?;
        sync(&b)?;
        assert_eq!(b.filters()?, [filter.clone()]);
        let matching: Vec<_> = b.tasks_in(&view, today)?.into_iter().map(|t| t.id).collect();
        assert_eq!(matching, [task.id]);
        filter.rules.conditions[1].not = true;
        b.save_filter(&filter)?;
        sync(&b)?;
        sync(&a)?;
        assert_eq!(a.filters()?, [filter]);
        assert!(a.tasks_in(&view, today)?.is_empty());
        Ok(())
    }

    /// A failed file leaves the successful transfers available and can be retried.
    #[test]
    fn attachments_over_http() {
        let dir = std::env::temp_dir().join(format!("fern-sync-http-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = http.server_addr().to_ip().unwrap().port();
        let token = "a-token-for-the-test-only";
        let server = fern_server::Server::open(&dir.join("server.db")).unwrap();
        std::thread::spawn(move || fern_server::serve(&http, server, token));
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}"), token: token.to_owned(), ..Default::default() };

        let sync = |store: &Store, files: &std::path::Path| {
            let request = store.sync_request().unwrap();
            store.finish_sync(&request, &post(&config, &request.body).unwrap()).unwrap();
            let (upload, download) = store.files_to_sync(files).unwrap();
            let result = transfer_files(&config, files, &upload, &download);
            store.mark_files_synced(&result.synced).unwrap();
            result
        };
        let (laptop_files, phone_files) = (dir.join("laptop"), dir.join("phone"));
        let source = dir.join("Билеты на поезд");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("ticket.pdf"), b"%PDF ticket").unwrap();
        let legacy_source = dir.join("plan.txt");
        std::fs::write(&legacy_source, b"plan").unwrap();
        let laptop = Store::open_in_memory().unwrap();
        let phone = Store::open_in_memory().unwrap();
        let task = laptop.add_task("Flight").unwrap();
        let archive = laptop.add_attachment(task.id, &source, &laptop_files).unwrap();
        let legacy = laptop.add_attachment(task.id, &legacy_source, &laptop_files).unwrap();
        let legacy_path = legacy.path(&laptop_files);
        let legacy_name = legacy_path.file_name().unwrap().to_str().unwrap();
        std::fs::OpenOptions::new().write(true).open(&legacy_path).unwrap()
            .set_len(MAX_ATTACHMENT_BYTES + 1).unwrap();

        let sent = sync(&laptop, &laptop_files);
        assert_eq!(sent.synced.len(), 1);
        assert_eq!(sent.failed.len(), 1);
        assert_eq!(sent.failed[0].0, legacy_name);
        assert!(sent.failed[0].1.is::<AttachmentTooLarge>());
        assert_eq!(laptop.files_to_sync(&laptop_files).unwrap().0, [legacy_name]);

        let fetched = sync(&phone, &phone_files);
        assert!(fetched.downloaded);
        assert_eq!(fetched.synced.len(), 1);
        assert_eq!(fetched.failed.len(), 1);
        assert_eq!(fetched.failed[0].0, legacy_name);
        assert!(!legacy.path(&phone_files).exists());
        assert_eq!(
            std::fs::read(archive.path(&phone_files)).unwrap(),
            std::fs::read(archive.path(&laptop_files)).unwrap(),
        );

        std::fs::copy(&legacy_source, &legacy_path).unwrap();
        assert!(sync(&laptop, &laptop_files).failed.is_empty());
        assert!(sync(&phone, &phone_files).failed.is_empty());
        assert_eq!(std::fs::read(legacy.path(&phone_files)).unwrap(), b"plan");
        assert_eq!(phone.files_to_sync(&phone_files).unwrap(), (vec![], vec![]));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn post_rejects_oversized_response_body() -> anyhow::Result<()> {
        let http = tiny_http::Server::http("127.0.0.1:0").map_err(anyhow::Error::msg)?;
        let port = http.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok(req) = http.recv() {
                let big = format!(r#"{{"data":"{}"}}"#, "x".repeat(4096));
                let _ = req.respond(tiny_http::Response::from_string(big));
            }
        });
        let config = SyncConfig {
            server: format!("http://127.0.0.1:{port}"),
            token: "tok".into(),
            ..Default::default()
        };
        let agent = sync_agent(Duration::from_secs(5));
        let err = post_with_limit(&agent, &config, &serde_json::json!({}), 256);
        let _ = handle.join();
        assert!(err.is_err(), "oversized sync response must be rejected by limit");
        Ok(())
    }

    #[test]
    fn post_parses_http_426_into_incompatible_protocol() -> anyhow::Result<()> {
        let http = tiny_http::Server::http("127.0.0.1:0").map_err(anyhow::Error::msg)?;
        let port = http.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok(req) = http.recv() {
                let body = r#"{"error":"incompatible_protocol","server_protocol":5,"min_protocol":3,"client_protocol":2}"#;
                let header = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(426)
                    .with_header(header);
                let _ = req.respond(resp);
            }
        });
        let config = SyncConfig {
            server: format!("http://127.0.0.1:{port}"),
            token: "tok".into(),
            ..Default::default()
        };
        let err = post(&config, &serde_json::json!({ "protocol": SYNC_PROTOCOL_VERSION }))
            .expect_err("HTTP 426 must return IncompatibleProtocol error");
        let _ = handle.join();

        let incompatible = is_incompatible_protocol(&err)
            .expect("error must downcast to IncompatibleProtocol");
        assert_eq!(
            incompatible,
            &IncompatibleProtocol {
                client: SYNC_PROTOCOL_VERSION,
                server: 5,
                min_server: 3,
            }
        );
        // Also verify helper finds IncompatibleProtocol through anyhow::Context wrapping.
        let wrapped = err.context("sync request failed");
        assert_eq!(is_incompatible_protocol(&wrapped), Some(&IncompatibleProtocol {
            client: SYNC_PROTOCOL_VERSION,
            server: 5,
            min_server: 3,
        }));
        Ok(())
    }

    #[test]
    fn post_reports_status_code_and_json_error_for_other_non_2xx() -> anyhow::Result<()> {
        let http = tiny_http::Server::http("127.0.0.1:0").map_err(anyhow::Error::msg)?;
        let port = http.server_addr().to_ip().unwrap().port();
        let handle = std::thread::spawn(move || {
            if let Ok(req) = http.recv() {
                let body = r#"{"error":"database temporarily locked"}"#;
                let header = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(503)
                    .with_header(header);
                let _ = req.respond(resp);
            }
        });
        let config = SyncConfig {
            server: format!("http://127.0.0.1:{port}"),
            token: "tok".into(),
            ..Default::default()
        };
        let err = post(&config, &serde_json::json!({}))
            .expect_err("HTTP 503 must return an error");
        let _ = handle.join();
        assert!(is_incompatible_protocol(&err).is_none());
        let msg = format!("{err:#}");
        assert!(msg.contains("503"), "expected status code in: {msg}");
        assert!(msg.contains("database temporarily locked"), "expected JSON error in: {msg}");
        Ok(())
    }

    #[test]
    fn e2ee_attachments_roundtrip_and_reject_wrong_or_missing_passphrase() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!("fern-sync-e2ee-files-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        let http = tiny_http::Server::http("127.0.0.1:0").map_err(anyhow::Error::msg)?;
        let port = http.server_addr().to_ip().unwrap().port();
        let token = "e2ee-files-token";
        let server_db = dir.join("server.db");
        let server = fern_server::Server::open(&server_db)?;
        std::thread::spawn(move || fern_server::serve(&http, server, token));

        let sender_dir = dir.join("sender");
        let good_dir = dir.join("good_receiver");
        let wrong_dir = dir.join("wrong_receiver");
        let missing_dir = dir.join("missing_receiver");
        let unencrypted_dir = dir.join("unencrypted_receiver");
        std::fs::create_dir_all(&sender_dir)?;

        let attachment_name = "0199-secret-notes.pdf";
        let plaintext = b"%PDF-1.7 confidential attachment payload";
        std::fs::write(sender_dir.join(attachment_name), plaintext)?;

        let sender_config = SyncConfig {
            server: format!("http://127.0.0.1:{port}"),
            token: token.to_owned(),
            passphrase: "correct-horse-battery-staple".to_owned(),
        };
        let sent = transfer_files(&sender_config, &sender_dir, &[attachment_name.to_owned()], &[]);
        assert!(sent.failed.is_empty(), "{:?}", sent.failed);
        assert_eq!(sent.synced, vec![attachment_name.to_owned()]);

        // Verify the remote blob stored on the server is encrypted and does not contain plaintext.
        let stored_blob = std::fs::read(
            dir.join("server.db.files").join(file_key(attachment_name)),
        )?;
        assert!(e2ee::is_encrypted_file(&stored_blob), "uploaded file must carry FERNE2EE header");
        assert!(
            !stored_blob.windows(plaintext.len()).any(|w| w == plaintext),
            "uploaded ciphertext must not contain plaintext bytes"
        );

        // 1. Matching passphrase decrypts cleanly.
        let good = transfer_files(&sender_config, &good_dir, &[], &[attachment_name.to_owned()]);
        assert!(good.failed.is_empty(), "{:?}", good.failed);
        assert!(good.downloaded);
        assert_eq!(std::fs::read(good_dir.join(attachment_name))?, plaintext);

        // 2. Wrong passphrase fails with E2eeError::WrongPassphrase.
        let wrong_config = SyncConfig {
            passphrase: "wrong-passphrase".to_owned(),
            ..sender_config.clone()
        };
        let wrong = transfer_files(&wrong_config, &wrong_dir, &[], &[attachment_name.to_owned()]);
        assert!(wrong.synced.is_empty());
        assert_eq!(wrong.failed.len(), 1);
        assert_eq!(is_e2ee_error(&wrong.failed[0].1), Some(&E2eeError::WrongPassphrase));
        assert!(!wrong_dir.join(attachment_name).exists());

        // 3. Missing passphrase fails with E2eeError::PassphraseRequired.
        let missing_config = SyncConfig {
            passphrase: String::new(),
            ..sender_config.clone()
        };
        let missing = transfer_files(&missing_config, &missing_dir, &[], &[attachment_name.to_owned()]);
        assert!(missing.synced.is_empty());
        assert_eq!(missing.failed.len(), 1);
        assert_eq!(is_e2ee_error(&missing.failed[0].1), Some(&E2eeError::PassphraseRequired));
        assert!(!missing_dir.join(attachment_name).exists());

        // 4. Plaintext file on server fails with E2eeError::Malformed when E2EE is enabled.
        let plain_name = "0199-plain.txt";
        std::fs::write(sender_dir.join(plain_name), b"not encrypted")?;
        let plain_sent = transfer_files(&missing_config, &sender_dir, &[plain_name.to_owned()], &[]);
        assert!(plain_sent.failed.is_empty(), "{:?}", plain_sent.failed);
        let downgrade = transfer_files(&sender_config, &unencrypted_dir, &[], &[plain_name.to_owned()]);
        assert_eq!(downgrade.failed.len(), 1);
        assert!(matches!(is_e2ee_error(&downgrade.failed[0].1), Some(E2eeError::Malformed(_))));
        assert!(!unencrypted_dir.join(plain_name).exists());

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn delete_remote_files_sends_authenticated_delete_and_skips_invalid_names() -> anyhow::Result<()> {
        let http = tiny_http::Server::http("127.0.0.1:0").map_err(anyhow::Error::msg)?;
        let port = http.server_addr().to_ip().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok(req) = http.recv() {
                    let method = req.method().as_str().to_owned();
                    let url = req.url().to_owned();
                    let auth = req
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.as_str().to_owned());
                    let _ = tx.send((method, url, auth));
                    let _ = req.respond(tiny_http::Response::from_data(Vec::new()).with_status_code(204));
                }
            }
        });

        let config = SyncConfig {
            server: format!("http://127.0.0.1:{port}"),
            token: "del-token".into(),
            ..Default::default()
        };
        delete_remote_files(
            &config,
            &[
                "../escape.txt".to_owned(),
                "0199-first.txt".to_owned(),
                "0199-with space.txt".to_owned(),
            ],
        );
        let _ = handle.join();

        let recorded: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            recorded,
            vec![
                (
                    "DELETE".to_owned(),
                    format!("/files/{}", file_key("0199-first.txt")),
                    Some("Bearer del-token".to_owned()),
                ),
                (
                    "DELETE".to_owned(),
                    format!("/files/{}", file_key("0199-with space.txt")),
                    Some("Bearer del-token".to_owned()),
                ),
            ]
        );
        Ok(())
    }
}
