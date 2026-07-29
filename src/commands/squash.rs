//! `velo squash <n>` — collapse the last N snapshots on the current branch
//! into a single snapshot with a new message.
//!
//! The combined snapshot has the same files as HEAD, the parent of the
//! oldest squashed snapshot as its parent, and is written atomically.

use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

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

    let head_hash = &rows[0].0;
    let new_parent = &rows[rows.len() - 1].2; // parent of the oldest squashed snapshot
    let squashed_hashes: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();

    // ── Safety: refuse if a snapshot outside the range depends on one inside ──
    // Another branch (or a merge commit) may point at a snapshot in the squash
    // range via parent_hash / merge_parent. Deleting it would orphan that
    // history, so refuse rather than corrupt the graph.
    {
        let placeholders = vec!["?"; squashed_hashes.len()].join(",");
        let sql = format!(
            "SELECT DISTINCT branch FROM snapshots
             WHERE hash NOT IN ({ph})
               AND (parent_hash IN ({ph}) OR merge_parent IN ({ph}))",
            ph = placeholders
        );
        // Three IN clauses, each taking the full set of squashed hashes in order.
        let mut params_vec: Vec<&dyn rusqlite::ToSql> =
            Vec::with_capacity(squashed_hashes.len() * 3);
        for _ in 0..3 {
            for h in &squashed_hashes {
                params_vec.push(h);
            }
        }
        let mut stmt = conn.prepare(&sql)?;
        let dependants: Vec<String> = stmt
            .query_map(params_vec.as_slice(), |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        if !dependants.is_empty() {
            return Err(VeloError::InvalidInput(format!(
                "Cannot squash: snapshot(s) in the range are the base of branch(es) [{}]. \
                 Squashing would orphan that history.",
                dependants.join(", ")
            )));
        }
    }

    println!(
        "\n{} Squashing {} snapshots on '{}'…",
        style("◆").cyan().bold(),
        count,
        style(branch).cyan()
    );
    for (i, (h, msg, _)) in rows.iter().enumerate() {
        let marker = if i == 0 { "HEAD" } else { "    " };
        println!(
            "  {} {} {}",
            style(marker).dim(),
            style(&h[..8]).yellow(),
            style(msg).dim()
        );
    }
    println!("  {} → new snapshot", style("─".repeat(40)).dim());

    // The squashed snapshot has the same tree as HEAD (the net result), a new
    // message, and the oldest squashed snapshot's parent. Capture HEAD's tree
    // before touching the DB, then content-address the new snapshot.
    let tree: Vec<(String, String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
        let collected = stmt
            .query_map([head_hash.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };
    let timestamp = crate::commands::snapshot_timestamp();
    let new_hash = crate::commands::snapshot_id(&tree, new_parent, "", message.trim(), &timestamp);
    let new_hash = new_hash.as_str();

    let tx = conn.transaction()?;

    // Insert new snapshot
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at)
         VALUES (?, ?, ?, ?, '', ?)",
        params![new_hash, message.trim(), branch, new_parent, timestamp],
    )?;

    // Copy the captured tree into the new snapshot's file_map.
    {
        let mut ins = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;
        for (p, h, m) in &tree {
            ins.execute(params![new_hash, p, h, m])?;
        }
    }

    // Remove all squashed snapshots and their file maps
    // (Objects are left in the store; gc will clean them up)
    for h in &squashed_hashes {
        tx.execute("DELETE FROM file_map  WHERE snapshot_hash = ?", [h])?;
        tx.execute("DELETE FROM snapshots WHERE hash = ?", [h])?;
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
    crate::storage::write_atomic(&root.join(".velo/PARENT"), new_hash.as_bytes())?;

    println!(
        "{} Squashed into {} — \"{}\"",
        style("✔").green().bold(),
        style(new_hash).yellow(),
        message.trim()
    );

    Ok(())
}
