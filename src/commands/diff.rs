use std::collections::HashMap;
use std::fs;
use std::path::Path;

use console::style;
use similar::{ChangeTag, TextDiff};

use crate::commands::{get_dirty_files, is_binary, FileStatus};
use crate::db;
use crate::error::Result;
use crate::storage;

pub fn run(root: &Path, target_file: &Option<String>) -> Result<()> {
    match target_file {
        Some(file) => {
            let dirty = get_dirty_files(root);
            diff_one(root, file, &dirty)
        }
        None => {
            let dirty = get_dirty_files(root);
            if dirty.is_empty() {
                println!("{}", style("Working directory clean.").dim());
                return Ok(());
            }
            let mut keys: Vec<&String> = dirty.keys().collect();
            keys.sort();
            for file in keys {
                println!(
                    "\n{}",
                    style(format!("── {} ", file)).bold().cyan().underlined()
                );
                diff_one(root, file, &dirty)?;
            }
            Ok(())
        }
    }
}

fn diff_one(root: &Path, rel_path: &str, dirty: &HashMap<String, FileStatus>) -> Result<()> {
    // ── Deleted file shortcut ─────────────────────────────────────────────────
    if dirty.get(rel_path) == Some(&FileStatus::Deleted) {
        println!("{} '{}' was deleted.", style("[-]").red().bold(), rel_path);
        return Ok(());
    }

    // ── Binary guard ─────────────────────────────────────────────────────────
    let full_path = root.join(rel_path);
    if is_binary(&full_path) {
        println!(
            "{} Binary file '{}' modified (diff omitted).",
            style("[~]").yellow().bold(),
            rel_path
        );
        return Ok(());
    }

    // ── Gather old and new content ────────────────────────────────────────────
    let (old_content, new_content, label_old, label_new) = {
        let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        let last_hash: Option<String> = conn
            .query_row(
                "SELECT hash FROM file_map WHERE path = ? AND snapshot_hash = ?",
                [rel_path, parent_hash.trim()],
                |r| r.get(0),
            )
            .ok();

        let old = if let Some(h) = last_hash {
            let objects_dir = root.join(".velo/objects");
            String::from_utf8_lossy(&storage::read_object(&objects_dir, &h)?).into_owned()
        } else {
            String::new()
        };

        let new = fs::read_to_string(&full_path).unwrap_or_default();
        (
            old,
            new,
            "last saved".to_string(),
            "working tree".to_string(),
        )
    };

    let old_norm = normalise(&old_content);
    let new_norm = normalise(&new_content);
    let diff = TextDiff::from_lines(&old_norm, &new_norm);

    println!(
        "{} {}    {} {}",
        style("---").red(),
        style(&label_old).dim(),
        style("+++").green(),
        style(&label_new).dim()
    );

    for hunk in diff.grouped_ops(3) {
        // Compute hunk header line numbers
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

                // Show the relevant line number: old for deletions, new for
                // insertions and context lines.
                let line_no = match change.tag() {
                    ChangeTag::Delete => change.old_index().map(|i| i + 1),
                    _ => change.new_index().map(|i| i + 1),
                };
                let ln = line_no
                    .map(|n| format!("{:>5}", n))
                    .unwrap_or_else(|| "     ".into());

                print!(
                    "{} {}{}",
                    style(ln).dim(),
                    style(sign).fg(color).bold(),
                    style(change.value()).fg(color)
                );
            }
        }
    }
    Ok(())
}

fn normalise(s: &str) -> String {
    s.strip_prefix('\u{feff}')
        .unwrap_or(s)
        .replace("\r\n", "\n")
}
