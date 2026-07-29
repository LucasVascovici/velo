use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::commands::get_dirty_files;
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
    let merge_head_path = root.join(".velo/MERGE_HEAD");
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let conflict_count: i64 = conn
        .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
        .unwrap_or(0);

    if !merge_head_path.exists() && conflict_count == 0 {
        return Err(VeloError::InvalidInput("No merge in progress.".into()));
    }

    // MERGE_HEAD stores "pre_merge_hash:source_branch"
    let merge_info = fs::read_to_string(&merge_head_path).unwrap_or_default();
    let merge_info = merge_info.trim();
    let (pre_merge_hash, source_branch) = merge_info
        .split_once(':')
        .unwrap_or((merge_info, "(unknown)"));

    // Clear all conflict state from the database
    conn.execute("DELETE FROM hunk_decisions", [])?;
    conn.execute("DELETE FROM conflict_files", [])?;
    let _ = fs::remove_file(&merge_head_path);

    // Restore the working tree to its pre-merge state
    if !pre_merge_hash.is_empty() {
        println!(
            "{} Aborting merge of '{}' — restoring to {}…",
            style("!").yellow().bold(),
            style(source_branch).cyan(),
            style(pre_merge_hash).yellow()
        );
        crate::commands::restore::run(root, pre_merge_hash, true, &[])?;
    } else {
        println!(
            "{} Merge aborted (no pre-merge snapshot recorded).",
            style("✔").green()
        );
    }

    println!("{} Merge aborted cleanly.", style("✔").green().bold());
    Ok(())
}

// ─── File-map loader ─────────────────────────────────────────────────────────

fn load_file_map(
    conn: &rusqlite::Connection,
    snapshot_hash: &str,
) -> Result<HashMap<String, (String, i64)>> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let collected: HashMap<String, (String, i64)> = stmt
        .query_map([snapshot_hash], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, i64>(2)?),
            ))
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
    let head_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let head_branch = head_raw.trim();
    // Read pre-merge parent for abort restoration
    let pre_merge_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    if head_branch == target_branch {
        return Err(VeloError::InvalidInput(format!(
            "Cannot merge branch '{}' into itself.",
            target_branch
        )));
    }

    // ── Resolve tip hashes ────────────────────────────────────────────────────
    // Resolve the merge source first, so an unborn current branch can adopt it.
    let target_probe = crate::commands::branch_tip(&conn, target_branch)
        .or_else(|| crate::commands::resolve_snapshot_id(root, target_branch).ok());

    let current_hash: String = match crate::commands::branch_tip(&conn, head_branch) {
        Some(h) => h,
        None => {
            // The current branch has no commits yet. Merging into an unborn
            // branch simply starts it at the target (Git does the same) — there
            // is nothing of our own to reconcile.
            let target_hash = target_probe.ok_or_else(|| {
                VeloError::InvalidInput(format!(
                    "'{}' not found — expected a branch, tag, snapshot, or remote ref.",
                    target_branch
                ))
            })?;
            crate::commands::set_branch_tip(&conn, head_branch, &target_hash)?;
            drop(conn);
            // The working tree often already matches (we just branched from it);
            // only touch it when it genuinely differs.
            if pre_merge_parent.trim() != target_hash {
                crate::commands::restore::run(root, &target_hash, true, &[])?;
            }
            println!(
                "{} Fast-forwarded '{}' to {} — the branch had no commits of its own.",
                style("✔").green().bold(),
                head_branch,
                style(&target_hash).yellow()
            );
            return Ok(());
        }
    };

    // The merge source, resolved above: an exact local-branch tip is preferred
    // (so a short branch name can't be mis-read as a hash prefix), falling back
    // to tags / hash prefixes / remote-tracking refs like `origin/main`.
    let target_hash: String = target_probe.ok_or_else(|| {
        VeloError::InvalidInput(format!(
            "'{}' not found — expected a branch, tag, snapshot, or remote ref.",
            target_branch
        ))
    })?;

    // Nothing to do when both branches already point at the same snapshot
    // (common right after branching, before either side has diverged).
    if current_hash == target_hash {
        println!(
            "{} Already up to date — '{}' and '{}' point at the same snapshot.",
            style("✔").green(),
            head_branch,
            target_branch
        );
        return Ok(());
    }

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
            root,
            &mut conn,
            head_branch,
            &current_hash,
            &target_hash,
            target_branch,
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
    let current_files = load_file_map(&conn, &current_hash)?;
    let target_files = load_file_map(&conn, &target_hash)?;
    let ancestor_files = match &ancestor_hash {
        Some(h) => load_file_map(&conn, h)?,
        None => HashMap::new(),
    };

    println!(
        "Merging '{}' into '{}' (ancestor: {})…",
        style(target_branch).yellow().bold(),
        style(head_branch).cyan().bold(),
        style(ancestor_hash.as_deref().unwrap_or("none")).dim()
    );

    let objects_dir = root.join(".velo/objects");
    let mut conflicts: Vec<(String, String, String, String)> = Vec::new(); // (path, anc_hash, our_hash, thr_hash)
    let mut new_count = 0usize;
    let mut del_count = 0usize;
    let mut took_count = 0usize; // files taken from target (non-conflicting)

    // Union of all paths seen in either branch tip
    let all_paths: HashSet<&str> = current_files
        .keys()
        .chain(target_files.keys())
        .map(|s| s.as_str())
        .collect();

    for path in &all_paths {
        let cur = current_files
            .get(*path)
            .map(|(h, m)| (h.as_str(), *m))
            .unwrap_or(("", 0));
        let tgt = target_files
            .get(*path)
            .map(|(h, m)| (h.as_str(), *m))
            .unwrap_or(("", 0));
        let anc = ancestor_files
            .get(*path)
            .map(|(h, m)| (h.as_str(), *m))
            .unwrap_or(("", 0));
        let full = root.join(crate::db::db_to_path(path));

        match crate::commands::reconcile_file(&objects_dir, anc, cur, tgt)? {
            crate::commands::Reconcile::Nothing => {}
            crate::commands::Reconcile::Delete => {
                if full.exists() {
                    fs::remove_file(&full)?;
                }
                println!("  {} Deleted: {}", style("-").red(), path);
                del_count += 1;
            }
            crate::commands::Reconcile::TakeTheirs { hash, mode, is_new } => {
                if let Some(p) = full.parent() {
                    fs::create_dir_all(p)?;
                }
                storage::apply_file(&full, mode, &storage::read_object(&objects_dir, &hash)?)?;
                if is_new {
                    println!("  {} New file: {}", style("+").green(), path);
                    new_count += 1;
                } else {
                    println!("  {} Updated:  {}", style("~").cyan(), path);
                    took_count += 1;
                }
            }
            crate::commands::Reconcile::AutoMerged { content, mode } => {
                if let Some(p) = full.parent() {
                    fs::create_dir_all(p)?;
                }
                storage::apply_file(&full, mode, &content)?;
                println!("  {} Auto-merged: {}", style("~").cyan(), path);
                took_count += 1;
            }
            crate::commands::Reconcile::KeepOurs => {
                println!(
                    "  {} Delete/modify conflict: '{}' (keeping ours)",
                    style("!").yellow().bold(),
                    path
                );
            }
            crate::commands::Reconcile::Conflict => {
                conflicts.push((
                    path.to_string(),
                    anc.0.to_string(),
                    cur.0.to_string(),
                    tgt.0.to_string(),
                ));
                println!("  {} Conflict: {}", style("!").yellow().bold(), path);
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
        // Record merge state: write pre-merge PARENT hash so --abort can restore it
        storage::write_atomic(
            &root.join(".velo/MERGE_HEAD"),
            format!("{}:{}", pre_merge_parent.trim(), target_branch).as_bytes(),
        )?;
        let conn2 = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        for (path, anc_h, our_h, thr_h) in &conflicts {
            conn2.execute(
                "INSERT OR REPLACE INTO conflict_files
                 (path, ancestor_hash, our_hash, their_hash)
                 VALUES (?, ?, ?, ?)",
                params![path, anc_h, our_h, thr_h],
            )?;
        }

        println!("\n{}", style("Action required:").red().bold());
        for (f, _, _, _) in &conflicts {
            println!("  [{}]", style(f).yellow());
            println!(
                "    Resolve interactively: {}",
                style(format!("velo resolve {}", f)).cyan()
            );
            println!(
                "    Quick-take:            {}  or  {}",
                style(format!("velo resolve {} --take theirs", f)).green(),
                style(format!("velo resolve {} --take ours", f)).dim()
            );
        }
        println!(
            "\nResolve all at once:  {}",
            style("velo resolve --all --take theirs").cyan()
        );
        println!(
            "Once resolved:        {}",
            style("velo save \"Merge <branch>\"").yellow().bold()
        );
    } else if new_count + took_count + del_count == 0 {
        // Nothing to apply — the branches already agree on every file.
        println!(
            "\n{} Already up to date — nothing to merge.",
            style("✔").green()
        );
    } else {
        // Clean merge: still record MERGE_HEAD so the finalising `velo save`
        // stamps this snapshot with a second parent (`merge_parent`). Without
        // this a conflict-free merge would collapse to a single-parent commit
        // and `velo history --graph` would draw it as linear.
        storage::write_atomic(
            &root.join(".velo/MERGE_HEAD"),
            format!("{}:{}", pre_merge_parent.trim(), target_branch).as_bytes(),
        )?;
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

    let msg = format!("Fast-forward merge from '{}'", target_branch);
    // The FF snapshot's tree is exactly the target's tree (with modes).
    let tree: Vec<(String, String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
        let v = stmt
            .query_map([target_hash], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        v
    };
    let timestamp = crate::commands::snapshot_timestamp();
    let new_hash = crate::commands::snapshot_id(&tree, current_hash, "", &msg, &timestamp);
    let new_hash = new_hash.as_str();

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, created_at) VALUES (?, ?, ?, ?, ?)",
        params![new_hash, &msg, head_branch, current_hash, timestamp],
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;
        for (p, h, m) in &tree {
            ins.execute(params![new_hash, p, h, m])?;
        }
    }
    tx.commit()?;

    // restore::run writes PARENT itself
    crate::commands::restore::run(root, new_hash, true, &[])?;
    println!("{} Fast-forward complete.", style("✔").green());
    Ok(())
}
