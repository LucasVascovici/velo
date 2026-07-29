use std::collections::HashMap;
use std::fs;
use std::path::Path;

use console::style;
use similar::{ChangeTag, TextDiff};

use crate::commands::{get_dirty_files, is_binary, FileStatus};
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

/// Entry point for `velo diff`, which covers every comparison in one command:
///
/// ```text
/// velo diff                  working tree vs the last snapshot
/// velo diff <file>           just that file, working tree vs last snapshot
/// velo diff <a>              snapshot <a> vs the working tree
/// velo diff <a> <b>          snapshot <a> vs snapshot <b>
/// velo diff <a>..<b>         same as `velo diff <a> <b>`
/// velo diff -- <paths>       restrict any of the above to paths
/// ```
///
/// A single argument is a *file* when one exists by that name, otherwise it is
/// resolved as a snapshot / tag / branch / remote ref. Checking the filesystem
/// first matters: a short filename like `a` could otherwise be swallowed by a
/// hash prefix. Use `--` to force path interpretation.
pub fn dispatch(root: &Path, args: &[String], paths: &[String]) -> Result<()> {
    let parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let parent = parent.trim().to_string();

    match args {
        // velo diff [-- paths]
        [] => {
            if paths.is_empty() {
                run(root, &None)
            } else {
                run_range(root, &parent, None, paths)
            }
        }

        // velo diff <a>..<b> | <file> | <ref>
        [one] => {
            if let Some((a, b)) = split_range(one) {
                return run_range(root, a, Some(b), paths);
            }
            if is_path_like(root, one) {
                // A file: fold it in with any explicit pathspec.
                if paths.is_empty() {
                    run(root, &Some(one.clone()))
                } else {
                    let mut all = vec![one.clone()];
                    all.extend_from_slice(paths);
                    run_range(root, &parent, None, &all)
                }
            } else {
                // A snapshot/tag/branch: compare it against the working tree.
                crate::commands::resolve_snapshot_id(root, one).map_err(|_| {
                    VeloError::InvalidInput(format!(
                        "'{}' is neither a file nor a snapshot, tag, or branch.\n  \
                         To diff a path that doesn't exist any more, use: velo diff -- {}",
                        one, one
                    ))
                })?;
                run_range(root, one, None, paths)
            }
        }

        // velo diff <a> <b>
        [a, b] => run_range(root, a, Some(b), paths),

        _ => Err(VeloError::InvalidInput(
            "velo diff takes at most two snapshots. Put file paths after '--'.".into(),
        )),
    }
}

/// Split `a..b` into its ends. Returns `None` when there's no range separator.
fn split_range(spec: &str) -> Option<(&str, &str)> {
    let (a, b) = spec.split_once("..")?;
    if a.is_empty() || b.is_empty() {
        None
    } else {
        Some((a, b))
    }
}

/// Does `arg` name something on disk, or a path tracked by the current snapshot?
fn is_path_like(root: &Path, arg: &str) -> bool {
    if root.join(arg).exists() {
        return true;
    }
    // Also treat a path that was tracked but has since been deleted as a path.
    let parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let normalised = db::normalise(arg);
    db::get_conn_at_path(&root.join(".velo/velo.db"))
        .and_then(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_map
                  WHERE snapshot_hash = ? AND (path = ? OR path LIKE ? || '/%'))",
                rusqlite::params![parent.trim(), normalised, normalised],
                |r| r.get::<_, bool>(0),
            )
        })
        .unwrap_or(false)
}

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
        .replace('\r', "\n")
}

// ─── Snapshot-to-snapshot diff ────────────────────────────────────────────────

/// Diff between two snapshots (or a snapshot and the working tree).
/// `a` and `b` are resolved snapshot IDs.  If `b` is None, compare `a` against
/// the working tree.
pub fn run_range(root: &Path, a_raw: &str, b_raw: Option<&str>, paths: &[String]) -> Result<()> {
    use console::style;

    use crate::commands::resolve_snapshot_id;
    use crate::storage;

    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let objects_dir = root.join(".velo/objects");

    let a_hash = resolve_snapshot_id(root, a_raw)?;
    let b_hash_opt = match b_raw {
        Some(b) => Some(resolve_snapshot_id(root, b)?),
        None => None,
    };

    // Load file maps
    let a_files: std::collections::HashMap<String, String> = {
        let mut stmt = conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
        let x: std::collections::HashMap<String, String> = stmt
            .query_map([&a_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        x
    };

    let label_a = format!("{} ({})", a_raw, &a_hash[..8]);

    match &b_hash_opt {
        Some(b_hash) => {
            let b_files: std::collections::HashMap<String, String> = {
                let mut stmt =
                    conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
                let result: std::collections::HashMap<String, String> = stmt
                    .query_map([b_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .filter_map(|r| r.ok())
                    .collect();
                result
            };

            let label_b = format!("{} ({})", b_raw.unwrap(), &b_hash[..8]);
            let mut all_paths: Vec<&str> = a_files
                .keys()
                .chain(b_files.keys())
                .map(|s| s.as_str())
                .collect();
            all_paths.sort();
            all_paths.dedup();

            let filter = |p: &&&str| -> bool {
                paths.is_empty() || paths.iter().any(|f| p.starts_with(f.as_str()))
            };

            for path in all_paths.iter().filter(filter) {
                let old_text = match a_files.get(*path) {
                    Some(h) => {
                        let b = storage::read_object(&objects_dir, h)?;
                        String::from_utf8_lossy(&b).into_owned()
                    }
                    None => String::new(),
                };
                let new_text = match b_files.get(*path) {
                    Some(h) => {
                        let b = storage::read_object(&objects_dir, h)?;
                        String::from_utf8_lossy(&b).into_owned()
                    }
                    None => String::new(),
                };
                if old_text == new_text {
                    continue;
                }

                println!(
                    "\n{}",
                    style(format!("── {} ", path)).bold().cyan().underlined()
                );
                println!(
                    "{} {}    {} {}",
                    style("---").red(),
                    style(&label_a).dim(),
                    style("+++").green(),
                    style(&label_b).dim()
                );
                print_diff_hunks(&old_text, &new_text);
            }
        }
        None => {
            // Compare snapshot a against the working tree
            let label_b = "working tree";
            let filter = |p: &str| -> bool {
                paths.is_empty() || paths.iter().any(|f| p.starts_with(f.as_str()))
            };

            let dirty = crate::commands::get_dirty_files(root);
            // Only diff files that are either in snapshot a or dirty
            let mut candidates: Vec<String> = a_files
                .keys()
                .cloned()
                .chain(dirty.keys().cloned())
                .collect();
            candidates.sort();
            candidates.dedup();

            for path in candidates.iter().filter(|p| filter(p)) {
                let old_text = match a_files.get(path.as_str()) {
                    Some(h) => {
                        let b = storage::read_object(&objects_dir, h)?;
                        String::from_utf8_lossy(&b).into_owned()
                    }
                    None => String::new(),
                };
                let full = root.join(db::db_to_path(path));
                let new_text = std::fs::read_to_string(&full).unwrap_or_default();
                if old_text == new_text {
                    continue;
                }

                println!(
                    "\n{}",
                    style(format!("── {} ", path)).bold().cyan().underlined()
                );
                println!(
                    "{} {}    {} {}",
                    style("---").red(),
                    style(&label_a).dim(),
                    style("+++").green(),
                    style(label_b).dim()
                );
                print_diff_hunks(&old_text, &new_text);
            }
        }
    }

    Ok(())
}

fn print_diff_hunks(old: &str, new: &str) {
    let old_n = normalise(old);
    let new_n = normalise(new);
    let diff = TextDiff::from_lines(&old_n, &new_n);
    for hunk in diff.grouped_ops(3) {
        let old_start = hunk.first().map(|o| o.old_range().start + 1).unwrap_or(1);
        let old_count: usize = hunk.iter().map(|o| o.old_range().len()).sum();
        let new_start = hunk.first().map(|o| o.new_range().start + 1).unwrap_or(1);
        let new_count: usize = hunk.iter().map(|o| o.new_range().len()).sum();
        println!(
            "{}",
            console::style(format!(
                "@@ -{},{} +{},{} @@",
                old_start, old_count, new_start, new_count
            ))
            .cyan()
        );
        for op in &hunk {
            for change in diff.iter_changes(op) {
                let tag: ChangeTag = change.tag();
                let (sign, col) = match tag {
                    ChangeTag::Delete => ("-", console::Color::Red),
                    ChangeTag::Insert => ("+", console::Color::Green),
                    ChangeTag::Equal => (" ", console::Color::White),
                };
                let ln = change.new_index().or(change.old_index()).map(|i| i + 1);
                print!(
                    "{:>5} {} ",
                    console::style(format!("{}", ln.unwrap_or(0))).dim(),
                    console::style(sign).fg(col).bold()
                );
                let line = change.value();
                print!("{}", console::style(line).fg(col));
                if !line.ends_with('\n') {
                    println!();
                }
            }
        }
    }
}
