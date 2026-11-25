//! Structured application logging backed by the diagnostics ring buffer, a rotating
//! on-disk log file (`<data_dir>/logs/fern.log`), and gated debug/opt-in stderr output.

use std::fmt::Display;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use chrono::Local;

use crate::diagnostics;
use crate::paths;
use crate::runtime_config::RuntimeConfig;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

static FILE_LOG: OnceLock<Mutex<FileLogState>> = OnceLock::new();
static LOG_STDERR_OVERRIDE: OnceLock<bool> = OnceLock::new();

#[derive(Default)]
struct FileLogState {
    initialized: bool,
    dir: Option<PathBuf>,
    file: Option<File>,
    bytes_written: u64,
}

#[derive(Clone, Copy)]
enum Level {
    Error,
    Warn,
    Info,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
        }
    }
}

fn should_write_stderr() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    *LOG_STDERR_OVERRIDE.get_or_init(|| {
        std::env::var("FERN_LOG_STDERR")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    })
}

fn resolved_log_dir() -> Option<PathBuf> {
    if cfg!(test) && RuntimeConfig::get().data_dir_override.is_none() {
        return None;
    }
    paths::data_dir().ok().map(|d| d.join("logs"))
}

fn rotate_and_open(dir: &Path, force: bool) -> std::io::Result<(File, u64)> {
    fs::create_dir_all(dir)?;
    let active = dir.join("fern.log");
    let backup = dir.join("fern.log.1");
    let len = fs::metadata(&active).map(|m| m.len()).unwrap_or(0);
    if (force && len > 0) || len >= MAX_LOG_BYTES {
        let _ = fs::remove_file(&backup);
        let _ = fs::rename(&active, &backup);
    }
    let file = OpenOptions::new().create(true).append(true).open(&active)?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((file, size))
}

fn append_line(state: &mut FileLogState, dir: &Path, line: &str) {
    if !state.initialized || state.dir.as_deref() != Some(dir) {
        state.initialized = true;
        state.dir = Some(dir.to_owned());
        state.file = None;
        state.bytes_written = 0;
        if let Ok((file, size)) = rotate_and_open(dir, false) {
            state.file = Some(file);
            state.bytes_written = size;
        }
    }
    if state.bytes_written >= MAX_LOG_BYTES {
        state.file = None;
        if let Ok((file, size)) = rotate_and_open(dir, true) {
            state.file = Some(file);
            state.bytes_written = size;
        }
    }
    if let Some(file) = state.file.as_mut() {
        let line_bytes = line.as_bytes();
        if file.write_all(line_bytes).is_ok() && file.write_all(b"\n").is_ok() {
            state.bytes_written = state.bytes_written.saturating_add(line_bytes.len() as u64 + 1);
        }
    }
}

fn emit(level: Level, target: &str, message: &str) {
    let single_line = message.replace(['\r', '\n'], " ");
    let sanitized = diagnostics::redact_sensitive(single_line.trim());
    let category = match level {
        Level::Error => target.to_owned(),
        Level::Warn => format!("{target}:warn"),
        Level::Info => format!("{target}:info"),
    };
    diagnostics::log_event(&category, &sanitized);

    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let formatted = format!("[{}] {} [{}] {}", level.tag(), timestamp, target, sanitized);
    if should_write_stderr() {
        eprintln!("{formatted}");
    }
    if let Some(dir) = resolved_log_dir() {
        let lock = FILE_LOG.get_or_init(|| Mutex::new(FileLogState::default()));
        let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        append_line(&mut guard, &dir, &formatted);
    }
}

/// Logs an error to diagnostics, rotating `<data_dir>/logs/fern.log`, and debug/opt-in stderr.
pub fn log_error(target: &str, message: impl AsRef<str>) {
    emit(Level::Error, target, message.as_ref());
}

/// Logs a warning to diagnostics, rotating `<data_dir>/logs/fern.log`, and debug/opt-in stderr.
pub fn log_warn(target: &str, message: impl AsRef<str>) {
    emit(Level::Warn, target, message.as_ref());
}

/// Logs an informational event to diagnostics, rotating `<data_dir>/logs/fern.log`, and debug/opt-in stderr.
pub fn log_info(target: &str, message: impl AsRef<str>) {
    emit(Level::Info, target, message.as_ref());
}

/// Logs `Err` results concisely and converts them into `Option<T>`.
pub trait ResultExt<T> {
    fn log_err(self, target: &str) -> Option<T>;
    fn log_err_ctx(self, target: &str, context: &str) -> Option<T>;
}

impl<T, E: Display> ResultExt<T> for Result<T, E> {
    fn log_err(self, target: &str) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                log_error(target, format!("{err:#}"));
                None
            }
        }
    }

    fn log_err_ctx(self, target: &str, context: &str) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                log_error(target, format!("{context}: {err:#}"));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotating_file_log_redacts_and_rolls_over_past_limit() {
        let dir = std::env::temp_dir().join(format!("fern-log-test-{}", uuid::Uuid::new_v4()));
        let mut state = FileLogState::default();

        // Write a sensitive message and verify diagnostics + file storage redact it cleanly.
        let secret = "sync failed at https://user:pw@private.example.com/v1 Bearer supersecret";
        let sanitized = diagnostics::redact_sensitive(secret);
        append_line(&mut state, &dir, &sanitized);
        let written = fs::read_to_string(dir.join("fern.log")).expect("read fern.log");
        assert!(!written.contains("private.example.com"));
        assert!(!written.contains("supersecret"));
        assert!(written.contains("https://<redacted-host>"));
        assert!(written.contains("Bearer <redacted>"));

        // Exceed 2 MiB and verify rotation creates fern.log.1 and starts fresh fern.log.
        state.bytes_written = MAX_LOG_BYTES;
        append_line(&mut state, &dir, "after-rotation");
        assert!(dir.join("fern.log.1").exists());
        let rolled = fs::read_to_string(dir.join("fern.log")).expect("read rolled log");
        assert_eq!(rolled.trim(), "after-rotation");
        drop(state);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_ext_logs_error_and_returns_none() {
        let _guard = diagnostics::TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let res: Result<u32, anyhow::Error> = Err(anyhow::anyhow!("boom"));
        assert_eq!(res.log_err_ctx("test_logging", "failed op"), None);
        let events = diagnostics::recent_events();
        assert!(
            events.iter().any(|e| e.contains("[test_logging]") && e.contains("failed op: boom")),
            "expected event in ring buffer, got: {events:?}"
        );
    }
}
