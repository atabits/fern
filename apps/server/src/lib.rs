//! Fern's sync server: it keeps the latest change of every synced row and hands each device the
//! changes it has not seen. It does not look into the rows; devices decide what they mean (see
//! `fern_core::sync`).
//!
//! One request does a whole sync, `POST /sync` with a bearer token:
//!
//! ```json
//! { "protocol": 2, "device": "<id>", "since": 42, "changes": [{ "entity": "tasks", "key": "…", "at": 1760000000000, "row": {…} }] }
//! ```
//!
//! The answer has the changes other devices made after `since`, and the position to send next
//! time: `{ "protocol": 2, "min_protocol": 1, "changes": […], "position": 57 }`.

use std::collections::HashSet;
use std::fmt;
use std::io::Read as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, bail};
use fern_core::attachment::MAX_ATTACHMENT_BYTES;
use fern_core::sync::{
    MAX_FILE_KEY_BYTES, MIN_SYNC_PROTOCOL_VERSION, SYNC_PROTOCOL_VERSION, encode_file_key, file_key,
};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Map, Value, json};

/// Default grace period before unreferenced files in `<data>.files` are eligible for garbage
/// collection (24 hours), protecting in-flight uploads that have not yet committed metadata.
pub const DEFAULT_GC_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// Maximum interval between automatic garbage-collection passes while serving.
const AUTO_GC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Number of completed sync requests between automatic garbage-collection passes.
const AUTO_GC_SYNC_THRESHOLD: u64 = 200;

/// Sync JSON requests larger than this are refused.
const MAX_BODY: usize = 32 * 1024 * 1024;

/// `POST /gc` JSON bodies larger than this are refused.
const MAX_GC_BODY: usize = 64 * 1024;

/// Maximum file upload size: `MAX_ATTACHMENT_BYTES` plus headroom for the client-side E2EE file
/// envelope (9-byte header + 12-byte nonce + 16-byte Poly1305 tag).
const MAX_FILE_UPLOAD_BYTES: u64 = MAX_ATTACHMENT_BYTES + 64;

/// Returned when a client sends a sync protocol version outside this server's supported range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolMismatchError {
    pub client: u64,
    pub server: u32,
    pub min_server: u32,
}

impl fmt::Display for ProtocolMismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "incompatible sync protocol: client reported {}, server supports {}..={}",
            self.client, self.min_server, self.server,
        )
    }
}

impl std::error::Error for ProtocolMismatchError {}

/// Summary of a `.files` garbage-collection pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    pub scanned: usize,
    pub removed: usize,
    pub freed_bytes: u64,
    pub skipped_recent: usize,
}

impl GcStats {
    /// Serializes these statistics as a JSON object for `POST /gc` and `--gc-now`.
    pub fn to_json(self) -> Value {
        json!({
            "scanned": self.scanned,
            "removed": self.removed,
            "freed_bytes": self.freed_bytes,
            "skipped_recent": self.skipped_recent,
        })
    }
}

pub struct Server {
    conn: Connection,
    /// Where attached files are kept (`<data>.files`), sent with `PUT /files/<name>`, fetched with
    /// `GET /files/<name>`, and removed with `DELETE /files/<name>`.
    files: Option<std::path::PathBuf>,
}

impl Server {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let mut files = path.as_os_str().to_owned();
        files.push(".files");
        Self::init(Connection::open(path)?, Some(files.into()))
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    /// Where a sent file goes: `None` for a name that is not a plain file name.
    pub fn file_path(&self, name: &str) -> Option<std::path::PathBuf> {
        let plain = !name.is_empty()
            && name.len() <= MAX_FILE_KEY_BYTES
            && !name.starts_with('.')
            && !name.contains(['/', '\\', ':', '\0']);
        self.files.as_ref().filter(|_| plain).map(|dir| dir.join(name))
    }

    fn init(conn: Connection, files: Option<std::path::PathBuf>) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        // Fail fast before opening WAL or migrating if the SQLite database file is corrupt.
        let issues: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA integrity_check(10)")?;
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if issues != ["ok"] {
            bail!("server database integrity check failed: {}", issues.join("; "));
        }

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS changes (
                 seq      INTEGER PRIMARY KEY AUTOINCREMENT,
                 entity   TEXT NOT NULL,
                 key      TEXT NOT NULL,
                 at       INTEGER NOT NULL,
                 row      TEXT,
                 device   TEXT NOT NULL,
                 file_key TEXT,
                 UNIQUE (entity, key)
             );",
        )?;

        let has_file_key = {
            let mut stmt = conn.prepare("PRAGMA table_info(changes)")?;
            let columns: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            columns.iter().any(|column| column == "file_key")
        };
        if !has_file_key {
            conn.execute_batch("ALTER TABLE changes ADD COLUMN file_key TEXT;")?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_changes_file_gc ON changes(entity, file_key) WHERE row IS NOT NULL;",
        )?;

        if let Some(dir) = &files {
            std::fs::create_dir_all(dir)?;
        }
        Ok(Self { conn, files })
    }

    /// One sync: stores the device's changes (the later change of a row wins) and returns the
    /// other devices' changes after `since`.
    pub fn sync(&mut self, request: &Value) -> Result<Value> {
        if !request.is_object() {
            bail!("sync request must be a JSON object");
        }
        let client_proto = match &request["protocol"] {
            Value::Null => 1,
            Value::Number(num) => num.as_u64().context("protocol must be a non-negative integer")?,
            _ => bail!("protocol must be a non-negative integer"),
        };
        if client_proto < u64::from(MIN_SYNC_PROTOCOL_VERSION)
            || client_proto > u64::from(SYNC_PROTOCOL_VERSION)
        {
            return Err(ProtocolMismatchError {
                client: client_proto,
                server: SYNC_PROTOCOL_VERSION,
                min_server: MIN_SYNC_PROTOCOL_VERSION,
            }
            .into());
        }

        let device = request["device"]
            .as_str()
            .filter(|d| !d.is_empty())
            .context("a sync needs a device id")?;
        let since = request["since"].as_i64().unwrap_or(0);
        let empty = Vec::new();
        let incoming = match &request["changes"] {
            Value::Null => &empty,
            Value::Array(changes) => changes,
            _ => bail!("changes must be a list"),
        };

        let tx = self.conn.transaction()?;
        {
            let mut select_at =
                tx.prepare_cached("SELECT at FROM changes WHERE entity = ?1 AND key = ?2")?;
            let mut delete_change =
                tx.prepare_cached("DELETE FROM changes WHERE entity = ?1 AND key = ?2")?;
            let mut insert_change = tx.prepare_cached(
                "INSERT INTO changes (entity, key, at, row, device, file_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for change in incoming {
                let entity = change["entity"].as_str().context("a change needs an entity")?;
                let key = change["key"].as_str().context("a change needs a key")?;
                let at = change["at"].as_i64().context("a change needs a time")?;
                let (row, stored_file_key) = match &change["row"] {
                    Value::Null => (None, None),
                    row @ Value::Object(map) => {
                        let fk = change
                            .get("file_key")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .or_else(|| extract_plaintext_file_name(entity, map).map(|name| file_key(&name)));
                        (Some(row.to_string()), fk)
                    }
                    _ => bail!("a change's row must be an object or null"),
                };
                let stored: Option<i64> = select_at
                    .query_row(params![entity, key], |r| r.get(0))
                    .optional()?;
                if stored.is_some_and(|stored| stored > at) {
                    continue;
                }
                // A new sequence number, so that devices that saw the old change get this one.
                delete_change.execute(params![entity, key])?;
                insert_change.execute(params![entity, key, at, row, device, stored_file_key])?;
            }
        }
        tx.commit()?;

        let position: i64 = self
            .conn
            .prepare_cached("SELECT COALESCE(MAX(seq), 0) FROM changes")?
            .query_row([], |r| r.get(0))?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT entity, key, at, row FROM changes WHERE seq > ?1 AND device != ?2 ORDER BY seq")?;
        let outgoing = stmt
            .query_map(params![since, device], |r| {
                let row: Option<String> = r.get(3)?;
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, row))
            })?
            .map(|item| {
                let (entity, key, at, row) = item?;
                let row = match row {
                    Some(text) => serde_json::from_str(&text)?,
                    None => Value::Null,
                };
                Ok(json!({ "entity": entity, "key": key, "at": at, "row": row }))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({
            "protocol": SYNC_PROTOCOL_VERSION,
            "min_protocol": MIN_SYNC_PROTOCOL_VERSION,
            "changes": outgoing,
            "position": position,
        }))
    }

    /// Removes unreferenced regular files in `<data>.files` older than `min_age`.
    ///
    /// Live files are protected via both the `changes.file_key` column (populated by v2 plaintext
    /// and E2EE clients) and legacy plaintext `task_attachments`/`countdowns` rows (including
    /// canonical, strict percent-encoded, and verbatim key aliases) so upgrades from v0.2.0 server
    /// databases never lose live attachments.
    pub fn gc_files(&self, min_age: Duration) -> Result<GcStats> {
        let Some(dir) = &self.files else {
            return Ok(GcStats::default());
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(GcStats::default()),
            Err(err) => return Err(err.into()),
        };

        let live = self.live_file_keys()?;
        let now = SystemTime::now();
        let mut stats = GcStats::default();

        for entry_res in entries {
            let entry = match entry_res {
                Ok(entry) => entry,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            };
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            };
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }

            stats.scanned += 1;
            if live.contains(&name) {
                continue;
            }

            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            };

            let is_recent = if min_age.is_zero() {
                false
            } else {
                match meta.modified().ok().and_then(|mtime| now.duration_since(mtime).ok()) {
                    Some(age) => age < min_age,
                    // Treat missing or future mtimes conservatively as recent.
                    None => true,
                }
            };
            if is_recent {
                stats.skipped_recent += 1;
                continue;
            }

            let size = meta.len();
            match std::fs::remove_file(entry.path()) {
                Ok(()) => {
                    stats.removed += 1;
                    stats.freed_bytes = stats.freed_bytes.saturating_add(size);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }

        Ok(stats)
    }

    fn live_file_keys(&self) -> Result<HashSet<String>> {
        let mut live = HashSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT entity, row, file_key
             FROM changes
             WHERE row IS NOT NULL
               AND (file_key IS NOT NULL OR entity IN ('task_attachments', 'countdowns'))",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for item in rows {
            let (entity, row_json, stored_key) = item?;
            if let Some(fk) = stored_key.filter(|s| !s.is_empty()) {
                live.insert(fk);
            }
            if matches!(entity.as_str(), "task_attachments" | "countdowns")
                && let Some(text) = row_json
                && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text)
                && let Some(raw_name) = extract_plaintext_file_name(&entity, &map)
            {
                live.insert(file_key(&raw_name));
                let strict = encode_file_key(&raw_name, false);
                if strict.len() <= MAX_FILE_KEY_BYTES {
                    live.insert(strict);
                }
                if raw_name.len() <= MAX_FILE_KEY_BYTES {
                    live.insert(raw_name);
                }
            }
        }
        Ok(live)
    }
}

/// Extracts the raw local attachment or countdown background filename from a plaintext synced row.
fn extract_plaintext_file_name(entity: &str, row: &Map<String, Value>) -> Option<String> {
    match entity {
        "task_attachments" => {
            if let Some(stored) = row.get("stored_name").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                return Some(stored.to_owned());
            }
            let id = row.get("id").and_then(Value::as_str).filter(|s| !s.is_empty())?;
            let name = row.get("name").and_then(Value::as_str).filter(|s| !s.is_empty())?;
            Some(format!("{id}-{name}"))
        }
        "countdowns" => {
            if let Some(bg) = row.get("background").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                return Some(bg.to_owned());
            }
            row.get("cover_image")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        }
        _ => None,
    }
}

fn parse_gc_min_age(body: &str, default_grace: Duration) -> Result<Duration> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(default_grace);
    }
    let value: Value = serde_json::from_str(trimmed).context("invalid JSON body")?;
    let obj = value.as_object().context("gc request body must be a JSON object")?;
    match obj.get("min_age_secs") {
        None | Some(Value::Null) => Ok(default_grace),
        Some(Value::Number(n)) => {
            let secs = n.as_u64().context("min_age_secs must be a non-negative integer")?;
            Ok(Duration::from_secs(secs))
        }
        Some(_) => bail!("min_age_secs must be a non-negative integer"),
    }
}

fn json_response(status: u16, body: String) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let response = tiny_http::Response::from_string(body).with_status_code(status);
    match tiny_http::Header::from_bytes("Content-Type", "application/json") {
        Ok(header) => response.with_header(header),
        Err(()) => response,
    }
}

fn health_payload() -> String {
    json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": SYNC_PROTOCOL_VERSION,
        "min_protocol": MIN_SYNC_PROTOCOL_VERSION,
    })
    .to_string()
}

fn run_auto_gc(server: &Server, gc_grace: Duration, syncs_since_gc: &mut u64, last_gc: &mut Instant) {
    *syncs_since_gc = 0;
    *last_gc = Instant::now();
    match server.gc_files(gc_grace) {
        Ok(stats) if stats.removed > 0 => {
            eprintln!(
                "fern-server gc: removed {} file(s), freed {} bytes (scanned {}, skipped recent {})",
                stats.removed, stats.freed_bytes, stats.scanned, stats.skipped_recent
            );
        }
        Ok(_) => {}
        Err(err) => {
            eprintln!("fern-server gc failed: {err:#}");
        }
    }
}

/// Answers sync requests on `http` until it closes using [`DEFAULT_GC_GRACE`] for periodic and
/// default `POST /gc` garbage collection.
pub fn serve(http: &tiny_http::Server, server: Server, token: &str) {
    serve_with_gc_grace(http, server, token, DEFAULT_GC_GRACE);
}

const WORKER_THREADS: usize = 4;
const REQUEST_QUEUE_BOUND: usize = 32;

struct SharedState {
    server: Server,
    syncs_since_gc: u64,
    last_gc: Instant,
}

fn write_file_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    static NEXT_TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = NEXT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let suffix = format!(".tmp.{}.{seq}", std::process::id());
    let tmp = if path.file_name().is_some_and(|name| name.len() + suffix.len() <= MAX_FILE_KEY_BYTES) {
        let mut tmp_os = path.as_os_str().to_owned();
        tmp_os.push(&suffix);
        std::path::PathBuf::from(tmp_os)
    } else if let Some(parent) = path.parent() {
        parent.join(format!("upload{suffix}"))
    } else {
        let mut tmp_os = path.as_os_str().to_owned();
        tmp_os.push(&suffix);
        std::path::PathBuf::from(tmp_os)
    };
    if let Err(err) = std::fs::write(&tmp, data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

fn maybe_run_auto_gc(state: &Mutex<SharedState>, gc_grace: Duration) {
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    if guard.syncs_since_gc >= AUTO_GC_SYNC_THRESHOLD || guard.last_gc.elapsed() >= AUTO_GC_INTERVAL {
        let SharedState { server, syncs_since_gc, last_gc } = &mut *guard;
        run_auto_gc(server, gc_grace, syncs_since_gc, last_gc);
    }
}

fn handle_request(
    mut request: tiny_http::Request,
    state: &Mutex<SharedState>,
    token: &str,
    gc_grace: Duration,
) {
    use tiny_http::{Method, Response};

    if request.method() == &Method::Get && request.url() == "/health" {
        maybe_run_auto_gc(state, gc_grace);
        if let Err(err) = request.respond(json_response(200, health_payload())) {
            eprintln!("failed to answer: {err}");
        }
        return;
    }

    let auth = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_owned());
    if !authorized(auth.as_deref(), token) {
        request.respond(json_response(401, r#"{"error":"unauthorized"}"#.to_owned())).ok();
        return;
    }

    if let Some(name) = request.url().strip_prefix("/files/").map(str::to_owned) {
        let path = state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .server
            .file_path(&name);
        let response = match (request.method(), path) {
            (_, None) => Response::from_data(Vec::new()).with_status_code(400),
            (Method::Put, Some(path)) => {
                let mut data = Vec::new();
                match request.as_reader().take(MAX_FILE_UPLOAD_BYTES + 1).read_to_end(&mut data) {
                    Ok(n) if n as u64 > MAX_FILE_UPLOAD_BYTES => {
                        Response::from_data(Vec::new()).with_status_code(413)
                    }
                    Ok(_) => match write_file_atomic(&path, &data) {
                        Ok(()) => Response::from_data(Vec::new()).with_status_code(204),
                        Err(_) => Response::from_data(Vec::new()).with_status_code(500),
                    },
                    Err(_) => Response::from_data(Vec::new()).with_status_code(400),
                }
            }
            (Method::Get, Some(path)) => match std::fs::read(&path) {
                Ok(data) => Response::from_data(data),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    Response::from_data(Vec::new()).with_status_code(404)
                }
                Err(_) => Response::from_data(Vec::new()).with_status_code(500),
            },
            (Method::Delete, Some(path)) => match std::fs::remove_file(&path) {
                Ok(()) => Response::from_data(Vec::new()).with_status_code(204),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    Response::from_data(Vec::new()).with_status_code(404)
                }
                Err(_) => Response::from_data(Vec::new()).with_status_code(500),
            },
            _ => Response::from_data(Vec::new()).with_status_code(405),
        };
        if let Err(err) = request.respond(response) {
            eprintln!("failed to answer: {err}");
        }
        return;
    }

    let mut trigger_auto_gc = false;
    let (status, body) = match (request.method(), request.url()) {
        (Method::Post, "/sync") => {
            let mut text = String::new();
            let read = request.as_reader().take(MAX_BODY as u64 + 1).read_to_string(&mut text);
            match read {
                Ok(n) if n > MAX_BODY => (413, r#"{"error":"too large"}"#.to_owned()),
                Ok(_) => match serde_json::from_str::<Value>(&text) {
                    Ok(req) => {
                        let (sync_res, should_gc) = {
                            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            let res = guard.server.sync(&req);
                            let should_gc = if res.is_ok() {
                                guard.syncs_since_gc += 1;
                                guard.syncs_since_gc >= AUTO_GC_SYNC_THRESHOLD
                                    || guard.last_gc.elapsed() >= AUTO_GC_INTERVAL
                            } else {
                                false
                            };
                            (res, should_gc)
                        };
                        match sync_res {
                            Ok(answer) => {
                                trigger_auto_gc = should_gc;
                                (200, answer.to_string())
                            }
                            Err(err) => {
                                if let Some(mismatch) = err.downcast_ref::<ProtocolMismatchError>() {
                                    (
                                        426,
                                        json!({
                                            "error": "incompatible_protocol",
                                            "server_protocol": mismatch.server,
                                            "min_protocol": mismatch.min_server,
                                            "client_protocol": mismatch.client,
                                        })
                                        .to_string(),
                                    )
                                } else {
                                    (400, json!({ "error": format!("{err:#}") }).to_string())
                                }
                            }
                        }
                    }
                    Err(err) => (400, json!({ "error": format!("{err:#}") }).to_string()),
                },
                Err(err) => (400, json!({ "error": err.to_string() }).to_string()),
            }
        }
        (Method::Post, "/gc") => {
            let mut text = String::new();
            let read = request.as_reader().take(MAX_GC_BODY as u64 + 1).read_to_string(&mut text);
            match read {
                Ok(n) if n > MAX_GC_BODY => (413, r#"{"error":"too large"}"#.to_owned()),
                Ok(_) => match parse_gc_min_age(&text, gc_grace) {
                    Ok(min_age) => {
                        let gc_res = {
                            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            let res = guard.server.gc_files(min_age);
                            if res.is_ok() {
                                guard.syncs_since_gc = 0;
                                guard.last_gc = Instant::now();
                            }
                            res
                        };
                        match gc_res {
                            Ok(stats) => (200, stats.to_json().to_string()),
                            Err(err) => (500, json!({ "error": format!("{err:#}") }).to_string()),
                        }
                    }
                    Err(err) => (400, json!({ "error": format!("{err:#}") }).to_string()),
                },
                Err(err) => (400, json!({ "error": err.to_string() }).to_string()),
            }
        }
        _ => (404, r#"{"error":"not found"}"#.to_owned()),
    };

    if let Err(err) = request.respond(json_response(status, body)) {
        eprintln!("failed to answer: {err}");
    }

    if trigger_auto_gc {
        maybe_run_auto_gc(state, gc_grace);
    }
}

/// Answers HTTP requests on `http` until it closes:
/// - `GET /health` (unauthenticated)
/// - `POST /sync` (bearer auth)
/// - `PUT`/`GET`/`DELETE /files/<name>` (bearer auth)
/// - `POST /gc` (bearer auth)
pub fn serve_with_gc_grace(http: &tiny_http::Server, server: Server, token: &str, gc_grace: Duration) {
    let state = Arc::new(Mutex::new(SharedState {
        server,
        syncs_since_gc: 0,
        last_gc: Instant::now(),
    }));
    let token: Arc<str> = Arc::from(token);
    let (tx, rx) = mpsc::sync_channel::<tiny_http::Request>(REQUEST_QUEUE_BOUND);
    let rx = Arc::new(Mutex::new(rx));

    let mut workers = Vec::with_capacity(WORKER_THREADS);
    for _ in 0..WORKER_THREADS {
        let rx = Arc::clone(&rx);
        let state = Arc::clone(&state);
        let token = Arc::clone(&token);
        workers.push(std::thread::spawn(move || {
            loop {
                let received = {
                    let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                    guard.recv()
                };
                let Ok(request) = received else {
                    break;
                };
                handle_request(request, &state, &token, gc_grace);
            }
        }));
    }

    for request in http.incoming_requests() {
        if tx.send(request).is_err() {
            break;
        }
    }
    drop(tx);
    for worker in workers {
        let _ = worker.join();
    }
}

/// Compares a request's `Authorization` header with the token, in constant time.
pub fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(given) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return false;
    };
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests;
