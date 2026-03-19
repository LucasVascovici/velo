use std::collections::HashMap;
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
        VeloError::InvalidInput(
            "Specify a branch to merge: velo merge <branch>".into(),
        )
    })?;

    do_merge(root, target)
}

// ─── Abort ───────────────────────────────────────────────────────────────────

fn do_abort(root: &Path) -> Result<()> {
    let merge_head = root.join(".velo/MERGE_HEAD");
    if !merge_head.exists() {
        return Err(VeloError::InvalidInput(
            "No merge in progress.".into(),
        ));
    }

    // Remove all .conflict files from the working tree
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
    collect_cf_recursive(root, root, &mut out)?;
    Ok(out)
}

fn collect_cf_recursive(
    dir: &Path,
    root: &Path,
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
            collect_cf_recursive(&path, root, out)?;
        } else if path.extension().map(|e| e == "conflict").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

// ─── Merge ───────────────────────────────────────────────────────────────────

fn do_merge(root: &Path, target_branch: &str) -> Result<()> {
    // ── Safety: refuse dirty working tree ────────────────────────────────────
    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Merge aborted: {} unsaved change(s). Save or discard first.",
            dirty.len()
        )));
    }

    // ── Guard: already merging ────────────────────────────────────────────────
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

    // ── Resolve latest hashes for both branches ───────────────────────────────
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
        return do_fast_forward(root, &mut conn, head_branch, &current_hash, &target_hash, target_branch);
    }

    // ── Three-way merge ───────────────────────────────────────────────────────
    println!(
        "Merging '{}' into '{}'…",
        style(target_branch).yellow().bold(),
        style(head_branch).cyan().bold()
    );

    let mut stmt =
        conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;

    let current_files: HashMap<String, String> = stmt
        .query_map(params![&current_hash], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let target_files: HashMap<String, String> = stmt
        .query_map(params![&target_hash], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let objects_dir = root.join(".velo/objects");
    let mut conflicts: Vec<String> = Vec::new();
    let mut new_count = 0usize;

    // Files in target branch
    for (path, t_hash) in &target_files {
        let full = root.join(crate::db::db_to_path(path));

        if let Some(c_hash) = current_files.get(path) {
            if c_hash == t_hash {
                continue; // Identical — nothing to do
            }
            // CONFLICT: both branches modified this file differently
            let data = storage::read_object(&objects_dir, t_hash)?;
            let conflict_path = root.join(crate::db::db_to_path(&format!("{}.conflict", path)));
            fs::write(&conflict_path, data)?;
            println!("  {} Conflict: {}", style("!").yellow().bold(), path);
            conflicts.push(path.clone());
        } else {
            // NEW file in target branch
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)?;
            }
            let data = storage::read_object(&objects_dir, t_hash)?;
            fs::write(&full, data)?;
            println!("  {} New file: {}", style("+").green(), path);
            new_count += 1;
        }
    }

    // ── Deletion propagation ──────────────────────────────────────────────────
    // Files that exist in current but NOT in target were deleted in the target
    // branch and should be removed from the working tree.
    let mut del_count = 0usize;
    for (path, _) in &current_files {
        if !target_files.contains_key(path) {
            let full = root.join(crate::db::db_to_path(path));
            if full.exists() {
                fs::remove_file(&full)?;
                println!("  {} Deleted: {}", style("-").red(), path);
                del_count += 1;
            }
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    println!("\n{}", style("Merge summary").bold().underlined());
    println!("  New:      {}", new_count);
    println!("  Deleted:  {}", del_count);
    println!("  Conflicts: {}", conflicts.len());

    if !conflicts.is_empty() {
        // Record merge state
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
                style(format!("velo resolve {} --take ours", f)).dim()
            );
        }
        println!(
            "\nOnce all conflicts are resolved: {}",
            style("velo save \"Finish merge\"").yellow().bold()
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

    // restore::run writes PARENT itself — don't pre-write here
    crate::commands::restore::run(root, new_hash, true)?;

    println!("{} Fast-forward complete.", style("✔").green());
    Ok(())
}