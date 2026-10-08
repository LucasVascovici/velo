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

#[cfg(not(target_family = "wasm"))]
use std::fs::OpenOptions;
use std::path::Path;

#[cfg(not(target_family = "wasm"))]
use fs2::FileExt;

use crate::error::{Result, VeloError};

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
}

impl RepoLock {
    /// Acquire the exclusive repo lock, failing fast with [`VeloError::Locked`]
    /// if another process already holds it (rather than blocking indefinitely).
    pub fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_at(&root.join(".velo/lock"))
    }

    /// Like [`RepoLock::acquire`], on an explicit lock-file path. A single-file
    /// repository locks `<file>.lock` beside its database.
    pub fn acquire_at(path: &Path) -> Result<Self> {
        match Self::try_acquire_at(path)? {
            Some(lock) => Ok(lock),
            None => Err(VeloError::Locked { held_by: None }),
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
        #[cfg(target_family = "wasm")]
        {
            let _ = path;
            Ok(Some(RepoLock {}))
        }
        #[cfg(not(target_family = "wasm"))]
        {
            Self::try_acquire_native(path)
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn try_acquire_native(path: &Path) -> Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(VeloError::Io)?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(RepoLock { _file: file })),
            Err(_) => Ok(None),
        }
    }
}
