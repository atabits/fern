use std::fs::{self, File};
use std::io::{self, Read as _, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use tempfile::{NamedTempFile, TempDir};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::model::{Attachment, Id};
use crate::store::now_ms;

/// The largest file the sync server accepts, including a folder's finished ZIP archive.
pub const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct AttachmentTooLarge;

impl std::fmt::Display for AttachmentTooLarge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "attachment exceeds {} MiB",
            MAX_ATTACHMENT_BYTES / (1024 * 1024)
        )
    }
}

impl std::error::Error for AttachmentTooLarge {}

pub fn check_attachment_size(size: u64) -> Result<()> {
    if size > MAX_ATTACHMENT_BYTES {
        return Err(AttachmentTooLarge.into());
    }
    Ok(())
}

/// Publishes a downloaded file only after all its bytes have been written.
pub fn save_download(dir: &Path, name: &str, data: &[u8]) -> Result<()> {
    let mut components = Path::new(name).components();
    ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(part)) if part == name)
            && components.next().is_none()
            && !name.contains(['/', '\\', '\0']),
        "invalid attachment name"
    );
    fs::create_dir_all(dir)?;
    let mut file = NamedTempFile::new_in(dir)?;
    file.write_all(data)?;
    file.persist_noclobber(dir.join(name))?;
    Ok(())
}

/// A copied attachment whose task has yet to be updated. Dropping it removes the staged file.
pub struct PreparedAttachment {
    attachment: Attachment,
    staged_path: PathBuf,
    _staging: TempDir,
    publication: Option<Publication>,
}

enum Publication {
    Linked(PathBuf),
    Moved(PathBuf),
}

impl Publication {
    fn path(&self) -> &Path {
        match self {
            Self::Linked(path) | Self::Moved(path) => path,
        }
    }
}

impl PreparedAttachment {
    pub fn metadata(&self) -> &Attachment {
        &self.attachment
    }

    pub fn path(&self) -> &Path {
        match &self.publication {
            Some(Publication::Moved(path)) => path,
            _ => &self.staged_path,
        }
    }

    pub(crate) fn publish(&mut self, dir: &Path) -> Result<()> {
        let destination = self.attachment.path(dir);
        if let Some(publication) = &self.publication {
            // A failed rollback retains the file for the next attempt.
            ensure!(
                publication.path() == destination,
                "attachment publication directory changed"
            );
            return Ok(());
        }
        match fs::hard_link(&self.staged_path, &destination) {
            Ok(()) => self.publication = Some(Publication::Linked(destination)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(error.into()),
            Err(_) => {
                // FAT and exFAT cannot hard link. Both locations are in the attachment directory,
                // so a reversible rename still publishes the file without copying its contents.
                drop(File::create_new(&destination)?);
                if let Err(error) = fs::rename(&self.staged_path, &destination) {
                    let _ = fs::remove_file(&destination);
                    return Err(error.into());
                }
                self.publication = Some(Publication::Moved(destination));
            }
        }
        Ok(())
    }

    pub(crate) fn rollback_publication(&mut self) -> io::Result<()> {
        match &self.publication {
            Some(Publication::Linked(path)) => match fs::remove_file(path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            },
            Some(Publication::Moved(path)) => fs::rename(path, &self.staged_path)?,
            None => {}
        }
        self.publication = None;
        Ok(())
    }

    pub(crate) fn keep_publication(&mut self) {
        self.publication = None;
    }

    pub fn copy(task: Id, source: &Path, dir: &Path) -> Result<Self> {
        let source_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .context("the attachment has no file name")?;
        let metadata = source.metadata()?;
        if !metadata.is_dir() {
            ensure!(metadata.is_file(), "the attachment is not a regular file");
            check_attachment_size(metadata.len())?;
        }
        let staging = dir.join(".pending");
        fs::create_dir_all(&staging)?;
        let staging = staging.canonicalize()?;
        let name = if metadata.is_dir() {
            format!("{source_name}.zip")
        } else {
            source_name.to_owned()
        };
        // Keep the original extension so the staged file can be previewed with its usual app.
        let staging_dir = tempfile::tempdir_in(&staging)?;
        let staged_path = staging_dir.path().join(&name);
        let mut file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&staged_path)?;
        if metadata.is_dir() {
            let source = source.canonicalize()?;
            ensure!(
                !source.starts_with(&staging),
                "cannot attach the staging directory"
            );
            let mut output = LimitedArchive {
                file: &mut file,
                exceeded_limit: false,
            };
            let result = archive_directory(&source, source_name, &staging, &mut output);
            if output.exceeded_limit {
                return Err(AttachmentTooLarge.into());
            }
            result?;
        } else {
            let copied = io::copy(
                &mut File::open(source)?.take(MAX_ATTACHMENT_BYTES + 1),
                &mut file,
            )?;
            check_attachment_size(copied)?;
            #[cfg(unix)]
            file.set_permissions(metadata.permissions())?;
        }
        let size = file.metadata()?.len();
        let attachment = Attachment {
            id: Uuid::now_v7(),
            task_id: task,
            name,
            size,
            created_at: now_ms(),
        };
        Ok(Self {
            attachment,
            staged_path,
            _staging: staging_dir,
            publication: None,
        })
    }
}

impl Drop for PreparedAttachment {
    fn drop(&mut self) {
        if let Some(publication) = &self.publication {
            let _ = fs::remove_file(publication.path());
        }
    }
}

/// ZIP writes its directory at the end and seeks back to update headers.
struct LimitedArchive<'a> {
    file: &'a mut File,
    exceeded_limit: bool,
}

impl Write for LimitedArchive<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .file
            .stream_position()?
            .saturating_add(bytes.len() as u64)
            > MAX_ATTACHMENT_BYTES
        {
            self.exceeded_limit = true;
            return Err(io::Error::other(AttachmentTooLarge));
        }
        self.file.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for LimitedArchive<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

fn archive_directory(
    source: &Path,
    name: &str,
    staging: &Path,
    output: &mut LimitedArchive<'_>,
) -> Result<()> {
    let mut archive = ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let root = Path::new(name);
    // Keep empty directories. Do not traverse links or include files from another pending copy.
    for entry in WalkDir::new(source)
        .into_iter()
        .filter_entry(|entry| entry.path() != staging)
    {
        let entry = entry?;
        let path = root.join(entry.path().strip_prefix(source)?);
        if entry.file_type().is_dir() {
            archive.add_directory_from_path(path, options)?;
        } else if entry.file_type().is_file() {
            let mut file = File::open(entry.path())?;
            archive.start_file_from_path(
                path,
                options.large_file(file.metadata()?.len() >= u64::from(u32::MAX)),
            )?;
            io::copy(&mut file, &mut archive)?;
        }
    }
    archive.finish()?;
    Ok(())
}
