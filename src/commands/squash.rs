//! `velo squash <n>` — collapse the last N snapshots on the current branch
//! into a single snapshot with a new message.
//!
//! The combined snapshot has the same files as HEAD, the parent of the
//! oldest squashed snapshot as its parent, and is written atomically.

use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::commands::SNAP_HASH_LEN;
use crate::db;
use crate::error::{Result, VeloError};

pub fn run(root: &Path, count: usize, message: &str) -> Result<()> {
    if count < 2 {
        return Err(VeloError::InvalidInput(
            "squash requires at least 2 snapshots.".into(),
        ));
    }

    let dirty = crate::commands::get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Squash aborted: {} unsaved change(s). Save or discard first.",
            dirty.len()
        )));
    }

    let branch_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let branch = branch_raw.trim();
    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Load the last `count` snapshots on this branch (newest first)
    let mut stmt = conn.prepare(
        "WITH RECURSIVE anc(hash, message, parent_hash, created_at, rowid, depth) AS (
            SELECT hash, message, parent_hash, created_at, rowid, 0
            FROM snapshots
            WHERE branch = ?1
              AND hash = (SELECT hash FROM snapshots WHERE branch = ?1
                          ORDER BY created_at DESC, rowid DESC LIMIT 1)
            UNION ALL
            SELECT s.hash, s.message, s.parent_hash, s.created_at, s.rowid, a.depth + 1
            FROM snapshots s JOIN anc a ON s.hash = a.parent_hash
            WHERE a.depth < ?2 AND s.branch = ?1
        )
        SELECT hash, message, parent_hash FROM anc ORDER BY depth ASC LIMIT ?3",
    )?;

    let rows: Vec<(String, String, String)> = stmt
        .query_map(params![branch, count as i64 - 1, count as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    if rows.len() < count {
        return Err(VeloError::InvalidInput(format!(
            "Branch '{}' only has {} snapshot(s) but squash needs {}.",
            branch,
            rows.len(),
            count
        )));
    }

    let head_hash  = &rows[0].0;
    let new_parent = &rows[rows.len() - 1].2; // parent of the oldest squashed snapshot
    let squashed_hashes: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();

    println!(
        "\n{} Squashing {} snapshots on '{}'…",
        style("◆").cyan().bold(),
        count,
        style(branch).cyan()
    );
    for (i, (h, msg, _)) in rows.iter().enumerate() {
        let marker = if i == 0 { "HEAD" } else { "    " };
        println!("  {} {} {}", style(marker).dim(), style(&h[..8]).yellow(), style(msg).dim());
    }
    println!(
        "  {} → new snapshot",
        style("─".repeat(40)).dim()
    );

    // Build the new snapshot hash (same content as HEAD, new message, new parent)
    let now = chrono::Utc::now().to_rfc3339();
    let full_hex = blake3::hash(
        format!("{}{}{}{}", message.trim(), branch, new_parent, now).as_bytes(),
    )
    .to_hex()
    .to_string();
    let new_hash = &full_hex[..SNAP_HASH_LEN];

    let tx = conn.transaction()?;

    // Insert new snapshot
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent)
         VALUES (?, ?, ?, ?, '')",
        params![new_hash, message.trim(), branch, new_parent],
    )?;

    // Copy file_map from HEAD to the new snapshot
    tx.execute(
        "INSERT INTO file_map (snapshot_hash, path, hash)
         SELECT ?, path, hash FROM file_map WHERE snapshot_hash = ?",
        params![new_hash, head_hash],
    )?;

    // Remove all squashed snapshots and their file maps
    // (Objects are left in the store; gc will clean them up)
    for h in &squashed_hashes {
        tx.execute("DELETE FROM file_map  WHERE snapshot_hash = ?", [h])?;
        tx.execute("DELETE FROM snapshots WHERE hash = ?",          [h])?;
    }

    // Redirect any tag pointing at a squashed snapshot to the new one
    for h in &squashed_hashes {
        tx.execute(
            "UPDATE tags SET snapshot_hash = ? WHERE snapshot_hash = ?",
            params![new_hash, h],
        )?;
    }

    tx.commit()?;

    // Update PARENT to point at the new snapshot
    fs::write(root.join(".velo/PARENT"), new_hash)?;

    println!(
        "{} Squashed into {} — \"{}\"",
        style("✔").green().bold(),
        style(new_hash).yellow(),
        message.trim()
    );

    Ok(())
}