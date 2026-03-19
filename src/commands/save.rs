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

/// Snapshot the current working directory.
///
/// Returns `Ok(None)` when the directory is clean (nothing to save).
/// Returns `Ok(Some(result))` on success.
pub fn run(root: &Path, message: &str) -> Result<Option<SaveResult>> {
    // ── Validate message ─────────────────────────────────────────────────────
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
    let modified_count = dirty.values().filter(|s| **s == FileStatus::Modified).count();
    let deleted_count = dirty.values().filter(|s| **s == FileStatus::Deleted).count();

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // ── Hash new / modified files in parallel ────────────────────────────────
    let objects_dir = root.join(".velo/objects");
    let files_to_hash: Vec<String> = dirty
        .iter()
        .filter(|(_, status)| **status != FileStatus::Deleted)
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

    // ── Write to DB inside a transaction ─────────────────────────────────────
    let tx = conn.transaction()?;

    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![snapshot_hash, message, branch.trim(), parent_hash.trim()],
    )?;

    let modified_and_deleted: Vec<String> = dirty.keys().cloned().collect();
    
    if !parent_hash.trim().is_empty() {
        if modified_and_deleted.is_empty() {
            // Edge case: literally nothing changed, just copy everything
            tx.execute(
                "INSERT INTO file_map (snapshot_hash, path, hash) 
                 SELECT ?, path, hash FROM file_map WHERE snapshot_hash = ?",
                params![snapshot_hash, parent_hash.trim()],
            )?;
        } else {
            // Process in chunks to stay under SQLite's parameter limit (usually 999)
            for chunk in modified_and_deleted.chunks(900) {
                let placeholders = vec!["?"; chunk.len()].join(",");
                let sql = format!(
                    "INSERT INTO file_map (snapshot_hash, path, hash) 
                     SELECT ?, path, hash FROM file_map 
                     WHERE snapshot_hash = ? AND path NOT IN ({})",
                    placeholders
                );

                let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                sql_params.push(Box::new(snapshot_hash.to_string()));
                sql_params.push(Box::new(parent_hash.trim().to_string()));
                for item in chunk {
                    sql_params.push(Box::new(item.clone()));
                }
                tx.execute(&sql, rusqlite::params_from_iter(sql_params))?;
            }
        }
    }

    // Insert newly hashed files
    for (rel, hash) in &hashed_files {
        tx.execute(
            "INSERT INTO file_map (snapshot_hash, path, hash) VALUES (?, ?, ?)",
            params![snapshot_hash, rel, hash],
        )?;
    }

    // A new save invalidates the redo stack for this branch
    tx.execute(
        "DELETE FROM trash WHERE branch = ?",
        [branch.trim()],
    )?;

    tx.commit()?;
    fs::write(root.join(".velo/PARENT"), snapshot_hash)?;

    Ok(Some(SaveResult {
        hash: snapshot_hash.to_string(),
        new_count,
        modified_count,
        deleted_count,
    }))
}