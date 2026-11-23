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
mod tests {
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
        // Automatic GC runs right after sending the 200th response; synchronize with a follow-up request.
        assert_eq!(http_request(http.port, "GET", "/health", None, b"").0, 200);
        assert!(
            !files_dir.join("stale-orphan.bin").exists(),
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
}
