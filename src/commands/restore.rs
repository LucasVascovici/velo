use std::fs;
use std::path::Path;

use rusqlite::params;

use crate::commands::{get_dirty_files, get_tracked_files, remove_empty_parents};
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(root: &Path, snapshot_hash: &str, force: bool) -> Result<()> {
    // ── No-op guard ───────────────────────────────────────────────────────────
    // Only skip if PARENT already points here AND the working tree is clean.
    // If force=true there may be dirty files that need to be overwritten even
    // though PARENT hasn't changed (e.g. switching branches with unsaved edits).
    let current_parent =
        fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    if current_parent.trim() == snapshot_hash {
        let dirty = get_dirty_files(root);
        if dirty.is_empty() {
            println!(
                "{} Already at snapshot {}. Nothing to do.",
                console::style("✔").green(),
                console::style(snapshot_hash).yellow()
            );
            return Ok(());
        }
        // Dirty files exist but PARENT is already correct — fall through to
        // overwrite disk contents (happens on force-switch with local edits).
    }

    // ── Dirty-check ───────────────────────────────────────────────────────────
    let dirty = get_dirty_files(root);
    if !force && !dirty.is_empty() {
        println!(
            "{} Restore aborted: {} unsaved change(s):",
            console::style("✖").red().bold(),
            dirty.len()
        );
        let mut keys: Vec<_> = dirty.keys().collect();
        keys.sort();
        for k in keys {
            println!("  {}", console::style(k).yellow());
        }
        println!(
            "Use {} to discard them.",
            console::style("velo restore <target> --force").cyan()
        );
        return Ok(());
    }

    if force && !dirty.is_empty() {
        println!(
            "{} Discarding {} unsaved change(s).",
            console::style("!").yellow().bold(),
            dirty.len()
        );
    }

    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Load the target snapshot's file map
    let mut stmt =
        conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
    let snapshot_files: std::collections::HashMap<String, String> = stmt
        .query_map(params![snapshot_hash], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Verify snapshot exists in the database
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
        [snapshot_hash],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(VeloError::InvalidInput(format!(
            "Snapshot '{}' does not exist.",
            snapshot_hash
        )));
    }

    let objects_dir = root.join(".velo/objects");

    // ── Remove ghost files (files on disk not in target snapshot) ─────────────
    let current_files = get_tracked_files(root);
    let mut ghost_count = 0usize;
    let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();

    for path in &current_files {
        let rel = crate::db::normalise(
            path.strip_prefix(root).unwrap().to_str().unwrap(),
        );
        if !snapshot_files.contains_key(&rel) {
            fs::remove_file(path)?;
            if let Some(parent) = path.parent() {
                dirs_to_check.push(parent.to_path_buf());
            }
            ghost_count += 1;
        }
    }

    // Clean up any directories that became empty
    for dir in dirs_to_check {
        remove_empty_parents(&dir, root);
    }

    if ghost_count > 0 {
        println!(
            "  {} Removed {} ghost file(s) not present in this snapshot.",
            console::style("~").yellow(),
            ghost_count
        );
    }

    // ── Write snapshot files to disk ──────────────────────────────────────────
    for (rel_path, hash) in &snapshot_files {
        let full_path = root.join(crate::db::db_to_path(rel_path));
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = storage::read_object(&objects_dir, hash)?;
        fs::write(&full_path, data).map_err(|e| {
            VeloError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to write '{}': {}. Is the file locked by another process?",
                    rel_path, e
                ),
            ))
        })?;
    }

    // ── Update PARENT last (best-effort atomicity) ────────────────────────────
    fs::write(root.join(".velo/PARENT"), snapshot_hash)?;

    // ── Fetch snapshot message for the success line ───────────────────────────
    let (message, branch): (String, String) = conn
        .query_row(
            "SELECT message, branch FROM snapshots WHERE hash = ?",
            [snapshot_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_else(|_| ("(unknown)".into(), "(unknown)".into()));

    println!(
        "{} Restored to {} on {} — \"{}\"",
        console::style("✔").green().bold(),
        console::style(snapshot_hash).yellow(),
        console::style(&branch).cyan(),
        console::style(&message).white()
    );

    Ok(())
}