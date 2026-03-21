use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::commands::{get_dirty_files, SNAP_HASH_LEN};
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(root: &Path, target_branch: Option<&str>, abort: bool) -> Result<()> {
    if abort {
        return do_abort(root);
    }
    let target = target_branch.ok_or_else(|| {
        VeloError::InvalidInput("Specify a branch to merge: velo merge <branch>".into())
    })?;
    do_merge(root, target)
}

// ─── Abort ───────────────────────────────────────────────────────────────────

fn do_abort(root: &Path) -> Result<()> {
    let merge_head = root.join(".velo/MERGE_HEAD");
    if !merge_head.exists() {
        return Err(VeloError::InvalidInput("No merge in progress.".into()));
    }
    let mut removed = 0usize;
    if let Ok(entries) = collect_conflict_files(root) {
        for path in entries {
            let _ = fs::remove_file(&path);
            removed += 1;
        }
    }
    fs::remove_file(&merge_head)?;
    println!(
        "{} Merge aborted. Removed {} conflict file(s).",
        style("✔").green(),
        removed
    );
    Ok(())
}

fn collect_conflict_files(root: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    collect_cf_recursive(root, &mut out)?;
    Ok(out)
}

fn collect_cf_recursive(
    dir: &Path,
    out: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let n = name.to_str().unwrap_or("");
        if n == ".velo" || n == ".git" {
            continue;
        }
        if path.is_dir() {
            collect_cf_recursive(&path, out)?;
        } else if path.extension().map(|e| e == "conflict").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

// ─── File-map loader ─────────────────────────────────────────────────────────

fn load_file_map(
    conn: &rusqlite::Connection,
    snapshot_hash: &str,
) -> Result<HashMap<String, String>> {
    let mut stmt =
        conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
    let collected: HashMap<String, String> = stmt
        .query_map([snapshot_hash], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(collected)
}

// ─── Merge ───────────────────────────────────────────────────────────────────

fn do_merge(root: &Path, target_branch: &str) -> Result<()> {
    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Merge aborted: {} unsaved change(s). Save or discard first.",
            dirty.len()
        )));
    }

    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::InvalidInput(
            "A merge is already in progress. Resolve conflicts and 'velo save', or run 'velo merge --abort'.".into(),
        ));
    }

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let head_raw =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let head_branch = head_raw.trim();

    if head_branch == target_branch {
        return Err(VeloError::InvalidInput(format!(
            "Cannot merge branch '{}' into itself.",
            target_branch
        )));
    }

    // ── Resolve tip hashes ────────────────────────────────────────────────────
    let current_hash: String = conn
        .query_row(
            "SELECT hash FROM snapshots WHERE branch = ? ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [head_branch],
            |r| r.get(0),
        )
        .map_err(|_| {
            VeloError::InvalidInput(format!(
                "Current branch '{}' has no snapshots. Save something first.",
                head_branch
            ))
        })?;

    let target_hash: String = conn
        .query_row(
            "SELECT hash FROM snapshots WHERE branch = ? ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [target_branch],
            |r| r.get(0),
        )
        .map_err(|_| {
            VeloError::InvalidInput(format!(
                "Branch '{}' not found or has no snapshots.",
                target_branch
            ))
        })?;

    // ── Fast-forward check ────────────────────────────────────────────────────
    let is_ff: bool = conn
        .query_row(
            "WITH RECURSIVE anc(hash, parent_hash) AS (
                SELECT hash, parent_hash FROM snapshots WHERE hash = ?1
                UNION ALL
                SELECT s.hash, s.parent_hash FROM snapshots s JOIN anc a ON s.hash = a.parent_hash
             )
             SELECT EXISTS(SELECT 1 FROM anc WHERE hash = ?2)",
            params![target_hash, current_hash],
            |r| r.get(0),
        )
        .unwrap_or(false);

    if is_ff {
        return do_fast_forward(
            root, &mut conn, head_branch, &current_hash, &target_hash, target_branch,
        );
    }

    // ── Find lowest common ancestor (LCA) ─────────────────────────────────────
    //
    // Walk the ancestry of the current tip and collect every ancestor hash
    // (with its depth).  Then walk the ancestry of the target tip and return
    // the first hash that is also an ancestor of current — that is the LCA.
    //
    // The recursive CTE is depth-limited to prevent infinite loops on
    // (theoretically impossible) cycles.
    let ancestor_hash: Option<String> = conn
        .query_row(
            "WITH RECURSIVE
             anc_cur(hash, parent_hash, depth) AS (
                 SELECT hash, parent_hash, 0
                 FROM snapshots WHERE hash = ?1
                 UNION ALL
                 SELECT s.hash, s.parent_hash, a.depth + 1
                 FROM snapshots s
                 JOIN anc_cur a ON s.hash = a.parent_hash
                 WHERE a.depth < 10000
             ),
             anc_tgt(hash, parent_hash) AS (
                 SELECT hash, parent_hash
                 FROM snapshots WHERE hash = ?2
                 UNION ALL
                 SELECT s.hash, s.parent_hash
                 FROM snapshots s
                 JOIN anc_tgt a ON s.hash = a.parent_hash
             )
             SELECT ac.hash
             FROM anc_cur ac
             JOIN anc_tgt at ON ac.hash = at.hash
             ORDER BY ac.depth ASC
             LIMIT 1",
            params![current_hash, target_hash],
            |r| r.get::<_, String>(0),
        )
        .ok();

    // ── Load all three file maps ───────────────────────────────────────────────
    let current_files  = load_file_map(&conn, &current_hash)?;
    let target_files   = load_file_map(&conn, &target_hash)?;
    let ancestor_files = match &ancestor_hash {
        Some(h) => load_file_map(&conn, h)?,
        None    => HashMap::new(),
    };

    println!(
        "Merging '{}' into '{}' (ancestor: {})…",
        style(target_branch).yellow().bold(),
        style(head_branch).cyan().bold(),
        style(ancestor_hash.as_deref().unwrap_or("none")).dim()
    );

    let objects_dir = root.join(".velo/objects");
    let mut conflicts: Vec<String> = Vec::new();
    let mut new_count  = 0usize;
    let mut del_count  = 0usize;
    let mut took_count = 0usize; // files taken from target (non-conflicting)

    // Union of all paths seen in either branch tip
    let all_paths: HashSet<&str> = current_files
        .keys()
        .chain(target_files.keys())
        .map(|s| s.as_str())
        .collect();

    for path in &all_paths {
        let cur_hash = current_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let tgt_hash = target_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let anc_hash = ancestor_files.get(*path).map(|s| s.as_str()).unwrap_or("");

        // Same content on both tips — nothing to do regardless of ancestor
        if cur_hash == tgt_hash {
            continue;
        }

        let cur_changed = cur_hash != anc_hash; // current tip changed vs ancestor
        let tgt_changed = tgt_hash != anc_hash; // target  tip changed vs ancestor

        match (cur_changed, tgt_changed) {
            // ── Only target changed since ancestor ────────────────────────────
            (false, true) => {
                let full = root.join(crate::db::db_to_path(path));
                if tgt_hash.is_empty() {
                    // Target deleted this file; current hasn't touched it → delete
                    if full.exists() {
                        fs::remove_file(&full)?;
                    }
                    println!("  {} Deleted: {}", style("-").red(), path);
                    del_count += 1;
                } else {
                    // Target modified or added this file
                    if let Some(p) = full.parent() {
                        fs::create_dir_all(p)?;
                    }
                    let data = storage::read_object(&objects_dir, tgt_hash)?;
                    fs::write(&full, data)?;
                    if anc_hash.is_empty() {
                        println!("  {} New file: {}", style("+").green(), path);
                        new_count += 1;
                    } else {
                        println!("  {} Updated:  {}", style("~").cyan(), path);
                        took_count += 1;
                    }
                }
            }

            // ── Only current changed since ancestor ───────────────────────────
            // The file is already correct on disk; nothing to write.
            (true, false) => {}

            // ── Both sides changed since ancestor → true conflict ─────────────
            (true, true) => {
                if tgt_hash.is_empty() {
                    // Target deleted, current modified → keep current, warn
                    println!(
                        "  {} Delete/modify conflict: '{}' (keeping ours)",
                        style("!").yellow().bold(),
                        path
                    );
                    // Not a blocker — no conflict file written; current wins
                } else if cur_hash.is_empty() {
                    // Current deleted, target modified → take target's version
                    // (current deleted intentionally; target's new work should not be lost)
                    let full = root.join(crate::db::db_to_path(path));
                    if let Some(p) = full.parent() {
                        fs::create_dir_all(p)?;
                    }
                    let data = storage::read_object(&objects_dir, tgt_hash)?;
                    fs::write(&full, data)?;
                    println!(
                        "  {} Restored (deleted on ours, modified on theirs): {}",
                        style("~").cyan(),
                        path
                    );
                    took_count += 1;
                } else {
                    // Both modified to different content → textbook conflict
                    let conflict_path =
                        root.join(crate::db::db_to_path(&format!("{}.conflict", path)));
                    let data = storage::read_object(&objects_dir, tgt_hash)?;
                    fs::write(&conflict_path, data)?;
                    println!("  {} Conflict: {}", style("!").yellow().bold(), path);
                    conflicts.push(path.to_string());
                }
            }

            // (false, false) with cur_hash != tgt_hash is impossible when ancestor
            // finding is correct (it would mean they diverged before the ancestor),
            // but treat defensively as a conflict.
            (false, false) => {
                let conflict_path =
                    root.join(crate::db::db_to_path(&format!("{}.conflict", path)));
                let data = storage::read_object(&objects_dir, tgt_hash)?;
                fs::write(&conflict_path, data)?;
                println!("  {} Conflict (pre-ancestor): {}", style("!").yellow().bold(), path);
                conflicts.push(path.to_string());
            }
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    println!("\n{}", style("Merge summary").bold().underlined());
    println!("  New:      {}", new_count);
    println!("  Updated:  {}", took_count);
    println!("  Deleted:  {}", del_count);
    println!("  Conflicts: {}", conflicts.len());

    if !conflicts.is_empty() {
        fs::write(root.join(".velo/MERGE_HEAD"), &target_hash)?;
        println!("\n{}", style("Action required:").red().bold());
        for f in &conflicts {
            println!("  [{}]", style(f).yellow());
            println!(
                "    View:    {}",
                style(format!("velo diff {} --conflict", f)).cyan()
            );
            println!(
                "    Resolve: {}  or  {}",
                style(format!("velo resolve {} --take theirs", f)).green(),
                style(format!("velo resolve {} --take ours",   f)).dim()
            );
        }
        println!(
            "\nOnce all conflicts are resolved: {}",
            style("velo save \"Merge <branch>\"").yellow().bold()
        );
    } else {
        println!(
            "\n{} Clean merge! Run {} to finalise.",
            style("✔").green(),
            style("velo save \"Merge <branch>\"").yellow().bold()
        );
    }

    Ok(())
}

fn do_fast_forward(
    root: &Path,
    conn: &mut rusqlite::Connection,
    head_branch: &str,
    current_hash: &str,
    target_hash: &str,
    target_branch: &str,
) -> Result<()> {
    println!(
        "{} Fast-forwarding '{}' to {}…",
        style(">>").green().bold(),
        head_branch,
        style(target_hash).yellow()
    );

    let now = chrono::Utc::now().to_rfc3339();
    let msg = format!("Fast-forward merge from '{}'", target_branch);
    let ff_full_hex = blake3::hash(
        format!("{}{}{}{}", msg, head_branch, current_hash, now).as_bytes(),
    )
    .to_hex()
    .to_string();
    let new_hash = &ff_full_hex[..SNAP_HASH_LEN];

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![new_hash, &msg, head_branch, current_hash],
    )?;
    tx.execute(
        "INSERT INTO file_map (snapshot_hash, path, hash)
         SELECT ?, path, hash FROM file_map WHERE snapshot_hash = ?",
        params![new_hash, target_hash],
    )?;
    tx.commit()?;

    // restore::run writes PARENT itself
    crate::commands::restore::run(root, new_hash, true, &[])?;
    println!("{} Fast-forward complete.", style("✔").green());
    Ok(())
}