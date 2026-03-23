//! `velo blame <file>` — annotate each line with the snapshot that last changed it.
//!
//! Algorithm: walk history from HEAD backwards; for each snapshot compute a diff
//! against its parent. Any line that was added or changed by this snapshot is
//! attributed to it. Lines already attributed are skipped.

use std::collections::HashMap;
use std::path::Path;

use console::style;
use rusqlite::params;
use similar::{ChangeTag, TextDiff};

use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

#[allow(unused_assignments)]
pub fn run(root: &Path, file: &str, at: Option<&str>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let objects_dir = root.join(".velo/objects");
    let rel = db::normalise(file);

    // Resolve starting snapshot
    let start_hash = match at {
        Some(t) => crate::commands::resolve_snapshot_id(root, t)?,
        None => std::fs::read_to_string(root.join(".velo/PARENT"))
            .unwrap_or_default()
            .trim()
            .to_string(),
    };

    if start_hash.is_empty() {
        return Err(VeloError::InvalidInput("No snapshots yet.".into()));
    }

    // Load the file content at the starting snapshot
    let tip_hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
            params![start_hash, rel],
            |r| r.get(0),
        )
        .ok();

    let tip_hash = tip_hash.ok_or_else(|| {
        VeloError::InvalidInput(format!(
            "'{}' is not tracked in snapshot {}.",
            file, &start_hash[..8]
        ))
    })?;

    let tip_content = storage::read_object(&objects_dir, &tip_hash)?;
    let tip_text = String::from_utf8_lossy(&tip_content);
    let total_lines: usize = tip_text.lines().count();

    // annotations[i] = Some((hash, date, message)) for line i
    let mut annotations: Vec<Option<(String, String, String)>> = vec![None; total_lines];
    let mut remaining = total_lines;

    // Walk ancestry
    let mut walk_hash = start_hash.clone();
    loop {
        if remaining == 0 {
            break;
        }

        // Load this snapshot's file content + its parent's
        let (parent_hash, snap_date, snap_msg): (String, String, String) = match conn
            .query_row(
                "SELECT parent_hash, created_at, message FROM snapshots WHERE hash = ?",
                [&walk_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ) {
            Ok(v) => v,
            Err(_) => break,
        };

        let snap_date = if snap_date.len() >= 16 {
            snap_date[..16].replace('T', " ")
        } else {
            snap_date.clone()
        };

        let our_hash: Option<String> = conn
            .query_row(
                "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
                params![walk_hash, rel],
                |r| r.get(0),
            )
            .ok();

        let par_hash: Option<String> = conn
            .query_row(
                "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
                params![parent_hash, rel],
                |r| r.get(0),
            )
            .ok();

        // Get the tip content to produce line indices relative to it
        let our_text = match &our_hash {
            Some(h) => {
                let b = storage::read_object(&objects_dir, h)?;
                String::from_utf8_lossy(&b).into_owned()
            }
            None => String::new(),
        };
        let par_text = match &par_hash {
            Some(h) => {
                let b = storage::read_object(&objects_dir, h)?;
                String::from_utf8_lossy(&b).into_owned()
            }
            None => String::new(),
        };

        // Diff tip → our to find which tip lines correspond to our lines
        // Then diff par → our to find lines this snapshot introduced
        let tip_text_owned = {
            let b = storage::read_object(&objects_dir, &tip_hash)?;
            String::from_utf8_lossy(&b).into_owned()
        };

        // Map: our_line_idx → tip_line_idx (for lines equal in both)
        let our_to_tip = line_map(&our_text, &tip_text_owned);
        // Lines that changed in our vs parent
        let changed_in_our = changed_lines(&par_text, &our_text);

        for our_idx in &changed_in_our {
            if let Some(&tip_idx) = our_to_tip.get(our_idx) {
                if tip_idx < annotations.len() && annotations[tip_idx].is_none() {
                    annotations[tip_idx] =
                        Some((walk_hash[..8].to_string(), snap_date.clone(), snap_msg.clone()));
                    remaining -= 1;
                }
            }
        }

        if parent_hash.is_empty() {
            // Root commit — attribute all remaining lines to it
            for ann in annotations.iter_mut() {
                if ann.is_none() {
                    *ann = Some((walk_hash[..8].to_string(), snap_date.clone(), snap_msg.clone()));
                    remaining -= 1;
                }
            }
            break;
        }
        walk_hash = parent_hash;
    }

    // Print
    let lines: Vec<&str> = tip_text.lines().collect();
    let max_msg = 28usize;
    for (i, line) in lines.iter().enumerate() {
        let (hash_s, date_s, msg_s) = match &annotations[i] {
            Some((h, d, m)) => (
                style(h).yellow().to_string(),
                style(d).dim().to_string(),
                {
                    let truncated = if m.len() > max_msg {
                        format!("{}…", &m[..max_msg - 1])
                    } else {
                        m.clone()
                    };
                    style(truncated).dim().to_string()
                },
            ),
            None => (
                style("????????").dim().to_string(),
                style("                ").dim().to_string(),
                style("(unknown)").dim().to_string(),
            ),
        };
        println!(
            "{} {} {:<28}  {}  {}",
            hash_s,
            date_s,
            msg_s,
            style(format!("{:>4}", i + 1)).dim(),
            line
        );
    }

    Ok(())
}

/// Returns a map from `new` line index → `old` line index for Equal ops.
fn line_map(old: &str, new: &str) -> HashMap<usize, usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let diff = TextDiff::from_slices(&old_lines, &new_lines);
    let mut map = HashMap::new();
    for op in diff.ops() {
        if let similar::DiffOp::Equal { old_index, new_index, .. } = op {
            let len = op.old_range().len();
            for i in 0..len {
                map.insert(new_index + i, old_index + i);
            }
        }
    }
    map
}

/// Returns the set of `new` line indices that differ from `old`.
fn changed_lines(old: &str, new: &str) -> Vec<usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let diff = TextDiff::from_slices(&old_lines, &new_lines);
    let mut changed = Vec::new();
    for change in diff.iter_all_changes() {
        if change.tag() == ChangeTag::Insert {
            if let Some(idx) = change.new_index() {
                changed.push(idx);
            }
        }
    }
    changed
}