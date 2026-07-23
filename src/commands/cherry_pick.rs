//! `velo cherry-pick <hash>` — apply the changes from one snapshot onto the
//! current working tree.
//!
//! Uses the shared 3-way reconciliation (`commands::reconcile_file`): the
//! snapshot's parent is the common ancestor, the current working tree is
//! "ours", and the snapshot's file map is "theirs". Non-overlapping edits are
//! auto-merged; genuinely overlapping edits are recorded as conflicts.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use console::style;

use crate::commands::{get_dirty_files, reconcile_file, Reconcile};
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(root: &Path, target: &str) -> Result<()> {
    // Safety: dirty working tree is ambiguous during a cherry-pick
    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "Cherry-pick aborted: {} unsaved change(s). Save or discard first.",
            dirty.len()
        )));
    }

    // Guard: already in a merge
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::InvalidInput(
            "A merge is already in progress. Resolve it before cherry-picking.".into(),
        ));
    }

    let snap_hash = crate::commands::resolve_snapshot_id(root, target)?;
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Load snapshot metadata
    let (message, parent_hash): (String, String) = conn
        .query_row(
            "SELECT message, parent_hash FROM snapshots WHERE hash = ?",
            [&snap_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| VeloError::InvalidInput(format!("Snapshot '{}' not found.", target)))?;

    println!(
        "Cherry-picking {} — \"{}\"",
        style(&snap_hash).yellow(),
        style(&message).dim()
    );

    let objects_dir = root.join(".velo/objects");
    let current_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // Three file maps: current (ours), snapshot (theirs), ancestor (its parent).
    let current_files = load_file_map(&conn, current_parent.trim())?;
    let their_files = load_file_map(&conn, &snap_hash)?;
    let ancestor_files = load_file_map(&conn, &parent_hash)?;

    let all_paths: std::collections::HashSet<&str> = current_files
        .keys()
        .chain(their_files.keys())
        .map(|s| s.as_str())
        .collect();

    let mut conflicts: Vec<(String, String, String, String)> = Vec::new(); // (path, anc, our, thr)
    let mut new_count = 0usize;
    let mut changed_count = 0usize;
    let mut del_count = 0usize;

    // Apply each file's reconciled outcome to the working tree.
    for path in &all_paths {
        let cur = current_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let tgt = their_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let anc = ancestor_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let full = root.join(db::db_to_path(path));

        match reconcile_file(&objects_dir, anc, cur, tgt)? {
            Reconcile::Nothing => {}
            Reconcile::Delete => {
                if full.exists() {
                    fs::remove_file(&full)?;
                }
                del_count += 1;
            }
            Reconcile::TakeTheirs { hash, is_new } => {
                if let Some(p) = full.parent() {
                    fs::create_dir_all(p)?;
                }
                fs::write(&full, storage::read_object(&objects_dir, &hash)?)?;
                if is_new {
                    new_count += 1;
                } else {
                    changed_count += 1;
                }
            }
            Reconcile::AutoMerged(bytes) => {
                if let Some(p) = full.parent() {
                    fs::create_dir_all(p)?;
                }
                fs::write(&full, bytes)?;
                changed_count += 1;
            }
            Reconcile::KeepOurs => {
                println!(
                    "  {} '{}' deleted in cherry-pick but modified locally — kept ours.",
                    style("!").yellow(),
                    path
                );
            }
            Reconcile::Conflict => {
                conflicts.push((
                    path.to_string(),
                    anc.to_string(),
                    cur.to_string(),
                    tgt.to_string(),
                ));
                println!("  {} Conflict: {}", style("!").yellow().bold(), path);
            }
        }
    }

    println!("\n{}", style("Cherry-pick summary").bold().underlined());
    println!("  New:      {}", new_count);
    println!("  Changed:  {}", changed_count);
    println!("  Deleted:  {}", del_count);
    println!("  Conflicts: {}", conflicts.len());

    if !conflicts.is_empty() {
        // Record conflict state; the user resolves then `velo save`s.
        // MERGE_HEAD format "pre_hash:cherry-pick/<snap>" enables --abort.
        let pre_cp_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        fs::write(
            root.join(".velo/MERGE_HEAD"),
            format!("{}:cherry-pick/{}", pre_cp_parent.trim(), &snap_hash[..8]),
        )?;
        for (path, anc_h, our_h, thr_h) in &conflicts {
            conn.execute(
                "INSERT OR REPLACE INTO conflict_files
                 (path, ancestor_hash, our_hash, their_hash)
                 VALUES (?, ?, ?, ?)",
                rusqlite::params![path, anc_h, our_h, thr_h],
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
                "    Quick-take: {}  or  {}",
                style(format!("velo resolve {} --take theirs", f)).green(),
                style(format!("velo resolve {} --take ours", f)).dim()
            );
        }
        println!(
            "\nOnce resolved: {}",
            style("velo save \"Apply cherry-pick\"").yellow().bold()
        );
        println!("  To cancel:    {}", style("velo merge --abort").dim());
    } else {
        // Clean pick — snapshot the applied working tree. Delegating to `save`
        // hashes any auto-merged content correctly and keeps one snapshot path.
        let cp_message = format!("Cherry-pick {}: {}", &snap_hash[..8], message);
        match crate::commands::save::run(root, &cp_message, false)? {
            Some(r) => println!(
                "\n{} Cherry-pick applied as snapshot {}",
                style("✔").green().bold(),
                style(&r.hash).yellow()
            ),
            None => println!(
                "\n{} Nothing to apply — already up to date.",
                style("✔").green()
            ),
        }
    }
    Ok(())
}

fn load_file_map(conn: &rusqlite::Connection, snap_hash: &str) -> Result<HashMap<String, String>> {
    if snap_hash.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
    let collected: HashMap<String, String> = stmt
        .query_map([snap_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(collected)
}
