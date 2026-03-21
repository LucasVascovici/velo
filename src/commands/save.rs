use std::collections::HashSet;
use std::fs;
use std::path::Path;

use rayon::prelude::*;
use rusqlite::params;

use crate::commands::{FileStatus, SNAP_HASH_LEN};
use crate::error::{Result, VeloError};
use crate::{db, storage};

#[derive(Debug)]
pub struct SaveResult {
    pub hash: String,
    pub new_count: usize,
    pub modified_count: usize,
    pub deleted_count: usize,
}

/// Snapshot the working directory.
/// Returns `Ok(None)` when there is nothing to save.
/// When `amend = true`, the most recent snapshot on this branch is replaced.
pub fn run(root: &Path, message: &str, amend: bool) -> Result<Option<SaveResult>> {
    let message = message.trim();
    if message.is_empty() {
        return Err(VeloError::InvalidInput(
            "Snapshot message cannot be empty. Use: velo save \"<description>\"".into(),
        ));
    }

    let dirty = crate::commands::get_dirty_files(root);
    if dirty.is_empty() && !amend {
        println!(
            "{}",
            console::style("Working directory clean. Nothing to save.").dim()
        );
        return Ok(None);
    }

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // ── Amend: find the snapshot to replace ──────────────────────────────────
    let amend_hash: Option<(String, String)> = if amend {
        conn.query_row(
            "SELECT hash, parent_hash FROM snapshots
             WHERE branch = ? ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    } else {
        None
    };

    // The effective parent for the new snapshot: amend keeps the original
    // snapshot's parent so history stays linear.
    let effective_parent = match &amend_hash {
        Some((_, orig_parent)) => orig_parent.trim().to_string(),
        None => parent_hash.trim().to_string(),
    };

    // ── Count changes ─────────────────────────────────────────────────────────
    let new_count = dirty.values().filter(|s| **s == FileStatus::New).count();
    let modified_count = dirty
        .values()
        .filter(|s| **s == FileStatus::Modified)
        .count();
    let deleted_count = dirty
        .values()
        .filter(|s| **s == FileStatus::Deleted)
        .count();

    // ── Parallel hash + compress ───────────────────────────────────────────────
    let objects_dir = root.join(".velo/objects");
    let files_to_hash: Vec<String> = dirty
        .iter()
        .filter(|(_, s)| **s != FileStatus::Deleted)
        .map(|(p, _)| p.clone())
        .collect();

    let hash_results: Result<Vec<(String, String)>> = files_to_hash
        .into_par_iter()
        .map(|rel| {
            let h = storage::hash_and_compress(&root.join(&rel), &objects_dir)?;
            Ok((rel, h))
        })
        .collect();
    let hashed_files = hash_results?;

    // ── Build snapshot hash ───────────────────────────────────────────────────
    let now = chrono::Utc::now().to_rfc3339();
    let full_hex =
        blake3::hash(format!("{}{}{}{}", message, branch.trim(), effective_parent, now).as_bytes())
            .to_hex()
            .to_string();
    let snapshot_hash = &full_hex[..SNAP_HASH_LEN];

    // ── DB transaction ────────────────────────────────────────────────────────
    let tx = conn.transaction()?;

    // If amending, delete the old snapshot and its file_map first
    if let Some((old_hash, _)) = &amend_hash {
        tx.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [old_hash])?;
        tx.execute("DELETE FROM snapshots   WHERE hash = ?", [old_hash])?;
        // Also remove from trash (shouldn't be there, but be safe)
        tx.execute("DELETE FROM trash WHERE hash = ?", [old_hash])?;
    }

    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![
            snapshot_hash,
            message,
            branch.trim(),
            effective_parent.as_str()
        ],
    )?;

    // Copy forward unchanged files from the effective parent
    {
        let modified_paths: HashSet<&str> = hashed_files.iter().map(|(p, _)| p.as_str()).collect();

        let parent_files: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
            let collected: Vec<(String, String)> = stmt
                .query_map([effective_parent.as_str()], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter(|(p, _)| {
                    !modified_paths.contains(p.as_str())
                        && dirty.get(p.as_str()) != Some(&FileStatus::Deleted)
                })
                .collect();
            collected
        };

        {
            let mut ins =
                tx.prepare("INSERT INTO file_map (snapshot_hash, path, hash) VALUES (?, ?, ?)")?;
            for (p, h) in &parent_files {
                ins.execute(params![snapshot_hash, p, h])?;
            }
            for (rel, hash) in &hashed_files {
                ins.execute(params![snapshot_hash, rel, hash])?;
            }
        }
    }

    // New save invalidates the redo stack for this branch
    tx.execute("DELETE FROM trash WHERE branch = ?", [branch.trim()])?;
    tx.commit()?;

    fs::write(root.join(".velo/PARENT"), snapshot_hash)?;

    // If a merge was in progress, this save finalises it — clear the merge state
    let merge_head = root.join(".velo/MERGE_HEAD");
    if merge_head.exists() {
        let _ = fs::remove_file(&merge_head);
    }

    Ok(Some(SaveResult {
        hash: snapshot_hash.to_string(),
        new_count,
        modified_count,
        deleted_count,
    }))
}
