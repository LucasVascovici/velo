//! `velo undo` — remove the newest snapshot on this branch, keeping it recoverable.
//!
//! The snapshot is moved to the trash rather than deleted, along with any tags
//! pointing at it, so `velo redo` can put it back. Returns what happened as data;
//! wording lives in `velo-cli`.

use std::fs;

use rusqlite::OptionalExtension;

use crate::commands::get_dirty_files;
use crate::error::{InProgress, Result, VeloError};
use crate::WriteGuard;

/// What an undo did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The snapshot that was shelved.
    pub snapshot: String,
    /// Its message.
    pub message: String,
    /// Where the branch now sits. `None` when the root snapshot was undone, in
    /// which case the working tree was emptied of tracked files.
    pub now_at: Option<String>,
}

impl Outcome {
    /// True when undoing removed the branch's only snapshot.
    pub fn cleared_working_tree(&self) -> bool {
        self.now_at.is_none()
    }
}

/// Shelve the newest snapshot on the current branch.
pub fn run(guard: &WriteGuard) -> Result<Outcome> {
    let root = guard.root();
    // A merge or rebase leaves MERGE_HEAD / REBASE_STATE and conflict rows
    // behind; removing the tip underneath them produces an inconsistent
    // repository. Checked before dirtiness, since both leave the tree dirty.
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
        });
    }
    if root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Rebase,
        });
    }

    let dirty = get_dirty_files(guard.repo());
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }

    let conn = guard.conn();
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());

    let snap: Option<(String, String, String)> = conn
        .query_row(
            "SELECT hash, message, parent_hash
             FROM snapshots
             WHERE branch = ?
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (snapshot, message, parent_hash) = snap.ok_or_else(|| {
        VeloError::invalid(format!("Nothing to undo on branch '{}'.", branch.trim()))
    })?;

    // One transaction: the snapshot and its tags move together, and file_map is
    // deliberately left in place so redo can restore the tree.
    let tx = guard.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO trash (hash, message, branch, parent_hash, merge_parent, created_at)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at
         FROM snapshots WHERE hash = ?",
        [&snapshot],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO trash_tags (name, snapshot_hash)
         SELECT name, snapshot_hash FROM tags WHERE snapshot_hash = ?",
        [&snapshot],
    )?;
    tx.execute("DELETE FROM tags WHERE snapshot_hash = ?", [&snapshot])?;
    tx.execute("DELETE FROM snapshots WHERE hash = ?", [&snapshot])?;
    tx.commit()?;

    let now_at = parent_hash.trim().to_string();
    if now_at.is_empty() {
        // The root snapshot went, so there is nothing to restore to: clear the
        // position and remove the files it tracked.
        crate::storage::write_atomic(&root.join(".velo/PARENT"), b"")?;
        for path in crate::commands::get_tracked_files(root) {
            let _ = fs::remove_file(&path);
        }
        return Ok(Outcome {
            snapshot,
            message,
            now_at: None,
        });
    }

    // restore::run writes PARENT itself.
    crate::commands::restore::run(guard, &now_at, true, &[])?;
    Ok(Outcome {
        snapshot,
        message,
        now_at: Some(now_at),
    })
}
