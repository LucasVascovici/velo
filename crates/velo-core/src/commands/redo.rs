//! `velo redo` — restore the most recently undone snapshot on this branch.
//!
//! Moves the snapshot back out of the trash, along with any tags `undo` shelved
//! with it. Returns what happened as data; wording lives in `velo-cli`.

use std::fs;

use rusqlite::OptionalExtension;

use crate::commands::get_dirty_files;
use crate::error::{InProgress, Result, VeloError};
use crate::WriteGuard;

/// What a redo did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The snapshot that was brought back, and where the branch now sits.
    pub snapshot: String,
    /// Its message.
    pub message: String,
}

/// Restore the most recently undone snapshot on the current branch.
pub fn run(guard: &WriteGuard) -> Result<Outcome> {
    let root = guard.root();
    // Checked before dirtiness: a merge leaves the tree dirty by design, so
    // testing dirtiness first would blame the wrong thing.
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
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

    let snap: Option<(String, String)> = conn
        .query_row(
            "SELECT hash, message FROM trash WHERE branch = ? ORDER BY deleted_at DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (snapshot, message) = snap.ok_or_else(|| {
        VeloError::invalid(format!("Nothing to redo on branch '{}'.", branch.trim()))
    })?;

    let tx = guard.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at
         FROM trash WHERE hash = ?",
        [&snapshot],
    )?;
    tx.execute("DELETE FROM trash WHERE hash = ?", [&snapshot])?;
    tx.execute(
        "INSERT OR REPLACE INTO tags (name, snapshot_hash)
         SELECT name, snapshot_hash FROM trash_tags WHERE snapshot_hash = ?",
        [&snapshot],
    )?;
    tx.execute(
        "DELETE FROM trash_tags WHERE snapshot_hash = ?",
        [&snapshot],
    )?;
    tx.commit()?;

    // restore::run writes PARENT itself.
    crate::commands::restore::run(guard, &snapshot, true, &[])?;
    Ok(Outcome { snapshot, message })
}
