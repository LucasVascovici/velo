use std::fs;
use std::path::{Path, PathBuf};

use console::style;
use rayon::prelude::*;
use rusqlite::params;

use crate::commands::{get_dirty_files, get_tracked_files, remove_empty_parents};
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(root: &Path, snapshot_hash: &str, force: bool) -> Result<()> {
    // ── No-op guard ───────────────────────────────────────────────────────────
    // Skip only when PARENT already points here AND the tree is clean.
    let current_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    if current_parent.trim() == snapshot_hash {
        let dirty = get_dirty_files(root);
        if dirty.is_empty() {
            println!(
                "{} Already at snapshot {}. Nothing to do.",
                style("✔").green(),
                style(snapshot_hash).yellow()
            );
            return Ok(());
        }
        // Dirty files exist but PARENT is already correct — overwrite disk
        // contents (happens on force-switch with local unsaved edits).
    }

    // ── Dirty-check ───────────────────────────────────────────────────────────
    let dirty = get_dirty_files(root);
    if !force && !dirty.is_empty() {
        println!(
            "{} Restore aborted: {} unsaved change(s):",
            style("✖").red().bold(),
            dirty.len()
        );
        let mut keys: Vec<_> = dirty.keys().collect();
        keys.sort();
        for k in keys {
            println!("  {}", style(k).yellow());
        }
        println!(
            "Use {} to discard them.",
            style(format!("velo restore {} --force", snapshot_hash)).cyan()
        );
        return Err(VeloError::InvalidInput(
            "Restore aborted: unsaved changes present. Use --force to discard them.".into(),
        ));
    }

    if force && !dirty.is_empty() {
        println!(
            "{} Discarding {} unsaved change(s).",
            style("!").yellow().bold(),
            dirty.len()
        );
    }

    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Verify snapshot exists
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

    // Load target snapshot's file map
    let snapshot_files: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
        let collected: Vec<(String, String)> = stmt
            .query_map(params![snapshot_hash], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    let snapshot_set: std::collections::HashSet<&str> =
        snapshot_files.iter().map(|(p, _)| p.as_str()).collect();

    let objects_dir = root.join(".velo/objects");

    // ── Remove ghost files in parallel ────────────────────────────────────────
    let current_files = get_tracked_files(root);
    let ghosts: Vec<_> = current_files
        .iter()
        .filter(|p| {
            let rel = crate::db::normalise(p.strip_prefix(root).unwrap().to_str().unwrap());
            !snapshot_set.contains(rel.as_str())
        })
        .collect();

    let ghost_count = ghosts.len();
    if ghost_count > 0 {
        // Collect parent dirs before removal
        let ghost_parents: Vec<PathBuf> = ghosts
            .iter()
            .filter_map(|p| p.parent().map(|d| d.to_path_buf()))
            .collect();

        // Remove in parallel
        ghosts.par_iter().for_each(|p| {
            let _ = fs::remove_file(p);
        });

        // Clean up empty directories (sequential — avoids TOCTOU on nested dirs)
        for dir in ghost_parents {
            remove_empty_parents(&dir, root);
        }

        println!(
            "  {} Removed {} ghost file(s).",
            style("~").yellow(),
            ghost_count
        );
    }

    // ── Write snapshot files in parallel ─────────────────────────────────────
    // create_dir_all is safe to call concurrently (idempotent).
    let write_errors: Vec<String> = snapshot_files
        .par_iter()
        .filter_map(|(rel_path, hash)| {
            let full_path = root.join(crate::db::db_to_path(rel_path));
            if let Some(parent) = full_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Some(format!("mkdir '{}': {}", rel_path, e));
                }
            }
            match storage::read_object(&objects_dir, hash) {
                Ok(data) => match fs::write(&full_path, &data) {
                    Ok(_) => None,
                    Err(e) => Some(format!(
                        "write '{}': {}  (is the file locked?)",
                        rel_path, e
                    )),
                },
                Err(e) => Some(format!("read object for '{}': {}", rel_path, e)),
            }
        })
        .collect();

    if !write_errors.is_empty() {
        for err in &write_errors {
            eprintln!("{} {}", style("error:").red().bold(), err);
        }
        return Err(VeloError::InvalidInput(format!(
            "{} file(s) could not be written.",
            write_errors.len()
        )));
    }

    // ── Invalidate index cache for all written paths ──────────────────────────
    // The files just written have new mtimes; stale cache entries would cause
    // `get_dirty_files` to report them as clean even if they differ.
    let written_paths: Vec<String> = snapshot_files.iter().map(|(p, _)| p.clone()).collect();
    crate::commands::invalidate_cache_entries(root, &written_paths);

    // ── Write PARENT last (best-effort atomicity) ─────────────────────────────
    fs::write(root.join(".velo/PARENT"), snapshot_hash)?;

    // ── Success message ───────────────────────────────────────────────────────
    let (message, branch): (String, String) = conn
        .query_row(
            "SELECT message, branch FROM snapshots WHERE hash = ?",
            [snapshot_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_else(|_| ("(unknown)".into(), "(unknown)".into()));

    println!(
        "{} Restored to {} on {} — \"{}\"",
        style("✔").green().bold(),
        style(snapshot_hash).yellow(),
        style(&branch).cyan(),
        style(&message).white()
    );

    Ok(())
}
