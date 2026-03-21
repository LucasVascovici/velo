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

use crate::commands::{get_dirty_files, get_tracked_files, FileStatus, SNAP_HASH_LEN};
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
        name.unwrap_or_else(|| format!("stash-{}", &Utc::now().format("%Y%m%d-%H%M%S")));

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

    let hashed: Result<Vec<(String, String)>> = files_to_hash
        .into_par_iter()
        .map(|rel| {
            let h = storage::hash_and_compress(&root.join(&rel), &objects_dir)?;
            Ok((rel, h))
        })
        .collect();
    let hashed = hashed?;

    // Build snapshot hash
    let now = Utc::now().to_rfc3339();
    let full_hex = blake3::hash(
        format!(
            "stash{}{}{}{}",
            shelf_name,
            branch.trim(),
            parent_hash.trim(),
            now
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let snap_hash = &full_hex[..SNAP_HASH_LEN];

    let tx = conn.transaction()?;

    // Insert a snapshot row on the hidden '_stash' branch
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash) VALUES (?, ?, '_stash', ?)",
        params![
            snap_hash,
            format!("stash: {}", shelf_name),
            parent_hash.trim()
        ],
    )?;

    // Copy unchanged files from parent, insert hashed files
    {
        let parent_files: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
            let collected: Vec<(String, String)> = stmt
                .query_map([parent_hash.trim()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter(|(p, _)| dirty.get(p.as_str()) != Some(&FileStatus::Deleted))
                .filter(|(p, _)| !hashed.iter().any(|(rp, _)| rp == p))
                .collect();
            collected
        };
        let mut ins =
            tx.prepare("INSERT INTO file_map (snapshot_hash, path, hash) VALUES (?, ?, ?)")?;
        for (p, h) in &parent_files {
            ins.execute(params![snap_hash, p, h])?;
        }
        for (rel, h) in &hashed {
            ins.execute(params![snap_hash, rel, h])?;
        }
    }

    // Register the shelf
    tx.execute(
        "INSERT INTO stash (name, snapshot_hash, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![shelf_name, snap_hash, branch.trim(), parent_hash.trim()],
    )?;

    tx.commit()?;

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
