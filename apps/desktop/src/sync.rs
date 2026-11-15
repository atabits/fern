//! Syncing with the user's own `fern-server`: the request and the answer travel as JSON over
//! HTTP(S); what they hold is the store's business (`fern_core::sync`).

use chrono::NaiveDateTime;
use fern_core::attachment::{AttachmentTooLarge, MAX_ATTACHMENT_BYTES, check_attachment_size, save_download};
use fern_core::sync::{MAX_FILE_KEY_BYTES, SyncConfig};
use sha2::{Digest as _, Sha256};
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

fn sync_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).build().into()
}

pub fn attachment_error(error: &anyhow::Error, strings: &crate::i18n::Strings) -> String {
    if error.is::<AttachmentTooLarge>() {
        strings.attachment_size_limit.replace("{limit}", &(MAX_ATTACHMENT_BYTES / (1024 * 1024)).to_string())
    } else {
        format!("{error:#}")
    }
}

/// Sends one sync request and returns the server's answer. Blocking: run it off the UI thread.
pub fn post(config: &SyncConfig, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let url = format!("{}/sync", config.server.trim().trim_end_matches('/'));
    let mut response = sync_agent(REQUEST_TIMEOUT).post(&url)
        .header("Authorization", &format!("Bearer {}", config.token))
        .header("Content-Type", "application/json")
        .send(body.to_string())?;
    let text = response.body_mut().read_to_string()?;
    Ok(serde_json::from_str(&text)?)
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

fn transfer_files_with_agent(agent: &ureq::Agent, config: &SyncConfig, dir: &Path, upload: &[String], download: &[String]) -> FileTransfers {
    let files_url = format!("{}/files", config.server.trim().trim_end_matches('/'));
    let auth = format!("Bearer {}", config.token);
    let mut outcome = FileTransfers::default();
    for name in upload {
        let sent = (|| -> anyhow::Result<()> {
            validate_file_name(name)?;
            let file = std::fs::File::open(dir.join(name))?;
            check_attachment_size(file.metadata()?.len())?;
            let mut data = Vec::new();
            file.take(MAX_ATTACHMENT_BYTES + 1).read_to_end(&mut data)?;
            check_attachment_size(data.len() as u64)?;
            let url = format!("{files_url}/{}", file_key(name));
            agent.put(&url).header("Authorization", &auth).send(&data[..])?;
            Ok(())
        })();
        match sent {
            Ok(()) => outcome.synced.push(name.clone()),
            Err(err) => outcome.failed.push((name.clone(), err)),
        }
    }
    for name in download {
        let fetched = download_file(agent, &files_url, &auth, name).and_then(|data| save_download(dir, name, &data));
        match fetched {
            Ok(()) => {
                outcome.downloaded = true;
                outcome.synced.push(name.clone());
            }
            Err(err) => outcome.failed.push((name.clone(), err)),
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
    if name.is_ascii() && name.len() <= MAX_FILE_KEY_BYTES
        && !name.contains(['?', '#', ':']) && !name.bytes().any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        && !candidates.contains(&name)
    {
        candidates.push(name);
    }
    for candidate in candidates {
        let url = format!("{files_url}/{candidate}");
        match agent.get(&url).header("Authorization", auth).call() {
            Ok(mut response) => return Ok(response.body_mut().with_config().limit(MAX_ATTACHMENT_BYTES).read_to_vec()?),
            Err(ureq::Error::StatusCode(404)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ureq::Error::StatusCode(404).into())
}

fn file_key(name: &str) -> String {
    let encoded = encode_file_key(name, true);
    if encoded.len() <= MAX_FILE_KEY_BYTES {
        encoded
    } else {
        // Escaping UTF-8 can exceed the server's component limit even for a valid local name.
        // The names already contain unique IDs. Only the storage key changes; sync metadata and
        // downloaded file names retain the original name. This works with older servers too.
        format!("~sha256-{:x}", Sha256::digest(name.as_bytes()))
    }
}

fn encode_file_key(name: &str, legacy_punctuation: bool) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(name.len());
    for byte in name.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        // Preserve the legacy keys for URL punctuation such as '+', '(' and ')'. ':' is
        // escaped as it has special meaning in native Windows paths.
        let punctuation = b"!$&'()*+,;=@[]{}".contains(&byte);
        if unreserved || (legacy_punctuation && punctuation) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
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
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}"), token: token.to_owned() };
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
        let config = SyncConfig { server: format!("http://{}", listener.local_addr()?), token: "test".into() };
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
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}/"), token: token.to_owned() };

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
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}"), token: token.to_owned() };

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
}
