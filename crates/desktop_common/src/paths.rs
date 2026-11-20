use anyhow::{Context as _, Result};

use crate::portable;

/// How many daily copies are kept.
pub const DAILY_BACKUPS: usize = 7;

/// Where Fern keeps its data.
pub fn data_dir() -> Result<std::path::PathBuf> {
    // Debug builds can be pointed at throwaway data, so that checking a change leaves the real
    // data alone.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("FERN_DATA_DIR") {
        return Ok(dir.into());
    }
    if let Some(dir) = portable::data_dir() {
        return Ok(dir.to_owned());
    }
    Ok(dirs::data_dir().context("cannot determine the user data directory")?.join("fern-tasks"))
}

/// Where temporary files for printing and backup exports are written.
pub fn scratch_dir() -> std::io::Result<std::path::PathBuf> {
    let Some(dir) = portable::data_dir() else {
        return Ok(std::env::temp_dir());
    };
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache)?;
    Ok(cache)
}

/// Where attached files are kept, as copies.
pub fn attachments_dir() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("attachments"))
}

/// A backup chosen with "Import Backups", restored on the next start before anything opens.
pub fn pending_restore_path() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("restore.db"))
}

/// Where the daily copies of the data are kept.
pub fn backups_dir() -> Result<std::path::PathBuf> {
    Ok(data_dir()?.join("backups"))
}
