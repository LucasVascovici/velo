//! `velo fsck` — verify repository integrity.
//!
//! Read-only. Checks the invariants the rest of Velo (and, later, sync) relies
//! on: every referenced object exists and re-hashes to its own name; every
//! snapshot's parents resolve; content-addressed snapshot ids recompute
//! correctly; and every ref (PARENT, tags, stash, conflicts) points somewhere
//! real. Prints a report and exits non-zero if anything is wrong.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use console::style;

use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(root: &Path) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let objects_dir = root.join(".velo/objects");
    let mut problems: Vec<String> = Vec::new();

    println!("{}", style("Checking repository integrity…").bold());

    // ── 1. Objects: referenced ones exist and re-hash to their own name ───────
    let mut referenced: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT hash FROM file_map")?;
        let set: HashSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter(|h| !h.is_empty())
            .collect();
        set
    };
    // Conflict blobs are objects too.
    {
        let mut stmt =
            conn.prepare("SELECT ancestor_hash, our_hash, their_hash FROM conflict_files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows.flatten() {
            for h in [row.0, row.1, row.2] {
                if !h.is_empty() {
                    referenced.insert(h);
                }
            }
        }
    }

    let mut verified = 0usize;
    for h in &referenced {
        let path = objects_dir.join(h);
        if !path.exists() {
            problems.push(format!("object {} is referenced but missing from the store", h));
            continue;
        }
        match storage::read_object(&objects_dir, h) {
            Ok(bytes) => {
                let actual = blake3::hash(&bytes).to_hex().to_string();
                if &actual == h {
                    verified += 1;
                } else {
                    problems.push(format!(
                        "object {} is corrupt — its content hashes to {}",
                        h,
                        &actual[..16.min(actual.len())]
                    ));
                }
            }
            Err(_) => problems.push(format!("object {} could not be decompressed (corrupt)", h)),
        }
    }
    report_line(problems.len(), &format!("Objects: {} referenced, {} verified", referenced.len(), verified));

    // ── 2. Snapshots: parents resolve; content-addressed ids recompute ────────
    let all_snaps: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT hash FROM snapshots")?;
        let set: HashSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        set
    };

    let before_snap = problems.len();
    let mut ids_verified = 0usize;
    let mut ids_legacy = 0usize;
    {
        let mut stmt = conn.prepare(
            "SELECT hash, message, branch, parent_hash, merge_parent, created_at FROM snapshots",
        )?;
        let snaps: Vec<(String, String, String, String, String, String)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5).unwrap_or_default(),
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for (hash, message, branch, parent, merge_parent, created_at) in &snaps {
            if !parent.is_empty() && !all_snaps.contains(parent) {
                problems.push(format!("snapshot {} has parent {} which does not exist", hash, parent));
            }
            if !merge_parent.is_empty() && !all_snaps.contains(merge_parent) {
                problems.push(format!(
                    "snapshot {} has merge parent {} which does not exist",
                    hash, merge_parent
                ));
            }

            // Verify content-addressed ids (new scheme = SNAP_HASH_LEN chars).
            if hash.len() == crate::commands::SNAP_HASH_LEN {
                let tree = load_tree(&conn, hash)?;
                let recomputed =
                    crate::commands::snapshot_id(&tree, parent, merge_parent, message, created_at);
                if &recomputed == hash {
                    ids_verified += 1;
                } else {
                    problems.push(format!(
                        "snapshot {} (\"{}\", branch {}) does not match its content (recomputed {})",
                        hash, message, branch, recomputed
                    ));
                }
            } else {
                ids_legacy += 1;
            }
        }
    }
    let legacy_note = if ids_legacy > 0 {
        format!(", {} legacy (unverifiable)", ids_legacy)
    } else {
        String::new()
    };
    report_line(
        problems.len() - before_snap,
        &format!(
            "Snapshots: {} checked, {} ids verified{}",
            all_snaps.len(),
            ids_verified,
            legacy_note
        ),
    );

    // ── 3. Refs resolve: PARENT, tags, stash ──────────────────────────────────
    let before_refs = problems.len();
    let parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let parent = parent.trim();
    if !parent.is_empty() && !all_snaps.contains(parent) {
        problems.push(format!("PARENT points to {} which does not exist", parent));
    }
    check_ref_table(&conn, "tags", "name", &all_snaps, &mut problems)?;
    check_ref_table(&conn, "stash", "name", &all_snaps, &mut problems)?;
    report_line(problems.len() - before_refs, "Refs: PARENT, tags, stash");

    // ── Summary ────────────────────────────────────────────────────────────────
    println!();
    if problems.is_empty() {
        println!("{} No problems found — repository is healthy.", style("✔").green().bold());
        Ok(())
    } else {
        for p in &problems {
            println!("  {} {}", style("✖").red().bold(), p);
        }
        Err(VeloError::CorruptRepo(format!(
            "{} integrity problem(s) found",
            problems.len()
        )))
    }
}

fn report_line(problem_count: usize, label: &str) {
    if problem_count == 0 {
        println!("  {} {}", style("✔").green(), label);
    } else {
        println!("  {} {} ({} problem(s))", style("✖").red().bold(), label, problem_count);
    }
}

/// Load a snapshot's tree (path, object-hash pairs) for id verification.
fn load_tree(conn: &rusqlite::Connection, snap: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
    let tree = stmt
        .query_map([snap], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tree)
}

/// Verify every `snapshot_hash` in `table` points at a live snapshot.
fn check_ref_table(
    conn: &rusqlite::Connection,
    table: &str,
    id_col: &str,
    snaps: &HashSet<String>,
    problems: &mut Vec<String>,
) -> Result<()> {
    let sql = format!("SELECT {}, snapshot_hash FROM {}", id_col, table);
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    for (name, hash) in rows {
        if !hash.is_empty() && !snaps.contains(&hash) {
            problems.push(format!("{} '{}' points to snapshot {} which does not exist", table, name, hash));
        }
    }
    Ok(())
}
