use std::io::Write as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fern_core::Store;

use super::*;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file_with_age(path: &Path, data: &[u8], age: Duration) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(data).unwrap();
    if !age.is_zero() {
        let mtime = SystemTime::now().checked_sub(age).unwrap();
        file.set_modified(mtime).unwrap();
    }
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();

    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(tok) = token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("HTTP response should contain header/body separator");
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let status: u16 = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, raw[split + 4..].to_vec())
}

struct TestHttpServer {
    http: Arc<tiny_http::Server>,
    port: u16,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl TestHttpServer {
    fn start(server: Server, token: &'static str, gc_grace: Duration) -> Self {
        let http = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let port = http.server_addr().to_ip().unwrap().port();
        let http_thread = Arc::clone(&http);
        let handle = std::thread::spawn(move || serve_with_gc_grace(&http_thread, server, token, gc_grace));
        Self { http, port, handle: Some(handle) }
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.http.unblock();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A device's sync through the server, the way the app does it.
fn sync(server: &mut Server, store: &Store) {
    let request = store.sync_request().unwrap();
    let answer = server.sync(&request.body).unwrap();
    store.finish_sync(&request, &answer).unwrap();
}

#[test]
fn devices_sync_through_the_server() {
    let mut server = Server::open_in_memory().unwrap();
    let laptop = Store::open_in_memory().unwrap();
    let phone = Store::open_in_memory().unwrap();

    let task = laptop.add_task("Call the bank").unwrap();
    sync(&mut server, &laptop);
    sync(&mut server, &phone);
    assert_eq!(phone.task(task.id).unwrap().unwrap().title, "Call the bank");

    phone.set_completed(task.id, true).unwrap();
    sync(&mut server, &phone);
    sync(&mut server, &laptop);
    assert!(laptop.task(task.id).unwrap().unwrap().is_completed());

    // Nothing new: nothing comes back, and a device never gets its own changes.
    let answer = server.sync(&laptop.sync_request().unwrap().body).unwrap();
    assert!(answer["changes"].as_array().unwrap().is_empty());
    assert_eq!(answer["protocol"], SYNC_PROTOCOL_VERSION);
    assert_eq!(answer["min_protocol"], MIN_SYNC_PROTOCOL_VERSION);
}

#[test]
fn an_older_change_does_not_replace_a_newer_one() {
    let mut server = Server::open_in_memory().unwrap();
    let change = |at: i64, title: &str| json!({ "entity": "tasks", "key": "t1", "at": at, "row": { "id": "t1", "title": title } });
    server.sync(&json!({ "device": "a", "changes": [change(20, "new")] })).unwrap();
    server.sync(&json!({ "device": "b", "changes": [change(10, "old")] })).unwrap();
    let answer = server.sync(&json!({ "device": "c", "since": 0 })).unwrap();
    assert_eq!(answer["changes"][0]["row"]["title"], "new");
    assert!(server.sync(&json!({ "since": 0 })).is_err(), "a device id is required");
}

#[test]
fn file_names() {
    let dir = unique_temp_dir("fern-server-files");
    let server = Server::open(&dir.join("server.db")).unwrap();
    assert!(server.file_path("0199-plan.txt").is_some());
    for bad in ["", "../x", "a/b", "a\\b", ".hidden", "C:foo", "foo:bar", "a\0b"] {
        assert!(server.file_path(bad).is_none(), "{bad:?}");
    }
    assert!(Server::open_in_memory().unwrap().file_path("x").is_none(), "no file store in memory");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tokens() {
    assert!(authorized(Some("Bearer s3cret"), "s3cret"));
    assert!(!authorized(Some("Bearer s3cre"), "s3cret"));
    assert!(!authorized(Some("s3cret"), "s3cret"));
    assert!(!authorized(None, "s3cret"));
}

#[test]
fn protocol_versions_and_healthcheck_over_http() {
    let token = "test-token-at-least-16-chars";
    let server = Server::open_in_memory().unwrap();
    let http = TestHttpServer::start(server, token, DEFAULT_GC_GRACE);

    // Unauthenticated GET /health returns 200 with version & protocol metadata.
    let (status, body) = http_request(http.port, "GET", "/health", None, b"");
    assert_eq!(status, 200);
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(health["protocol"], SYNC_PROTOCOL_VERSION);
    assert_eq!(health["min_protocol"], MIN_SYNC_PROTOCOL_VERSION);

    // Legacy v1 (omitted protocol), explicit v1, and v2 are accepted and echo protocol range.
    for payload in [
        json!({ "device": "legacy", "since": 0, "changes": [] }),
        json!({ "protocol": 1, "device": "v1", "since": 0, "changes": [] }),
        json!({ "protocol": 2, "device": "v2", "since": 0, "changes": [] }),
    ] {
        let (status, body) = http_request(http.port, "POST", "/sync", Some(token), payload.to_string().as_bytes());
        assert_eq!(status, 200, "payload {payload}");
        let answer: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(answer["protocol"], SYNC_PROTOCOL_VERSION);
        assert_eq!(answer["min_protocol"], MIN_SYNC_PROTOCOL_VERSION);
    }

    // Unsupported future protocol v99 returns HTTP 426 with structured JSON.
    let v99 = json!({ "protocol": 99, "device": "future", "since": 0, "changes": [] });
    let (status, body) = http_request(http.port, "POST", "/sync", Some(token), v99.to_string().as_bytes());
    assert_eq!(status, 426);
    let err_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err_json["error"], "incompatible_protocol");
    assert_eq!(err_json["server_protocol"], SYNC_PROTOCOL_VERSION);
    assert_eq!(err_json["min_protocol"], MIN_SYNC_PROTOCOL_VERSION);
    assert_eq!(err_json["client_protocol"], 99);

    // Unsupported protocol 0 also returns HTTP 426.
    let v0 = json!({ "protocol": 0, "device": "zero", "since": 0, "changes": [] });
    let (status, body) = http_request(http.port, "POST", "/sync", Some(token), v0.to_string().as_bytes());
    assert_eq!(status, 426);
    let err_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err_json["error"], "incompatible_protocol");
    assert_eq!(err_json["client_protocol"], 0);

    // Direct Server::sync returns typed ProtocolMismatchError.
    let mut direct = Server::open_in_memory().unwrap();
    let err = direct.sync(&v99).unwrap_err();
    let mismatch = err.downcast_ref::<ProtocolMismatchError>().expect("typed ProtocolMismatchError");
    assert_eq!(
        mismatch,
        &ProtocolMismatchError {
            client: 99,
            server: SYNC_PROTOCOL_VERSION,
            min_server: MIN_SYNC_PROTOCOL_VERSION,
        }
    );
}

#[test]
fn gc_files_deletes_old_orphans_and_preserves_live_and_recent_files() {
    let dir = unique_temp_dir("fern-server-gc");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    let mut server = Server::open(&db_path).unwrap();

    let old_age = Duration::from_secs(48 * 3600);
    let grace = Duration::from_secs(24 * 3600);

    // 1. Live plaintext attachment (via stored_name)
    let plain_stored = "att1-Plan(v2) notes.pdf";
    let plain_stored_key = file_key(plain_stored);
    let plain_strict_alias = encode_file_key(plain_stored, false);
    assert_ne!(plain_stored_key, plain_strict_alias);

    // 2. Live plaintext attachment (via id + name)
    let plain_id_name = "att2-report.txt";
    let plain_id_name_key = file_key(plain_id_name);

    // 3. Live E2EE attachment (encrypted row + outer file_key)
    let e2ee_att_key = file_key("att3-secret.bin");

    // 4. Live countdown covers (plaintext background, plaintext cover_image, E2EE file_key)
    let cover_bg_key = file_key("cd1-beach.jpg");
    let cover_img_key = file_key("cd2-mountain.png");
    let cover_e2ee_key = file_key("cd3-encrypted-cover.jpg");

    // 5. Deleted attachment (row = null)
    let deleted_att_key = file_key("att4-deleted.pdf");

    server
        .sync(&json!({
            "protocol": 2,
            "device": "laptop",
            "since": 0,
            "changes": [
                {
                    "entity": "task_attachments",
                    "key": "att1",
                    "at": 100,
                    "row": { "id": "att1", "stored_name": plain_stored }
                },
                {
                    "entity": "task_attachments",
                    "key": "att2",
                    "at": 100,
                    "row": { "id": "att2", "task_id": "t1", "name": "report.txt", "size": 5, "created_at": 100 }
                },
                {
                    "entity": "task_attachments",
                    "key": "att3",
                    "at": 100,
                    "file_key": e2ee_att_key,
                    "row": { "__fern_e2ee": 1, "nonce": "AAAA", "ct": "BBBB" }
                },
                {
                    "entity": "countdowns",
                    "key": "cd1",
                    "at": 100,
                    "row": { "id": "cd1", "name": "Trip", "background": "cd1-beach.jpg" }
                },
                {
                    "entity": "countdowns",
                    "key": "cd2",
                    "at": 100,
                    "row": { "id": "cd2", "name": "Hike", "cover_image": "cd2-mountain.png" }
                },
                {
                    "entity": "countdowns",
                    "key": "cd3",
                    "at": 100,
                    "file_key": cover_e2ee_key,
                    "row": { "__fern_e2ee": 1, "nonce": "CCCC", "ct": "DDDD" }
                },
                {
                    "entity": "task_attachments",
                    "key": "att4",
                    "at": 100,
                    "file_key": deleted_att_key,
                    "row": null
                }
            ]
        }))
        .unwrap();

    // Populate .files directory with live files (all old), old orphans, and a recent orphan.
    write_file_with_age(&files_dir.join(&plain_stored_key), b"live1", old_age);
    write_file_with_age(&files_dir.join(&plain_strict_alias), b"live1-strict", old_age);
    write_file_with_age(&files_dir.join(&plain_id_name_key), b"live2", old_age);
    write_file_with_age(&files_dir.join(&e2ee_att_key), b"live-e2ee", old_age);
    write_file_with_age(&files_dir.join(&cover_bg_key), b"cover-bg", old_age);
    write_file_with_age(&files_dir.join(&cover_img_key), b"cover-img", old_age);
    write_file_with_age(&files_dir.join(&cover_e2ee_key), b"cover-e2ee", old_age);

    write_file_with_age(&files_dir.join(&deleted_att_key), b"deleted-att-bytes", old_age);
    write_file_with_age(&files_dir.join("orphan-old.dat"), b"0123456789", old_age);
    write_file_with_age(&files_dir.join("orphan-recent.dat"), b"in-flight-upload", Duration::ZERO);

    let stats = server.gc_files(grace).unwrap();
    assert_eq!(stats.scanned, 10);
    assert_eq!(stats.removed, 2);
    assert_eq!(stats.freed_bytes, (b"deleted-att-bytes".len() + b"0123456789".len()) as u64);
    assert_eq!(stats.skipped_recent, 1);

    // Live files & recent orphan must still exist.
    for kept in [
        &plain_stored_key,
        &plain_strict_alias,
        &plain_id_name_key,
        &e2ee_att_key,
        &cover_bg_key,
        &cover_img_key,
        &cover_e2ee_key,
        &"orphan-recent.dat".to_owned(),
    ] {
        assert!(files_dir.join(kept).exists(), "{kept} should have been preserved");
    }
    // Old orphans must be gone.
    assert!(!files_dir.join(&deleted_att_key).exists());
    assert!(!files_dir.join("orphan-old.dat").exists());

    // Deleting the E2EE attachment clears its file_key so subsequent GC reclaims the file.
    server
        .sync(&json!({
            "protocol": 2,
            "device": "laptop",
            "since": 0,
            "changes": [{
                "entity": "task_attachments",
                "key": "att3",
                "at": 200,
                "row": null
            }]
        }))
        .unwrap();
    let stats2 = server.gc_files(grace).unwrap();
    assert_eq!(stats2.removed, 1);
    assert_eq!(stats2.freed_bytes, b"live-e2ee".len() as u64);
    assert_eq!(stats2.skipped_recent, 1);
    assert!(!files_dir.join(&e2ee_att_key).exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_files_and_post_gc_endpoints_require_auth_and_work() {
    let dir = unique_temp_dir("fern-server-http-gc");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    let token = "secret-bearer-token-12345";
    let server = Server::open(&db_path).unwrap();
    let http = TestHttpServer::start(server, token, DEFAULT_GC_GRACE);

    // Unauthorized requests are rejected.
    assert_eq!(http_request(http.port, "DELETE", "/files/a.txt", None, b"").0, 401);
    assert_eq!(http_request(http.port, "DELETE", "/files/a.txt", Some("wrong"), b"").0, 401);
    assert_eq!(http_request(http.port, "POST", "/gc", None, b"").0, 401);

    // PUT, GET, DELETE lifecycle on /files/<name>.
    assert_eq!(http_request(http.port, "PUT", "/files/sample.txt", Some(token), b"hello world").0, 204);
    let (status, body) = http_request(http.port, "GET", "/files/sample.txt", Some(token), b"");
    assert_eq!(status, 200);
    assert_eq!(body, b"hello world");

    assert_eq!(http_request(http.port, "DELETE", "/files/sample.txt", Some(token), b"").0, 204);
    assert_eq!(http_request(http.port, "DELETE", "/files/sample.txt", Some(token), b"").0, 404);
    assert_eq!(http_request(http.port, "GET", "/files/sample.txt", Some(token), b"").0, 404);

    // Invalid names return 400.
    assert_eq!(http_request(http.port, "DELETE", "/files/../escape", Some(token), b"").0, 400);
    assert_eq!(http_request(http.port, "DELETE", "/files/.hidden", Some(token), b"").0, 400);
    assert_eq!(http_request(http.port, "DELETE", "/files/", Some(token), b"").0, 400);

    // POST /gc uses default 24h grace period when body is empty, skipping recent files.
    assert_eq!(http_request(http.port, "PUT", "/files/orphan.bin", Some(token), b"123456").0, 204);
    let (status, body) = http_request(http.port, "POST", "/gc", Some(token), b"");
    assert_eq!(status, 200);
    let stats: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(stats, json!({ "scanned": 1, "removed": 0, "freed_bytes": 0, "skipped_recent": 1 }));
    assert!(files_dir.join("orphan.bin").exists());

    // POST /gc with min_age_secs: 0 removes the unreferenced file immediately.
    let (status, body) = http_request(http.port, "POST", "/gc", Some(token), br#"{"min_age_secs":0}"#);
    assert_eq!(status, 200);
    let stats: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(stats, json!({ "scanned": 1, "removed": 1, "freed_bytes": 6, "skipped_recent": 0 }));
    assert!(!files_dir.join("orphan.bin").exists());

    // Malformed POST /gc body returns 400.
    assert_eq!(http_request(http.port, "POST", "/gc", Some(token), br#"{"min_age_secs":-1}"#).0, 400);
    assert_eq!(http_request(http.port, "POST", "/gc", Some(token), b"not-json").0, 400);

    drop(http);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn upgrade_from_v020_schema_preserves_live_legacy_attachments() {
    let dir = unique_temp_dir("fern-server-upgrade-v020");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    std::fs::create_dir_all(&files_dir).unwrap();

    // Create an existing v0.2.0 server DB WITHOUT the `file_key` column.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE changes (
                 seq    INTEGER PRIMARY KEY AUTOINCREMENT,
                 entity TEXT NOT NULL,
                 key    TEXT NOT NULL,
                 at     INTEGER NOT NULL,
                 row    TEXT,
                 device TEXT NOT NULL,
                 UNIQUE (entity, key)
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO changes (entity, key, at, row, device) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "task_attachments",
                "att-v1",
                100,
                json!({ "id": "att-v1", "task_id": "t1", "name": "Trip(1)+plan.pdf", "size": 10, "created_at": 100 }).to_string(),
                "legacy-device"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO changes (entity, key, at, row, device) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "task_attachments",
                "att-stored",
                100,
                json!({ "id": "att-stored", "stored_name": "att-stored-notes.txt" }).to_string(),
                "legacy-device"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO changes (entity, key, at, row, device) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                "countdowns",
                "cd-v1",
                100,
                json!({ "id": "cd-v1", "name": "Trip", "background": "cd-v1-cover.jpg" }).to_string(),
                "legacy-device"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO changes (entity, key, at, row, device) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["task_attachments", "att-gone", 100, Option::<String>::None, "legacy-device"],
        )
        .unwrap();
    }

    let old_age = Duration::from_secs(48 * 3600);
    let canonical = file_key("att-v1-Trip(1)+plan.pdf");
    let strict = encode_file_key("att-v1-Trip(1)+plan.pdf", false);
    assert_ne!(canonical, strict);

    write_file_with_age(&files_dir.join(&canonical), b"canonical", old_age);
    write_file_with_age(&files_dir.join(&strict), b"strict", old_age);
    write_file_with_age(&files_dir.join("att-stored-notes.txt"), b"stored", old_age);
    write_file_with_age(&files_dir.join("cd-v1-cover.jpg"), b"cover", old_age);
    write_file_with_age(&files_dir.join("orphan-v020.bin"), b"trash", old_age);

    // Opening upgrades the schema cleanly and protects all legacy live files during GC.
    let server = Server::open(&db_path).unwrap();
    let stats = server.gc_files(Duration::from_secs(24 * 3600)).unwrap();
    assert_eq!(stats.scanned, 5);
    assert_eq!(stats.removed, 1);
    assert_eq!(stats.freed_bytes, b"trash".len() as u64);
    assert_eq!(stats.skipped_recent, 0);

    assert!(files_dir.join(&canonical).exists());
    assert!(files_dir.join(&strict).exists());
    assert!(files_dir.join("att-stored-notes.txt").exists());
    assert!(files_dir.join("cd-v1-cover.jpg").exists());
    assert!(!files_dir.join("orphan-v020.bin").exists());

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_fails_fast_on_corrupt_database() {
    let dir = unique_temp_dir("fern-server-corrupt");
    let db_path = dir.join("corrupt.db");
    std::fs::write(&db_path, b"this is definitely not a valid sqlite database").unwrap();
    assert!(Server::open(&db_path).is_err(), "corrupt database must fail on startup");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn automatic_gc_runs_after_sync_threshold() {
    let dir = unique_temp_dir("fern-server-auto-gc");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    let token = "auto-gc-test-token-12345";
    let server = Server::open(&db_path).unwrap();
    let http = TestHttpServer::start(server, token, Duration::ZERO);

    write_file_with_age(&files_dir.join("stale-orphan.bin"), b"stale", Duration::ZERO);
    assert!(files_dir.join("stale-orphan.bin").exists());

    let sync_body = br#"{"protocol":2,"device":"dev-1","since":0,"changes":[]}"#;
    for _ in 0..AUTO_GC_SYNC_THRESHOLD {
        let (status, _) = http_request(http.port, "POST", "/sync", Some(token), sync_body);
        assert_eq!(status, 200);
    }
    // Automatic GC runs right after sending the threshold response on its worker thread;
    // poll briefly so Windows CI does not race against concurrent `/health` handling.
    assert_eq!(http_request(http.port, "GET", "/health", None, b"").0, 200);
    let orphan = files_dir.join("stale-orphan.bin");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while orphan.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !orphan.exists(),
        "automatic GC should remove orphaned files after reaching sync threshold"
    );

    drop(http);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn e2ee_store_attachments_survive_and_gc_on_delete() {
    let dir = unique_temp_dir("fern-server-e2ee-gc");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    let laptop_files = dir.join("laptop-files");
    std::fs::create_dir_all(&laptop_files).unwrap();
    let src_att = dir.join("ledger.txt");
    std::fs::write(&src_att, b"encrypted ledger contents").unwrap();

    let mut server = Server::open(&db_path).unwrap();
    let laptop = Store::open_in_memory().unwrap();
    laptop
        .set_sync_config(&fern_core::sync::SyncConfig {
            server: "https://sync.example.org".into(),
            token: "test-token-12345678".into(),
            passphrase: "correct horse battery staple".into(),
        })
        .unwrap();

    let task = laptop.add_task("Confidential").unwrap();
    let att = laptop.add_attachment(task.id, &src_att, &laptop_files).unwrap();
    let att_name = att.path(&laptop_files).file_name().unwrap().to_string_lossy().into_owned();
    let att_key = file_key(&att_name);

    sync(&mut server, &laptop);

    let old_age = Duration::from_secs(48 * 3600);
    let grace = Duration::from_secs(24 * 3600);
    write_file_with_age(&files_dir.join(&att_key), b"enc-att-blob", old_age);
    write_file_with_age(&files_dir.join("unrelated-orphan.bin"), b"orphan", old_age);

    // E2EE attachment is preserved via outer file_key; only the orphan is removed.
    let stats = server.gc_files(grace).unwrap();
    assert_eq!(stats.scanned, 2);
    assert_eq!(stats.removed, 1);
    assert!(files_dir.join(&att_key).exists());
    assert!(!files_dir.join("unrelated-orphan.bin").exists());

    // Deleting the attachment clears file_key so subsequent GC reclaims the encrypted blob.
    laptop.delete_attachment(&att, &laptop_files).unwrap();
    sync(&mut server, &laptop);

    let stats = server.gc_files(grace).unwrap();
    assert_eq!(stats.scanned, 1);
    assert_eq!(stats.removed, 1);
    assert_eq!(stats.freed_bytes, b"enc-att-blob".len() as u64);
    assert!(!files_dir.join(&att_key).exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_file_uploads_and_sync_leave_no_temp_files() {
    let dir = unique_temp_dir("fern-server-concurrent");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    let token = "concurrent-test-token-12345";
    let server = Server::open(&db_path).unwrap();
    let http = TestHttpServer::start(server, token, DEFAULT_GC_GRACE);

    let port = http.port;
    let handles: Vec<_> = (0..8)
        .map(|i| {
            std::thread::spawn(move || {
                let payload = format!("payload-{i}");
                let path = format!("/files/item-{}.txt", i % 4);
                assert_eq!(http_request(port, "PUT", &path, Some(token), payload.as_bytes()).0, 204);
                let sync_body = format!(r#"{{"protocol":2,"device":"dev-{i}","since":0,"changes":[]}}"#);
                assert_eq!(http_request(port, "POST", "/sync", Some(token), sync_body.as_bytes()).0, 200);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    for k in 0..4 {
        let (status, body) = http_request(port, "GET", &format!("/files/item-{k}.txt"), Some(token), b"");
        assert_eq!(status, 200);
        assert!(body.starts_with(b"payload-"));
    }
    let leftover_tmps: Vec<_> = std::fs::read_dir(&files_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftover_tmps.is_empty(), "atomic PUT must leave zero temp files: {leftover_tmps:?}");
    drop(http);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gc_preserves_dot_prefixed_temp_files_even_at_zero_grace() {
    let dir = unique_temp_dir("fern-server-gc-dot");
    let db_path = dir.join("fern-server.db");
    let files_dir = dir.join("fern-server.db.files");
    let server = Server::open(&db_path).unwrap();

    let in_flight = files_dir.join(".item.txt.tmp.123.0");
    write_file_with_age(&in_flight, b"partial", Duration::ZERO);
    let stats = server.gc_files(Duration::ZERO).unwrap();
    assert_eq!(stats.scanned, 0);
    assert_eq!(stats.removed, 0);
    assert!(in_flight.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn internal_errors_return_500_without_leaking_sqlite_details() {
    let server = Server::open_in_memory().unwrap();
    // Corrupt schema on purpose to force an internal SQLite error in sync() while keeping handle valid.
    server.conn.execute_batch("DROP TABLE changes;").unwrap();
    let http = TestHttpServer::start(server, "tok", DEFAULT_GC_GRACE);
    let (status, body) = http_request(
        http.port,
        "POST",
        "/sync",
        Some("tok"),
        br#"{"protocol":2,"device":"d1","since":0,"changes":[]}"#,
    );
    assert_eq!(status, 500);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("internal server error"), "got {text}");
    assert!(!text.contains("changes"), "must not leak SQL schema details: {text}");
}
