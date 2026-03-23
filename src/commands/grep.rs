//! `velo grep <pattern>` — search tracked files for a regex or literal pattern.
//!
//! Searches the working tree by default.  With --snapshot, searches the stored
//! content at that snapshot without touching the working tree.

use std::path::Path;

use console::style;
use rusqlite::params;

use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

pub fn run(
    root:     &Path,
    pattern:  &str,
    snapshot: Option<&str>,
    case:     bool,   // -i: case-insensitive
    files_only: bool, // -l: only print file names
    context:  usize,  // -C: context lines
) -> Result<()> {
    // Compile the pattern
    let re = build_regex(pattern, case)?;

    if let Some(target) = snapshot {
        grep_snapshot(root, &re, target, pattern, files_only, context)
    } else {
        grep_working_tree(root, &re, pattern, files_only, context)
    }
}

// ── Working tree search ────────────────────────────────────────────────────────

fn grep_working_tree(
    root:       &Path,
    re:         &regex::Regex,
    pattern:    &str,
    files_only: bool,
    context:    usize,
) -> Result<()> {
    let entries = crate::commands::walk_with_meta(root);
    let mut any = false;

    for entry in entries {
        let rel = db::normalise(
            entry.path.strip_prefix(root).unwrap().to_str().unwrap(),
        );
        let content = match std::fs::read_to_string(&entry.path) {
            Ok(s) => s,
            Err(_) => continue, // binary / unreadable
        };
        if print_matches(&rel, &content, re, pattern, files_only, context) {
            any = true;
        }
    }

    if !any {
        println!("{}", style(format!("No matches for '{}'.", pattern)).dim());
    }
    Ok(())
}

// ── Snapshot search ────────────────────────────────────────────────────────────

fn grep_snapshot(
    root:       &Path,
    re:         &regex::Regex,
    target:     &str,
    pattern:    &str,
    files_only: bool,
    context:    usize,
) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let objects_dir = root.join(".velo/objects");

    let snap_hash = crate::commands::resolve_snapshot_id(root, target)?;
    let (snap_msg, snap_date): (String, String) = conn
        .query_row(
            "SELECT message, created_at FROM snapshots WHERE hash = ?",
            [&snap_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| VeloError::InvalidInput(format!("Snapshot '{}' not found.", target)))?;

    println!(
        "\n{} {}  {}",
        style("Searching snapshot").dim(),
        style(&snap_hash[..8]).yellow(),
        style(snap_msg).dim()
    );

    let mut stmt = conn.prepare(
        "SELECT path, hash FROM file_map WHERE snapshot_hash = ? ORDER BY path",
    )?;
    let files: Vec<(String, String)> = stmt
        .query_map(params![snap_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut any = false;
    for (path, hash) in files {
        let bytes = match storage::read_object(&objects_dir, &hash) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if print_matches(&path, &content, re, pattern, files_only, context) {
            any = true;
        }
    }

    let _ = snap_date;
    if !any {
        println!("{}", style(format!("No matches for '{}'.", pattern)).dim());
    }
    Ok(())
}

// ── Shared output ──────────────────────────────────────────────────────────────

/// Print matches in `content` for file at `rel_path`.
/// Returns true if any match was found.
fn print_matches(
    rel_path:   &str,
    content:    &str,
    re:         &regex::Regex,
    _pattern:   &str,
    files_only: bool,
    context:    usize,
) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    let matching: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| re.is_match(l))
        .map(|(i, _)| i)
        .collect();

    if matching.is_empty() {
        return false;
    }

    println!(
        "\n{}",
        style(rel_path).cyan().bold().underlined()
    );

    if files_only {
        return true;
    }

    // Group matches into context windows to avoid repeating lines
    let mut printed: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for &mi in &matching {
        let start = mi.saturating_sub(context);
        let end   = (mi + context + 1).min(lines.len());

        // Separator if there's a gap
        if !printed.is_empty() {
            let last = *printed.iter().max().unwrap();
            if start > last + 1 {
                println!("  {}", style("···").dim());
            }
        }

        for i in start..end {
            if !printed.contains(&i) {
                let line_no = style(format!("{:>4}", i + 1)).dim();
                let separator = if i == mi {
                    style(":").yellow().bold().to_string()
                } else {
                    style("│").dim().to_string()
                };

                if i == mi {
                    // Highlight the match within the line
                    let highlighted = highlight_match(lines[i], re);
                    println!("  {}{}  {}", line_no, separator, highlighted);
                } else {
                    println!("  {}{}  {}", line_no, separator, style(lines[i]).dim());
                }
                printed.insert(i);
            }
        }
    }
    true
}

/// Wrap matching substrings in bold+yellow.
fn highlight_match(line: &str, re: &regex::Regex) -> String {
    let mut result = String::new();
    let mut last = 0;
    for m in re.find_iter(line) {
        result.push_str(&line[last..m.start()]);
        result.push_str(&style(m.as_str()).yellow().bold().to_string());
        last = m.end();
    }
    result.push_str(&line[last..]);
    result
}

fn build_regex(pattern: &str, case_insensitive: bool) -> Result<regex::Regex> {
    let p = if case_insensitive {
        format!("(?i){}", pattern)
    } else {
        pattern.to_string()
    };
    regex::Regex::new(&p).map_err(|e| {
        VeloError::InvalidInput(format!("Invalid regex '{}': {}", pattern, e))
    })
}