//! Repository-level advisory lock.
//!
//! Velo coordinates the SQLite database (via WAL) automatically, but the object
//! store (`.velo/objects/`) and the ref files (`PARENT`, `HEAD`, `MERGE_HEAD`)
//! are not transactional with it. Two mutating `velo` processes running at once
//! could therefore race — most dangerously `gc` deleting an object that a
//! concurrent `save` has written to disk but not yet committed to `file_map`.
//!
//! A single coarse lock, held for the duration of any mutating command,
//! serialises those operations. Read-only commands (`status`, `history`, …) do
//! not take it, so they never block. The lock is advisory (OS-level, on the
//! `.velo/lock` file handle) and is released automatically when the process
//! exits — even on crash — so it can never go stale.

use std::fs::OpenOptions;
use std::path::Path;

use fs2::FileExt;

use crate::error::{Result, VeloError};

/// An acquired repository lock. Dropping it (or the process exiting) releases
/// the underlying OS lock.
#[derive(Debug)]
pub struct RepoLock {
    _file: std::fs::File,
}

impl RepoLock {
    /// Acquire the exclusive repo lock, failing fast with [`VeloError::Locked`]
    /// if another process already holds it (rather than blocking indefinitely).
    pub fn acquire(root: &Path) -> Result<Self> {
        match Self::try_acquire(root)? {
            Some(lock) => Ok(lock),
            None => Err(VeloError::Locked { held_by: None }),
        }
    }

    /// Try to acquire the lock. `Ok(None)` means someone else holds it, which is
    /// a normal outcome rather than an error — callers that want to wait or skip
    /// can decide for themselves.
    pub fn try_acquire(root: &Path) -> Result<Option<Self>> {
        let path = root.join(".velo/lock");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(VeloError::Io)?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(RepoLock { _file: file })),
            Err(_) => Ok(None),
        }
    }
}
