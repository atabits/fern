use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, params};

use super::{DATE_FORMAT, SCHEMA_VERSION, Store};

/// Summary row counts for the primary user-facing tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableCounts {
    pub tasks: usize,
    pub lists: usize,
    pub habits: usize,
    pub attachments: usize,
    pub countdowns: usize,
}

/// A daily backup file (`fern-*.db`) discovered in a backups directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupEntry {
    pub path: PathBuf,
    pub filename: String,
    pub date: Option<NaiveDate>,
    pub size_bytes: u64,
}

/// Lists `fern-*.db` backup files in `dir`, sorted newest-first by date and filename.
pub fn list_backups(dir: &Path) -> Vec<BackupEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !filename.starts_with("fern-") || !filename.ends_with(".db") {
            continue;
        }
        let stem = &filename["fern-".len()..filename.len() - ".db".len()];
        let date = stem
            .get(..10)
            .and_then(|s| NaiveDate::parse_from_str(s, DATE_FORMAT).ok());
        backups.push(BackupEntry {
            path: entry.path(),
            filename,
            date,
            size_bytes: meta.len(),
        });
    }
    backups.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.filename.cmp(&a.filename)));
    backups
}

impl Store {
    /// Runs `PRAGMA integrity_check(10)` and returns the resulting messages (`["ok"]` when healthy).
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check(10)")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Runs `PRAGMA quick_check(10)` and returns the resulting messages (`["ok"]` when healthy).
    /// Much faster than [`Self::integrity_check`] for routine startup checks.
    pub fn quick_check(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA quick_check(10)")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Returns the database's current `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<usize> {
        let v: i64 = self.conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(usize::try_from(v).unwrap_or(0))
    }

    /// Returns row counts for the primary user-facing tables.
    pub fn table_counts(&self) -> Result<TableCounts> {
        let count = |sql: &str| -> Result<usize> {
            let n: i64 = self.conn.query_row(sql, [], |r| r.get(0))?;
            Ok(usize::try_from(n).unwrap_or(0))
        };
        Ok(TableCounts {
            tasks: count("SELECT COUNT(*) FROM tasks")?,
            lists: count("SELECT COUNT(*) FROM lists")?,
            habits: count("SELECT COUNT(*) FROM habits")?,
            attachments: count("SELECT COUNT(*) FROM task_attachments")?,
            countdowns: count("SELECT COUNT(*) FROM countdowns")?,
        })
    }

    /// Whether `path` is a Fern backup this version can restore.
    pub fn check_backup(path: &std::path::Path) -> Result<()> {
        let backup = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let is_fern: bool = backup
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tasks'", [], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        anyhow::ensure!(is_fern, "not a Fern backup");
        let version: i64 = backup.pragma_query_value(None, "user_version", |row| row.get(0))?;
        anyhow::ensure!(
            usize::try_from(version).is_ok_and(|v| v <= SCHEMA_VERSION),
            "the backup comes from a newer version of Fern"
        );
        Ok(())
    }

    /// "Generate Backup": a copy of all data in one SQLite file at `path`. With `attachments`
    /// (the attachment folder), the attached files go into the copy too, in a `backup_files`
    /// table, so that the backup stays one file.
    pub fn backup_to(&self, path: &std::path::Path, attachments: Option<&std::path::Path>) -> Result<()> {
        self.conn.backup(rusqlite::MAIN_DB, path, None::<fn(rusqlite::backup::Progress)>)?;
        if let Some(dir) = attachments.filter(|dir| dir.is_dir()) {
            let copy = Connection::open(path)?;
            copy.execute_batch("CREATE TABLE backup_files (name TEXT PRIMARY KEY, data BLOB NOT NULL);")?;
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file()
                    && let Some(name) = entry.file_name().to_str()
                {
                    copy.execute(
                        "INSERT INTO backup_files (name, data) VALUES (?1, ?2)",
                        params![name, std::fs::read(entry.path())?],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// The daily copy of the data: `fern-<date>.db` in `dir`, made once a day, keeping the
    /// last `keep` copies. Attached files are not copied: they stay in their own folder.
    pub fn daily_backup(&self, dir: &std::path::Path, date: NaiveDate, keep: usize) -> Result<()> {
        let path = dir.join(format!("fern-{}.db", date.format(DATE_FORMAT)));
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        self.backup_to(&path, None)?;
        let mut copies: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("fern-") && n.ends_with(".db")))
            .collect();
        copies.sort();
        for old in copies.iter().rev().skip(keep) {
            std::fs::remove_file(old)?;
            // What SQLite leaves beside a copy that was opened.
            for suffix in ["-wal", "-shm"] {
                let mut side = old.clone().into_os_string();
                side.push(suffix);
                std::fs::remove_file(side).ok();
            }
        }
        Ok(())
    }

    /// "Import Backups": replaces all data with a backup made by `backup_to`, bringing an older
    /// backup up to date. A file that is not a Fern backup is refused and nothing changes. When
    /// the backup carries attached files, the attachment folder `attachments` is replaced by them.
    pub fn restore_from(&mut self, path: &std::path::Path, attachments: Option<&std::path::Path>) -> Result<()> {
        Self::check_backup(path)?;
        self.conn.restore(rusqlite::MAIN_DB, path, None::<fn(rusqlite::backup::Progress)>)?;
        let has_files: bool = self
            .conn
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'backup_files'", [], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if has_files {
            if let Some(dir) = attachments {
                if dir.is_dir() {
                    std::fs::remove_dir_all(dir)?;
                }
                std::fs::create_dir_all(dir)?;
                let mut stmt = self.conn.prepare_cached("SELECT name, data FROM backup_files")?;
                let files = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)))?;
                for file in files {
                    let (name, data) = file?;
                    // Only plain names: a backup must not write outside the folder.
                    if !name.contains(['/', '\\']) && name != ".." {
                        std::fs::write(dir.join(name), data)?;
                    }
                }
            }
            self.conn.execute_batch("DROP TABLE backup_files;")?;
        }
        Self::migrate(&self.conn)
    }
}
