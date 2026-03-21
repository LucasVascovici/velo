//! `velo show <target>` — inspect a snapshot without restoring it.
//!
//! Shows the full diff of a snapshot vs its parent, or just one file.
//!   velo show <hash|tag>
//!   velo show <hash|tag> -- src/auth.py

use std::path::Path;

use console::style;
use similar::{ChangeTag, TextDiff};

use crate::commands::is_binary;
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(root: &Path, target: &str, file_filter: &Option<String>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // Resolve target to a full hash
    let snap_hash = crate::commands::resolve_snapshot_id(root, target)?;

    // Look up the snapshot metadata
    let (message, branch, parent_hash, created_at): (String, String, String, String) = conn
        .query_row(
            "SELECT message, branch, parent_hash, created_at FROM snapshots WHERE hash = ?",
            [&snap_hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| VeloError::InvalidInput(format!("Snapshot '{}' not found.", target)))?;

    // Header
    let date = if created_at.len() >= 19 {
        &created_at[..19]
    } else {
        &created_at
    };
    println!(
        "{} {}  {}  {}",
        style("snapshot").dim(),
        style(&snap_hash).yellow().bold(),
        style(&branch).cyan(),
        style(date).dim()
    );
    println!("  {}", style(&message).white().bold());
    if !parent_hash.is_empty() {
        println!("  parent: {}", style(&parent_hash).yellow().dim());
    }
    println!();

    diff_snapshots(root, &conn, &parent_hash, &snap_hash, file_filter)
}

/// Diff two snapshot hashes and print the result.
/// `old_hash` may be empty (meaning the snapshot has no parent).
pub fn diff_snapshots(
    root: &Path,
    conn: &rusqlite::Connection,
    old_hash: &str,
    new_hash: &str,
    file_filter: &Option<String>,
) -> Result<()> {
    let objects_dir = root.join(".velo/objects");

    // Load file maps for both snapshots
    let old_files = load_file_map(conn, old_hash)?;
    let new_files = load_file_map(conn, new_hash)?;

    // Union of all paths, sorted
    let mut all_paths: Vec<&str> = old_files
        .keys()
        .chain(new_files.keys())
        .map(|s| s.as_str())
        .collect();
    all_paths.sort_unstable();
    all_paths.dedup();

    // Apply file filter if provided
    let filter = file_filter.as_deref();

    let mut any_output = false;

    for path in all_paths {
        if let Some(f) = filter {
            // Allow prefix match so "src/auth" matches "src/auth.py"
            let normalised_f = db::normalise(f);
            if !path.starts_with(normalised_f.as_str()) && path != normalised_f.as_str() {
                continue;
            }
        }

        let old_hash_opt = old_files.get(path);
        let new_hash_opt = new_files.get(path);

        match (old_hash_opt, new_hash_opt) {
            (None, Some(nh)) => {
                // Added
                println!(
                    "{} {}",
                    style("+++ new file:").green().bold(),
                    style(path).green()
                );
                let full_path = root.join(db::db_to_path(path));
                if is_binary(&full_path) {
                    println!("  {}", style("(binary)").dim());
                } else {
                    let data = storage::read_object(&objects_dir, nh)?;
                    let content = String::from_utf8_lossy(&data);
                    for line in content.lines() {
                        println!("{}", style(format!("+{}", line)).green());
                    }
                }
                any_output = true;
            }
            (Some(_), None) => {
                // Deleted
                println!(
                    "{} {}",
                    style("--- deleted:").red().bold(),
                    style(path).red()
                );
                any_output = true;
            }
            (Some(oh), Some(nh)) if oh != nh => {
                // Modified
                println!(
                    "\n{} {}",
                    style("~~~ modified:").yellow().bold(),
                    style(path).yellow().underlined()
                );
                let full_path = root.join(db::db_to_path(path));
                if is_binary(&full_path) {
                    println!("  {}", style("binary file changed").dim());
                } else {
                    let old_data = storage::read_object(&objects_dir, oh)?;
                    let new_data = storage::read_object(&objects_dir, nh)?;
                    let old_text = normalise_text(&String::from_utf8_lossy(&old_data));
                    let new_text = normalise_text(&String::from_utf8_lossy(&new_data));
                    print_diff(&old_text, &new_text);
                }
                any_output = true;
            }
            _ => {} // Identical — skip
        }
    }

    if !any_output {
        println!("{}", style("No changes in this snapshot.").dim());
    }
    Ok(())
}

fn load_file_map(
    conn: &rusqlite::Connection,
    snap_hash: &str,
) -> Result<std::collections::HashMap<String, String>> {
    if snap_hash.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut stmt = conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
    let collected: std::collections::HashMap<String, String> = stmt
        .query_map([snap_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(collected)
}

fn print_diff(old: &str, new: &str) {
    let diff = TextDiff::from_lines(old, new);
    for hunk in diff.grouped_ops(3) {
        let old_start = hunk.first().map(|op| op.old_range().start + 1).unwrap_or(1);
        let old_count: usize = hunk.iter().map(|op| op.old_range().len()).sum();
        let new_start = hunk.first().map(|op| op.new_range().start + 1).unwrap_or(1);
        let new_count: usize = hunk.iter().map(|op| op.new_range().len()).sum();
        println!(
            "{}",
            style(format!(
                "@@ -{},{} +{},{} @@",
                old_start, old_count, new_start, new_count
            ))
            .cyan()
            .dim()
        );
        for op in &hunk {
            for change in diff.iter_changes(op) {
                let (sign, color) = match change.tag() {
                    ChangeTag::Delete => ("-", console::Color::Red),
                    ChangeTag::Insert => ("+", console::Color::Green),
                    ChangeTag::Equal => (" ", console::Color::White),
                };
                let ln = match change.tag() {
                    ChangeTag::Delete => change.old_index().map(|i| i + 1),
                    _ => change.new_index().map(|i| i + 1),
                };
                let ln_str = ln
                    .map(|n| format!("{:>5}", n))
                    .unwrap_or_else(|| "     ".into());
                print!(
                    "{} {}{}",
                    style(ln_str).dim(),
                    style(sign).fg(color).bold(),
                    style(change.value()).fg(color)
                );
            }
        }
    }
}

fn normalise_text(s: &str) -> String {
    s.strip_prefix('\u{feff}')
        .unwrap_or(s)
        .replace("\r\n", "\n")
}
