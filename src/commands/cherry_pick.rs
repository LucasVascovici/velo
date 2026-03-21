//! `velo cherry-pick <hash>` — apply the changes from one snapshot onto the
//! current working tree.
//!
//! Uses the same 3-way merge logic as `velo merge`: the snapshot's parent is
//! the common ancestor, the current working tree is "ours", and the snapshot's
//! file map is "theirs".  Conflicts are written as `.conflict` files.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use console::style;
use rayon::prelude::*;
use rusqlite::params;

use crate::commands::{get_dirty_files, SNAP_HASH_LEN};
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

    // Load three file maps: current, snapshot (theirs), ancestor (snapshot's parent)
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

    // Collect writes to do in parallel, conflicts to handle sequentially
    let mut writes: Vec<(String, String)> = Vec::new(); // (rel_path, object_hash)
    let mut deletes: Vec<String> = Vec::new();

    for path in &all_paths {
        let cur = current_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let tgt = their_files.get(*path).map(|s| s.as_str()).unwrap_or("");
        let anc = ancestor_files.get(*path).map(|s| s.as_str()).unwrap_or("");

        // No change between snapshot and its parent — skip
        if tgt == anc {
            continue;
        }

        let cur_changed = cur != anc;
        let tgt_changed = tgt != anc;

        match (cur_changed, tgt_changed) {
            (_, true) if !cur_changed => {
                // Only theirs changed — apply it
                if tgt.is_empty() {
                    deletes.push(path.to_string());
                } else {
                    writes.push((path.to_string(), tgt.to_string()));
                }
            }
            (false, _) => {} // Already identical — nothing to do
            (true, true) => {
                if cur == tgt {
                    // Both changed to the same thing — no-op
                } else if tgt.is_empty() {
                    // Theirs deleted, ours modified — keep ours, warn
                    println!(
                        "  {} '{}' deleted in cherry-pick but modified locally — kept ours.",
                        style("!").yellow(),
                        path
                    );
                } else {
                    // Real content conflict — store in DB (no sidecar file)
                    // ancestor = snapshot's parent, cur = current file, tgt = cherry-picked
                    let anc_obj = ancestor_files.get(*path).map(|s| s.as_str()).unwrap_or("");
                    conflicts.push((
                        path.to_string(),
                        anc_obj.to_string(),
                        cur.to_string(),
                        tgt.to_string(),
                    ));
                    println!("  {} Conflict: {}", style("!").yellow().bold(), path);
                }
            }
            _ => {}
        }
    }

    // Apply deletes
    for rel in &deletes {
        let full = root.join(db::db_to_path(rel));
        if full.exists() {
            fs::remove_file(&full)?;
        }
        del_count += 1;
    }

    // Apply writes in parallel
    let write_errors: Vec<String> = writes
        .par_iter()
        .filter_map(|(rel, obj_hash)| {
            let full = root.join(db::db_to_path(rel));
            if let Some(p) = full.parent() {
                fs::create_dir_all(p).ok()?;
            }
            match storage::read_object(&objects_dir, obj_hash) {
                Ok(data) => {
                    let is_new = !full.exists();
                    match fs::write(&full, data) {
                        Ok(_) => {
                            if is_new {
                                // We'll count new vs changed later
                            }
                            None
                        }
                        Err(e) => Some(format!("{}: {}", rel, e)),
                    }
                }
                Err(e) => Some(format!("{}: {}", rel, e)),
            }
        })
        .collect();

    if !write_errors.is_empty() {
        for e in &write_errors {
            eprintln!("{} {}", style("error:").red().bold(), e);
        }
    }

    // Count new vs changed
    for (rel, _) in &writes {
        if ancestor_files.contains_key(rel.as_str()) || current_files.contains_key(rel.as_str()) {
            changed_count += 1;
        } else {
            new_count += 1;
        }
    }

    println!("\n{}", style("Cherry-pick summary").bold().underlined());
    println!("  New:      {}", new_count);
    println!("  Changed:  {}", changed_count);
    println!("  Deleted:  {}", del_count);
    println!("  Conflicts: {}", conflicts.len());

    if !conflicts.is_empty() {
        // Write MERGE_HEAD + store conflicts in DB
        // Format: "pre_merge_hash:cherry-pick/<snap_hash>" for abort support
        let pre_cp_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        fs::write(
            root.join(".velo/MERGE_HEAD"),
            format!("{}:cherry-pick/{}", pre_cp_parent.trim(), &snap_hash[..8]),
        )?;
        let conn2 = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        for (path, anc_h, our_h, thr_h) in &conflicts {
            conn2.execute(
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
    } else {
        // Auto-save the cherry-pick as a new snapshot
        let now = chrono::Utc::now().to_rfc3339();
        let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
        let cp_message = format!("Cherry-pick {}: {}", &snap_hash[..8], message);
        let full_hex = blake3::hash(
            format!(
                "{}{}{}{}",
                cp_message,
                branch.trim(),
                current_parent.trim(),
                now
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let new_snap = &full_hex[..SNAP_HASH_LEN];

        let mut conn2 = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        let tx = conn2.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (hash, message, branch, parent_hash) VALUES (?, ?, ?, ?)",
            params![new_snap, &cp_message, branch.trim(), current_parent.trim()],
        )?;
        // File map: current files + all changes from the writes
        let writes_set: HashMap<&str, &str> = writes
            .iter()
            .map(|(p, h)| (p.as_str(), h.as_str()))
            .collect();
        let deletes_set: std::collections::HashSet<&str> =
            deletes.iter().map(|s| s.as_str()).collect();

        {
            let mut ins =
                tx.prepare("INSERT INTO file_map (snapshot_hash, path, hash) VALUES (?, ?, ?)")?;
            // Copy current files forward, overriding with writes and skipping deletes
            for (path, hash) in &current_files {
                if deletes_set.contains(path.as_str()) {
                    continue;
                }
                let h = writes_set
                    .get(path.as_str())
                    .copied()
                    .unwrap_or(hash.as_str());
                ins.execute(params![new_snap, path, h])?;
            }
            // New files not in current
            for (path, hash) in &writes_set {
                if !current_files.contains_key(*path) {
                    ins.execute(params![new_snap, path, hash])?;
                }
            }
        }
        tx.commit()?;
        fs::write(root.join(".velo/PARENT"), new_snap)?;

        println!(
            "\n{} Cherry-pick applied as snapshot {}",
            style("✔").green().bold(),
            style(new_snap).yellow()
        );
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
