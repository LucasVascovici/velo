//! `velo rebase <target>` — replay the current branch's commits on top of
//! another branch (or snapshot), producing a linear history.
//!
//! For each commit on the current branch that is NOT already in the target's
//! ancestry, we cherry-pick it onto the target in order.  If any cherry-pick
//! produces a conflict the rebase pauses and tells the user how to continue.
//!
//! REBASE_STATE file: "<target_hash>\n<c1>\n<c2>\n…" (remaining commits, newest first)
//! REBASE_ONTO file:  the hash we are rebasing onto

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::commands::get_dirty_files;
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn run(root: &Path, target: &str, abort: bool, cont: bool) -> Result<()> {
    if abort {
        return do_abort(root);
    }
    if cont {
        return do_continue(root);
    }
    do_start(root, target)
}

// ── Start ─────────────────────────────────────────────────────────────────────

fn do_start(root: &Path, target: &str) -> Result<()> {
    if root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::InvalidInput(
            "A rebase is already in progress. Use --continue or --abort.".into(),
        ));
    }

    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Rebase aborted: {} unsaved change(s). Save or discard first.",
            dirty.len()
        )));
    }

    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let branch = branch_raw.trim().to_string();
    let head_hash = fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default()
        .trim()
        .to_string();

    let onto_hash = crate::commands::resolve_snapshot_id(root, target)?;

    if onto_hash == head_hash {
        println!("{} Already up to date.", style("✔").green().bold());
        return Ok(());
    }

    // Build ancestry set of the onto commit
    let onto_ancestors = ancestry(&conn, &onto_hash);

    // Collect commits on this branch NOT in onto's ancestry (oldest first)
    let branch_commits = branch_linear_history(&conn, &head_hash, &onto_ancestors);

    if branch_commits.is_empty() {
        println!("{} Already up to date.", style("✔").green().bold());
        return Ok(());
    }

    println!(
        "\n{} Rebasing '{}' onto {}…\n  {} commits to replay",
        style("◆").cyan().bold(),
        style(&branch).cyan(),
        style(&onto_hash[..8]).yellow(),
        branch_commits.len()
    );

    // Save the onto hash so --abort and --continue can use it
    fs::write(root.join(".velo/REBASE_ONTO"), &onto_hash)?;
    // Also save the original HEAD so --abort can restore it
    fs::write(root.join(".velo/REBASE_ORIG_HEAD"), &head_hash)?;

    // Switch the branch tip to `onto` and restore the working tree
    crate::commands::restore::run(root, &onto_hash, true, &[])?;
    // Point HEAD at our branch (restore updates PARENT but not HEAD for different branches)
    // We need to update PARENT to onto_hash; restore already did that
    // but we need to keep tracking our branch name.
    // Actually restore::run restores files and sets PARENT, but it also sets
    // the PARENT to match onto. That's correct — we're building on top of onto.

    // Write state file: remaining commits, oldest first
    let state: String = branch_commits
        .iter()
        .map(|(h, _)| h.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join(".velo/REBASE_STATE"), &state)?;

    replay_next(root, &conn, &branch, &branch_commits)
}

// ── Continue after resolving a conflict ───────────────────────────────────────

fn do_continue(root: &Path) -> Result<()> {
    if !root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::InvalidInput("No rebase in progress.".into()));
    }
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::InvalidInput(
            "Conflicts still unresolved. Run `velo resolve --all` then `velo save` first.".into(),
        ));
    }
    // Read remaining state
    let state_raw = fs::read_to_string(root.join(".velo/REBASE_STATE"))?;
    let remaining: Vec<&str> = state_raw.lines().filter(|l| !l.is_empty()).collect();

    if remaining.is_empty() {
        return finish_rebase(root);
    }

    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let branch = branch_raw.trim().to_string();

    // Re-load the full list from the remaining hashes
    let commits: Vec<(String, String)> = remaining
        .iter()
        .map(|h| {
            let msg: String = conn
                .query_row("SELECT message FROM snapshots WHERE hash = ?", [h], |r| r.get(0))
                .unwrap_or_default();
            (h.to_string(), msg)
        })
        .collect();

    replay_next(root, &conn, &branch, &commits)
}

// ── Abort ─────────────────────────────────────────────────────────────────────

fn do_abort(root: &Path) -> Result<()> {
    if !root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::InvalidInput("No rebase in progress.".into()));
    }

    let orig_head = fs::read_to_string(root.join(".velo/REBASE_ORIG_HEAD"))
        .unwrap_or_default()
        .trim()
        .to_string();

    // Clean up conflict state if any
    if let Ok(conn) = db::get_conn_at_path(&root.join(".velo/velo.db")) {
        let _ = conn.execute("DELETE FROM hunk_decisions", []);
        let _ = conn.execute("DELETE FROM conflict_files",  []);
    }
    let _ = fs::remove_file(root.join(".velo/MERGE_HEAD"));
    let _ = fs::remove_file(root.join(".velo/REBASE_STATE"));
    let _ = fs::remove_file(root.join(".velo/REBASE_ONTO"));
    let _ = fs::remove_file(root.join(".velo/REBASE_ORIG_HEAD"));

    if !orig_head.is_empty() {
        println!(
            "{} Rebase aborted — restoring to {}…",
            style("!").yellow().bold(),
            style(&orig_head[..8]).yellow()
        );
        crate::commands::restore::run(root, &orig_head, true, &[])?;
    }

    println!("{} Rebase aborted cleanly.", style("✔").green().bold());
    Ok(())
}

// ── Core replay loop ──────────────────────────────────────────────────────────

fn replay_next(
    root:    &Path,
    conn:    &rusqlite::Connection,
    _branch: &str,
    commits: &[(String, String)],
) -> Result<()> {
    for (idx, (snap_hash, msg)) in commits.iter().enumerate() {
        println!(
            "  {} {}/{} {} {}",
            style("◦").dim(),
            idx + 1,
            commits.len(),
            style(&snap_hash[..8]).yellow(),
            style(msg).dim()
        );

        // Apply this commit's changes onto the current working tree
        match apply_one(root, conn, snap_hash) {
            Ok(ApplyResult::Clean) => {
                // Auto-save with the original message
                crate::commands::save::run(root, msg, false)?;
                // Advance the state file (remove the first entry)
                let state_raw = fs::read_to_string(root.join(".velo/REBASE_STATE"))
                    .unwrap_or_default();
                let rest: Vec<&str> = state_raw.lines()
                    .skip(1)
                    .filter(|l| !l.is_empty())
                    .collect();
                fs::write(root.join(".velo/REBASE_STATE"), rest.join("\n"))?;
            }
            Ok(ApplyResult::Conflict(n)) => {
                println!(
                    "\n{} Conflict in {} file(s) while replaying {}.",
                    style("!").red().bold(),
                    n,
                    style(&snap_hash[..8]).yellow()
                );
                println!(
                    "  Resolve with {} then {}",
                    style("velo resolve --all").cyan(),
                    style("velo save \"…\"").cyan()
                );
                println!(
                    "  Then continue with {}",
                    style("velo rebase --continue").cyan()
                );
                println!(
                    "  Or give up with {}",
                    style("velo rebase --abort").cyan()
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }

    finish_rebase(root)
}

fn finish_rebase(root: &Path) -> Result<()> {
    let _ = fs::remove_file(root.join(".velo/REBASE_STATE"));
    let _ = fs::remove_file(root.join(".velo/REBASE_ONTO"));
    let _ = fs::remove_file(root.join(".velo/REBASE_ORIG_HEAD"));

    let new_head = fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default()
        .trim()
        .to_string();

    println!(
        "\n{} Rebase complete. HEAD is now {}.",
        style("✔").green().bold(),
        style(&new_head[..8.min(new_head.len())]).yellow()
    );
    Ok(())
}

// ── Single-commit application (cherry-pick logic) ─────────────────────────────

enum ApplyResult {
    Clean,
    Conflict(usize),
}

fn apply_one(
    root:      &Path,
    conn:      &rusqlite::Connection,
    snap_hash: &str,
) -> Result<ApplyResult> {
    let objects_dir = root.join(".velo/objects");

    let parent_hash: String = conn
        .query_row(
            "SELECT parent_hash FROM snapshots WHERE hash = ?",
            [snap_hash],
            |r| r.get(0),
        )
        .map_err(|_| VeloError::InvalidInput(format!("Snapshot {} not found.", snap_hash)))?;

    let anc_files  = load_file_map(conn, &parent_hash)?;
    let snap_files = load_file_map(conn, snap_hash)?;
    let cur_files  = load_file_map_from_parent(root, conn)?;

    let all_paths: HashSet<&str> = anc_files.keys()
        .chain(snap_files.keys())
        .chain(cur_files.keys())
        .map(|s| s.as_str())
        .collect();

    let mut conflicts: Vec<(String, String, String, String)> = Vec::new();
    let mut new_count = 0usize;
    let mut mod_count = 0usize;
    let mut del_count = 0usize;

    for path in all_paths {
        let anc_h = anc_files.get(path).map(|s| s.as_str()).unwrap_or("");
        let snp_h = snap_files.get(path).map(|s| s.as_str()).unwrap_or("");
        let cur_h = cur_files.get(path).map(|s| s.as_str()).unwrap_or("");

        let snap_changed = snp_h != anc_h;
        let cur_changed  = cur_h  != anc_h;

        let full = root.join(db::db_to_path(path));

        match (snap_changed, cur_changed) {
            (false, _) => {} // snapshot didn't change this file — leave ours
            (true, false) => {
                // Snapshot changed, we didn't — apply cleanly
                if snp_h.is_empty() {
                    if full.exists() { let _ = fs::remove_file(&full); del_count += 1; }
                } else {
                    let data = storage::read_object(&objects_dir, snp_h)?;
                    if let Some(p) = full.parent() { let _ = fs::create_dir_all(p); }
                    fs::write(&full, data)?;
                    if anc_h.is_empty() { new_count += 1; } else { mod_count += 1; }
                }
            }
            (true, true) if snp_h == cur_h => {} // both changed to same — no conflict
            (true, true) => {
                // Both changed differently — conflict
                conflicts.push((
                    path.to_string(),
                    anc_h.to_string(),
                    cur_h.to_string(),
                    snp_h.to_string(),
                ));
            }
        }
    }

    let _ = (new_count, mod_count, del_count);

    if conflicts.is_empty() {
        return Ok(ApplyResult::Clean);
    }

    // Store conflicts in DB and write MERGE_HEAD for resolve/abort
    let pre_parent = fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default();
    fs::write(
        root.join(".velo/MERGE_HEAD"),
        format!("{}:rebase/{}", pre_parent.trim(), &snap_hash[..8]),
    )?;

    for (path, anc_h, our_h, thr_h) in &conflicts {
        conn.execute(
            "INSERT OR REPLACE INTO conflict_files
             (path, ancestor_hash, our_hash, their_hash)
             VALUES (?, ?, ?, ?)",
            params![path, anc_h, our_h, thr_h],
        )?;
    }

    Ok(ApplyResult::Conflict(conflicts.len()))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_file_map(
    conn:  &rusqlite::Connection,
    hash:  &str,
) -> Result<HashMap<String, String>> {
    if hash.is_empty() { return Ok(HashMap::new()); }
    let mut stmt = conn.prepare(
        "SELECT path, hash FROM file_map WHERE snapshot_hash = ?"
    )?;
    let result: HashMap<String, String> = stmt
        .query_map([hash], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(result)
}

fn load_file_map_from_parent(
    root: &Path,
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, String>> {
    let parent = fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default();
    load_file_map(conn, parent.trim())
}

/// Walk history from `start` stopping when a hash in `stop_set` is reached.
/// Returns commits in replay order (oldest first).
fn branch_linear_history(
    conn:     &rusqlite::Connection,
    start:    &str,
    stop_set: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut cur = start.to_string();
    loop {
        if stop_set.contains(&cur) || cur.is_empty() {
            break;
        }
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT message, parent_hash FROM snapshots WHERE hash = ?",
                [&cur],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match row {
            Some((msg, parent)) => {
                result.push((cur, msg));
                cur = parent;
            }
            None => break,
        }
    }
    result.reverse(); // oldest first
    result
}

/// Return all ancestors of `hash` as a HashSet.
fn ancestry(conn: &rusqlite::Connection, hash: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut stack = vec![hash.to_string()];
    while let Some(h) = stack.pop() {
        if set.contains(&h) || h.is_empty() { continue; }
        set.insert(h.clone());
        if let Ok(p) = conn.query_row(
            "SELECT parent_hash FROM snapshots WHERE hash = ?",
            [&h], |r| r.get::<_, String>(0),
        ) {
            stack.push(p);
        }
    }
    set
}