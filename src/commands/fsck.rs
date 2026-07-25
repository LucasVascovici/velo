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

pub fn run(root: &Path, repair: bool) -> Result<()> {
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

    // ── 4. Repairable state (cruft / broken in-progress state) ────────────────
    // These are warnings, not corruption: they don't fail the exit code, and
    // `--repair` cleans them up.
    let orphan_hunks: i64 = conn
        .query_row(
            "SELECT count(*) FROM hunk_decisions
             WHERE file_path NOT IN (SELECT path FROM conflict_files)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let orphan_tags: i64 = conn
        .query_row(
            "SELECT count(*) FROM trash_tags
             WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
               AND snapshot_hash NOT IN (SELECT hash FROM trash)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // Remote-tracking refs can outlive their snapshot (e.g. after `gc` prunes
    // history that was only reachable from a stale tracking ref). Not
    // corruption — the next `fetch` re-establishes them.
    let stale_remote_refs: i64 = conn
        .query_row(
            "SELECT count(*) FROM remote_refs
             WHERE hash NOT IN (SELECT hash FROM snapshots)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // A tracking ref for a remote that no longer exists is pure cruft.
    let orphan_remote_refs: i64 = conn
        .query_row(
            "SELECT count(*) FROM remote_refs
             WHERE remote NOT IN (SELECT name FROM remotes)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let conflicts_cnt: i64 = conn
        .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
        .unwrap_or(0);
    let broken_conflict = conflicts_cnt > 0 && !root.join(".velo/MERGE_HEAD").exists();

    let mut warnings: Vec<String> = Vec::new();
    if orphan_hunks > 0 {
        warnings.push(format!("{} orphaned hunk-decision row(s)", orphan_hunks));
    }
    if orphan_tags > 0 {
        warnings.push(format!("{} shelved tag(s) with no snapshot", orphan_tags));
    }
    if broken_conflict {
        warnings.push(format!(
            "{} conflict row(s) with no merge in progress (broken merge state)",
            conflicts_cnt
        ));
    }
    if stale_remote_refs > 0 {
        warnings.push(format!(
            "{} remote-tracking ref(s) pointing at absent snapshots (re-fetch to refresh)",
            stale_remote_refs
        ));
    }
    if orphan_remote_refs > 0 {
        warnings.push(format!(
            "{} remote-tracking ref(s) for removed remote(s)",
            orphan_remote_refs
        ));
    }

    let mut repaired: Vec<String> = Vec::new();
    if repair && !warnings.is_empty() {
        if orphan_hunks > 0 {
            conn.execute(
                "DELETE FROM hunk_decisions WHERE file_path NOT IN (SELECT path FROM conflict_files)",
                [],
            )?;
            repaired.push(format!("pruned {} orphaned hunk-decision row(s)", orphan_hunks));
        }
        if orphan_tags > 0 {
            conn.execute(
                "DELETE FROM trash_tags
                 WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
                   AND snapshot_hash NOT IN (SELECT hash FROM trash)",
                [],
            )?;
            repaired.push(format!("pruned {} orphaned shelved tag(s)", orphan_tags));
        }
        if broken_conflict {
            conn.execute("DELETE FROM conflict_files", [])?;
            conn.execute("DELETE FROM hunk_decisions", [])?;
            repaired.push("cleared broken conflict state".into());
        }
        if stale_remote_refs > 0 {
            conn.execute(
                "DELETE FROM remote_refs WHERE hash NOT IN (SELECT hash FROM snapshots)",
                [],
            )?;
            repaired.push(format!("pruned {} stale remote-tracking ref(s)", stale_remote_refs));
        }
        if orphan_remote_refs > 0 {
            conn.execute(
                "DELETE FROM remote_refs WHERE remote NOT IN (SELECT name FROM remotes)",
                [],
            )?;
            repaired.push(format!(
                "pruned {} remote-tracking ref(s) for removed remote(s)",
                orphan_remote_refs
            ));
        }
    }
    if repaired.is_empty() && warnings.is_empty() {
        report_line(0, "State: no cruft");
    } else if repair {
        report_line(0, "State: repaired");
    } else {
        report_line(warnings.len(), "State");
    }

    // ── Summary ────────────────────────────────────────────────────────────────
    println!();
    for w in &warnings {
        let mark = if repair { style("~").green() } else { style("!").yellow() };
        println!("  {} {}", mark, w);
    }
    for r in &repaired {
        println!("  {} {}", style("✔").green(), r);
    }
    if problems.is_empty() {
        if warnings.is_empty() || repair {
            println!(
                "\n{} Repository is healthy.",
                style("✔").green().bold()
            );
        } else {
            println!(
                "\n{} No corruption; {} cleanup item(s) — run {} to tidy.",
                style("✔").green(),
                warnings.len(),
                style("velo fsck --repair").cyan()
            );
        }
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

/// Load a snapshot's tree (path, object-hash, mode) for id verification.
fn load_tree(conn: &rusqlite::Connection, snap: &str) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let tree = stmt
        .query_map([snap], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })?
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
