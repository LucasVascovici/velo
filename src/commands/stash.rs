//! `velo stash` — shelve and restore dirty working-tree state.
//!
//! Unlike Git's stash (which uses a hidden ref with cryptic `stash@{N}` names),
//! Velo stash shelves are named explicitly and listed in a dedicated table.
//!
//! Subcommands
//!   velo stash [<name>]          push current dirty state onto a named shelf
//!   velo stash list              list all shelves
//!   velo stash pop [<name>]      restore the most recent shelf (or named one)
//!   velo stash drop [<name>]     delete a shelf without restoring it
//!   velo stash show [<name>]     show what a shelf contains

use std::fs;
use std::path::Path;

use chrono::Utc;
use console::style;
use rayon::prelude::*;
use rusqlite::params;

use crate::commands::{get_dirty_files, get_tracked_files, FileStatus};
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

// ─── push ────────────────────────────────────────────────────────────────────

pub fn push(root: &Path, name: Option<String>) -> Result<()> {
    let dirty = get_dirty_files(root);
    if dirty.is_empty() {
        println!(
            "{}",
            style("Working directory clean — nothing to stash.").dim()
        );
        return Ok(());
    }

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // Auto-generate a name if none supplied
    let shelf_name =
        name.unwrap_or_else(|| format!("stash-{}", Utc::now().format("%Y%m%d-%H%M%S")));

    // Check for name collision
    let existing: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM stash WHERE name = ?)",
        [&shelf_name],
        |r| r.get(0),
    )?;
    if existing {
        return Err(VeloError::InvalidInput(format!(
            "A shelf named '{}' already exists. Use a different name or drop it first.",
            shelf_name
        )));
    }

    // Hash + compress all dirty (new/modified) files in parallel
    let objects_dir = root.join(".velo/objects");
    let files_to_hash: Vec<String> = dirty
        .iter()
        .filter(|(_, s)| **s != FileStatus::Deleted)
        .map(|(p, _)| p.clone())
        .collect();

    let hashed: Result<Vec<(String, String, i64)>> = files_to_hash
        .into_par_iter()
        .map(|rel| {
            let full = root.join(&rel);
            let mode = storage::capture_mode(&full);
            let hash = if mode == storage::MODE_SYMLINK {
                storage::store_raw(&objects_dir, &storage::read_symlink_target(&full)?)?
            } else {
                storage::hash_and_compress(&full, &objects_dir)?
            };
            Ok((rel, hash, mode))
        })
        .collect();
    let hashed = hashed?;

    // Assemble the stash's tree: unchanged files carried from the parent plus
    // the freshly hashed dirty files.
    let mut tree: Vec<(String, String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
        let collected: Vec<(String, String, i64)> = stmt
            .query_map([parent_hash.trim()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter(|(p, _, _)| dirty.get(p.as_str()) != Some(&FileStatus::Deleted))
            .filter(|(p, _, _)| !hashed.iter().any(|(rp, _, _)| rp == p))
            .collect();
        collected
    };
    tree.extend(hashed.iter().cloned());

    // Content-addressed id for the stash snapshot.
    let message = format!("stash: {}", shelf_name);
    let timestamp = crate::commands::snapshot_timestamp();
    let snap_hash =
        crate::commands::snapshot_id(&tree, parent_hash.trim(), "", &message, &timestamp);
    let snap_hash = snap_hash.as_str();

    let tx = conn.transaction()?;

    // Insert a snapshot row on the hidden '_stash' branch
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, created_at)
         VALUES (?, ?, '_stash', ?, ?)",
        params![snap_hash, message, parent_hash.trim(), timestamp],
    )?;

    {
        let mut ins = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;
        for (p, h, m) in &tree {
            ins.execute(params![snap_hash, p, h, m])?;
        }
    }

    // Register the shelf
    tx.execute(
        "INSERT INTO stash (name, snapshot_hash, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![shelf_name, snap_hash, branch.trim(), parent_hash.trim()],
    )?;

    tx.commit()?;

    // Clear the brand-new files we just shelved. `restore` deliberately leaves
    // untracked files alone (they exist in no snapshot, so removing them would
    // be unrecoverable) — but here they *are* safely stored in the shelf, so
    // clearing them is what "shelve my work" means.
    for (rel, status) in &dirty {
        if *status == FileStatus::New {
            let full = root.join(db::db_to_path(rel));
            let _ = fs::remove_file(&full);
            if let Some(parent) = full.parent() {
                crate::commands::remove_empty_parents(parent, root);
            }
        }
    }

    // Restore working tree to the clean parent state
    if parent_hash.trim().is_empty() {
        // No parent — just remove all tracked files
        for path in get_tracked_files(root) {
            let _ = fs::remove_file(&path);
        }
    } else {
        crate::commands::restore::run(root, parent_hash.trim(), true, &[])?;
    }

    let n_mod = dirty
        .values()
        .filter(|s| **s == FileStatus::Modified)
        .count();
    let n_new = dirty.values().filter(|s| **s == FileStatus::New).count();
    let n_del = dirty
        .values()
        .filter(|s| **s == FileStatus::Deleted)
        .count();

    println!(
        "{} Shelved '{}' ({} modified, {} new, {} deleted)",
        style("✔").green().bold(),
        style(&shelf_name).cyan(),
        n_mod,
        n_new,
        n_del
    );
    println!(
        "  Working tree restored to {}",
        style(parent_hash.trim()).yellow()
    );
    Ok(())
}

// ─── list ─────────────────────────────────────────────────────────────────────

pub fn list(root: &Path) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let mut stmt =
        conn.prepare("SELECT name, branch, created_at, snapshot_hash FROM stash ORDER BY id DESC")?;
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        println!("{}", style("No shelves found.").dim());
        return Ok(());
    }

    println!("{}", style("Stash shelves:").bold());
    for (name, branch, date, hash) in &rows {
        let date_short = if date.len() >= 16 { &date[..16] } else { date };
        println!(
            "  {} {} {} {}",
            style(name).cyan().bold(),
            style(format!("(on {})", branch)).dim(),
            style(date_short).dim(),
            style(&hash[..8.min(hash.len())]).yellow().dim()
        );
    }
    Ok(())
}

// ─── pop ─────────────────────────────────────────────────────────────────────

pub fn pop(root: &Path, name: Option<String>) -> Result<()> {
    apply_shelf(root, name, true)
}

// ─── drop ────────────────────────────────────────────────────────────────────

pub fn drop_shelf(root: &Path, name: Option<String>) -> Result<()> {
    apply_shelf(root, name, false)
}

fn apply_shelf(root: &Path, name: Option<String>, restore: bool) -> Result<()> {
    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Resolve shelf name
    let (shelf_name, snap_hash, saved_branch, saved_parent): (String, String, String, String) =
        if let Some(n) = name {
            conn.query_row(
                "SELECT name, snapshot_hash, branch, parent_hash FROM stash WHERE name = ?",
                [&n],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| VeloError::InvalidInput(format!("Shelf '{}' not found.", n)))?
        } else {
            conn.query_row(
                "SELECT name, snapshot_hash, branch, parent_hash FROM stash ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| VeloError::InvalidInput("No shelves found.".into()))?
        };

    if restore {
        // Safety: refuse to pop onto a dirty working tree
        let dirty = get_dirty_files(root);
        if !dirty.is_empty() {
            return Err(VeloError::InvalidInput(format!(
                "Pop aborted: {} unsaved change(s). Save or discard them first.",
                dirty.len()
            )));
        }

        let current_branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
        let current_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

        if current_branch.trim() != saved_branch {
            println!(
                "{} Note: shelf was created on branch '{}', you are on '{}'.",
                style("!").yellow(),
                style(&saved_branch).cyan(),
                style(current_branch.trim()).cyan()
            );
        }
        if current_parent.trim() != saved_parent {
            println!(
                "{} Note: the working tree has moved since this shelf was created.",
                style("!").yellow()
            );
        }

        // Apply the stash snapshot's file map onto the working tree
        let objects_dir = root.join(".velo/objects");
        let files: Vec<(String, String)> = {
            let mut stmt =
                conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
            let collected: Vec<(String, String)> = stmt
                .query_map([&snap_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        // Write stashed files to disk in parallel
        let errors: Vec<String> = files
            .par_iter()
            .filter_map(|(rel, hash)| {
                let full = root.join(db::db_to_path(rel));
                if let Some(p) = full.parent() {
                    fs::create_dir_all(p).ok()?;
                }
                match storage::read_object(&objects_dir, hash) {
                    Ok(data) => fs::write(&full, data)
                        .err()
                        .map(|e| format!("{}: {}", rel, e)),
                    Err(e) => Some(format!("{}: {}", rel, e)),
                }
            })
            .collect();

        if !errors.is_empty() {
            for e in &errors {
                eprintln!("{} {}", style("error:").red().bold(), e);
            }
            return Err(VeloError::InvalidInput(
                "Some files could not be restored from stash.".into(),
            ));
        }

        println!(
            "{} Applied shelf '{}'",
            style("✔").green().bold(),
            style(&shelf_name).cyan()
        );
    }

    // Remove the shelf entry + its snapshot row
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM stash WHERE name = ?", [&shelf_name])?;
    tx.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [&snap_hash])?;
    tx.execute("DELETE FROM snapshots WHERE hash = ?", [&snap_hash])?;
    tx.commit()?;

    if !restore {
        println!(
            "{} Dropped shelf '{}'.",
            style("✔").green(),
            style(&shelf_name).cyan()
        );
    }
    Ok(())
}

// ─── show ─────────────────────────────────────────────────────────────────────

pub fn show_shelf(root: &Path, name: Option<String>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    let (shelf_name, snap_hash, saved_branch, saved_parent): (String, String, String, String) =
        if let Some(n) = name {
            conn.query_row(
                "SELECT name, snapshot_hash, branch, parent_hash FROM stash WHERE name = ?",
                [&n],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| VeloError::InvalidInput(format!("Shelf '{}' not found.", n)))?
        } else {
            conn.query_row(
                "SELECT name, snapshot_hash, branch, parent_hash FROM stash ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| VeloError::InvalidInput("No shelves found.".into()))?
        };

    println!(
        "Shelf: {}  (from {} @ {})",
        style(&shelf_name).cyan().bold(),
        style(&saved_branch).dim(),
        style(&saved_parent[..8.min(saved_parent.len())]).yellow()
    );

    // Show the diff between saved_parent and the stash snapshot
    crate::commands::show::diff_snapshots(root, &conn, &saved_parent, &snap_hash, &None)
}
