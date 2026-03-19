use std::fs;
use std::path::Path;

use console::style;
use rusqlite::OptionalExtension;

use crate::commands::get_dirty_files;
use crate::db;
use crate::error::{Result, VeloError};

pub fn run(root: &Path) -> Result<String> {
    // ── Safety: refuse to undo with a dirty working tree ─────────────────────
    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Undo aborted: {} unsaved change(s). Save or discard them first.",
            dirty.len()
        )));
    }

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());

    // ── Find the latest snapshot on this branch ───────────────────────────────
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

    let (hash, message, parent_hash) = snap.ok_or_else(|| {
        VeloError::InvalidInput("Nothing to undo on this branch.".into())
    })?;

    println!(
        "Removing snapshot {} — \"{}\"",
        style(&hash).yellow(),
        style(&message).dim()
    );

    // ── Atomically move snapshot to trash (preserves file_map for redo) ───────
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO trash (hash, message, branch, parent_hash, created_at)
         SELECT hash, message, branch, parent_hash, created_at
         FROM snapshots WHERE hash = ?",
        [&hash],
    )?;
    // Tags on the deleted snapshot are removed (they point to non-existent snap)
    tx.execute("DELETE FROM tags WHERE snapshot_hash = ?", [&hash])?;
    tx.execute("DELETE FROM snapshots WHERE hash = ?", [&hash])?;
    tx.commit()?;

    // ── Rewind PARENT and restore working tree ───────────────────────────────
    let new_parent = parent_hash.trim().to_string();

    if new_parent.is_empty() {
        // The very first commit was undone — clear PARENT and delete all tracked files
        fs::write(root.join(".velo/PARENT"), "")?;
        for path in crate::commands::get_tracked_files(root) {
            let _ = fs::remove_file(&path);
        }
        println!(
            "{} Working tree cleared (first snapshot removed).",
            style("✔").green()
        );
    } else {
        // restore::run writes PARENT itself at the end — don't pre-write here
        crate::commands::restore::run(root, &new_parent, true)?;
    }

    Ok(format!(
        "{} Snapshot {} ('{}') removed.",
        style("✔").green(),
        style(&hash).yellow(),
        message
    ))
}