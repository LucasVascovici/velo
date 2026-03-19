use std::fs;
use std::path::Path;

use console::style;
use rusqlite::OptionalExtension;

use crate::commands::get_dirty_files;
use crate::db;
use crate::error::{Result, VeloError};

pub fn run(root: &Path) -> Result<()> {
    // ── Safety: refuse to redo with a dirty working tree ─────────────────────
    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Redo aborted: {} unsaved change(s). Save or discard them first.",
            dirty.len()
        )));
    }

    // ── Can't redo during a merge ─────────────────────────────────────────────
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::InvalidInput(
            "Redo is not available while a merge is in progress.".into(),
        ));
    }

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());

    // ── Find the most recently trashed snapshot for this branch ──────────────
    let snap: Option<(String, String)> = conn
        .query_row(
            "SELECT hash, message FROM trash WHERE branch = ? ORDER BY deleted_at DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let (hash, message) = snap.ok_or_else(|| {
        VeloError::InvalidInput(format!(
            "Nothing to redo on branch '{}'.",
            branch.trim()
        ))
    })?;

    println!(
        "Redoing snapshot {} — \"{}\"",
        style(&hash).yellow(),
        style(&message).dim()
    );

    // ── Move snapshot back from trash into snapshots ──────────────────────────
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, created_at)
         SELECT hash, message, branch, parent_hash, created_at FROM trash WHERE hash = ?",
        [&hash],
    )?;
    tx.execute("DELETE FROM trash WHERE hash = ?", [&hash])?;
    tx.commit()?;

    // ── Restore working tree (restore::run writes PARENT itself) ─────────────
    crate::commands::restore::run(root, &hash, true)?;

    println!(
        "{} Redo complete — now at snapshot {}",
        style("✔").green().bold(),
        style(&hash).yellow()
    );

    Ok(())
}