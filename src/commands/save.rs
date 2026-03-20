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
///
/// Returns `Ok(None)` when the directory is clean.
pub fn run(root: &Path, message: &str) -> Result<Option<SaveResult>> {
    let message = message.trim();
    if message.is_empty() {
        return Err(VeloError::InvalidInput(
            "Snapshot message cannot be empty. Use: velo save \"<description>\"".into(),
        ));
    }

    let dirty = crate::commands::get_dirty_files(root);
    if dirty.is_empty() {
        println!(
            "{}",
            console::style("Working directory clean. Nothing to save.").dim()
        );
        return Ok(None);
    }

    let new_count = dirty.values().filter(|s| **s == FileStatus::New).count();
    let modified_count = dirty
        .values()
        .filter(|s| **s == FileStatus::Modified)
        .count();
    let deleted_count = dirty
        .values()
        .filter(|s| **s == FileStatus::Deleted)
        .count();

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // ── Parallel hash + compress new/modified files ───────────────────────────
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
    let full_hex = blake3::hash(
        format!("{}{}{}{}", message, branch.trim(), parent_hash.trim(), now).as_bytes(),
    )
    .to_hex()
    .to_string();
    let snapshot_hash = &full_hex[..SNAP_HASH_LEN];

    // ── Single transaction for all DB writes ──────────────────────────────────
    let tx = conn.transaction()?;

    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![snapshot_hash, message, branch.trim(), parent_hash.trim()],
    )?;

    // Copy forward unchanged files from the parent snapshot (delta storage).
    // Prepare the insert once and reuse it for every row.
    {
        let modified_paths: HashSet<&str> = hashed_files.iter().map(|(p, _)| p.as_str()).collect();

        let parent_files: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
            let collected: Vec<(String, String)> = stmt
                .query_map([parent_hash.trim()], |r| {
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

        // Reuse a single prepared statement for all inserts
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

    Ok(Some(SaveResult {
        hash: snapshot_hash.to_string(),
        new_count,
        modified_count,
        deleted_count,
    }))
}
