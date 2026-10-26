//! Repository-level advisory lock.
//!
//! Velody coordinates the SQLite database (via WAL) automatically, but the object
//! store (`.velo/objects/`) and the ref files (`PARENT`, `HEAD`, `MERGE_HEAD`)
//! are not transactional with it. Two mutating `velody` processes running at once
//! could therefore race — most dangerously `gc` deleting an object that a
//! concurrent `save` has written to disk but not yet committed to `file_map`.
//!
//! A single coarse lock, held for the duration of any mutating command,
//! serialises those operations. Read-only commands (`status`, `history`, …) do
//! not take it, so they never block. The lock is advisory (OS-level, on the
//! `.velo/lock` file handle) and is released automatically when the process
//! exits — even on crash — so it can never go stale.
//!
//! Beside the lock sits an informational *holder record* (`<lock>.holder`:
//! pid, start time, command) so a refused writer can say who is in the way. The
//! record is not the lock: it can outlive a crashed holder, and readers must
//! treat it as a hint.

#[cfg(not(target_family = "wasm"))]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

#[cfg(not(target_family = "wasm"))]
use fs2::FileExt;

use crate::error::{Result, VelodyError};

/// Who holds the repository write lock, as recorded when they took it.
///
/// Informational: the OS lock is the authority. A record can be left behind by
/// a crashed process, so check that `pid` is alive before trusting it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LockHolder {
    /// Process id of the holder.
    pub pid: u32,
    /// When the holder took the lock, in milliseconds since the Unix epoch.
    pub started_at_ms: i64,
    /// What the holder said it was doing (usually its command line), if it said.
    pub command: Option<String>,
}

/// An acquired repository lock. Dropping it (or the process exiting) releases
/// the underlying OS lock.
///
/// On `wasm` targets there is no OS file lock and no second process to race
/// with, so this is a documented no-op: every constructor succeeds and nothing
/// is created on disk.
#[derive(Debug)]
pub struct RepoLock {
    #[cfg(not(target_family = "wasm"))]
    _file: std::fs::File,
    /// The holder record this lock wrote, removed on drop.
    #[cfg(not(target_family = "wasm"))]
    holder_path: PathBuf,
}

/// The holder record's path for a lock file. A separate file because on Windows
/// an exclusive lock makes the locked file itself unreadable to other processes.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn holder_path(lock_path: &Path) -> PathBuf {
    let mut name = lock_path.as_os_str().to_owned();
    name.push(".holder");
    PathBuf::from(name)
}

impl RepoLock {
    /// Acquire the exclusive repo lock, failing fast with [`VelodyError::Locked`]
    /// if another process already holds it (rather than blocking indefinitely).
    pub fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_at(&root.join(".velo/lock"))
    }

    /// Like [`RepoLock::acquire`], on an explicit lock-file path. A single-file
    /// repository locks `<file>.lock` beside its database.
    pub fn acquire_at(path: &Path) -> Result<Self> {
        match Self::try_acquire_at(path)? {
            Some(lock) => Ok(lock),
            None => {
                let holder = Self::holder_at(path);
                Err(VelodyError::Locked {
                    held_by: holder.as_ref().map(|h| h.pid),
                    holder,
                })
            }
        }
    }

    /// Try to acquire the lock. `Ok(None)` means someone else holds it, which is
    /// a normal outcome rather than an error — callers that want to wait or skip
    /// can decide for themselves.
    pub fn try_acquire(root: &Path) -> Result<Option<Self>> {
        Self::try_acquire_at(&root.join(".velo/lock"))
    }

    /// Like [`RepoLock::try_acquire`], on an explicit lock-file path.
    pub fn try_acquire_at(path: &Path) -> Result<Option<Self>> {
        Self::try_acquire_labelled_at(path, None)
    }

    /// Like [`RepoLock::try_acquire_at`], recording `label` (usually the command
    /// line) in the holder record so a refused writer can report it.
    pub fn try_acquire_labelled_at(path: &Path, label: Option<&str>) -> Result<Option<Self>> {
        #[cfg(target_family = "wasm")]
        {
            let _ = (path, label);
            Ok(Some(RepoLock {}))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            Self::try_acquire_native(path, label)
        }
    }

    /// Read the holder record for the lock at `path`. `None` when there is no
    /// record or it cannot be parsed. The record may be stale; see [`LockHolder`].
    pub fn holder_at(path: &Path) -> Option<LockHolder> {
        #[cfg(target_family = "wasm")]
        {
            let _ = path;
            None
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let text = std::fs::read_to_string(holder_path(path)).ok()?;
            let mut lines = text.splitn(3, '\n');
            let pid = lines.next()?.trim().parse().ok()?;
            let started_at_ms = lines.next()?.trim().parse().ok()?;
            let command = lines
                .next()
                .map(|c| c.trim_end_matches(['\r', '\n']))
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            Some(LockHolder {
                pid,
                started_at_ms,
                command,
            })
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn try_acquire_native(path: &Path, label: Option<&str>) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(VelodyError::Io)?;

        match file.try_lock_exclusive() {
            Ok(()) => {
                let holder_path = holder_path(path);
                // Best effort: the record is a courtesy, never a reason to
                // refuse a lock we already hold.
                let mut record = format!(
                    "{}\n{}\n",
                    std::process::id(),
                    crate::commands::snapshot_timestamp_ms()
                );
                if let Some(label) = label {
                    record.push_str(&label.replace(['\r', '\n'], " "));
                }
                let _ = crate::storage::write_atomic(&holder_path, record.as_bytes());
                Ok(Some(RepoLock {
                    _file: file,
                    holder_path,
                }))
            }
            Err(_) => Ok(None),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl Drop for RepoLock {
    /// Remove the holder record. Runs before `_file` drops, so the record is
    /// gone before the OS lock is released and never outlives a clean release.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.holder_path);
    }
}
