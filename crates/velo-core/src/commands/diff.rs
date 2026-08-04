//! `velo diff` — every comparison in one command, returned as data.
//!
//! This module owns the diff data model that `show` and `stash show` also build
//! on, so there is exactly one representation of "what changed" in the codebase.
//! Formatting lives in `velo-cli`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use similar::{ChangeTag, TextDiff};

use crate::commands::{get_dirty_files, is_binary, FileStatus};
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;
use crate::Repo;

// ─── Data model ───────────────────────────────────────────────────────────────

/// What a line represents in a hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineTag {
    /// Unchanged, shown for context.
    Context,
    Added,
    Removed,
}

/// One line of a hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub tag: LineTag,
    /// The line number worth showing: the old file's for removals, the new file's
    /// otherwise. `None` when the line exists on neither side.
    pub line_no: Option<usize>,
    /// Line content with the trailing newline stripped.
    pub text: String,
}

/// A contiguous run of changes plus its surrounding context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<DiffLine>,
}

/// How one file changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// The file is new. Carries its full content, since there is no old side to
    /// diff against and every line is an addition.
    Added {
        lines: Vec<String>,
    },
    /// The file is gone. No content to show.
    Deleted,
    Modified {
        hunks: Vec<Hunk>,
    },
    /// Binary content changed; a textual diff would be meaningless.
    BinaryChanged {
        added: bool,
    },
}

/// One file's entry in a diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Normalised (forward-slash) repository path.
    pub path: String,
    pub change: FileChange,
}

/// A complete comparison between two states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diff {
    /// What the left-hand side is, e.g. `last saved` or `v1.0 (a1b2c3d4)`.
    pub old_label: String,
    /// What the right-hand side is, e.g. `working tree`.
    pub new_label: String,
    /// Changed files, in path order. Unchanged files are omitted.
    pub files: Vec<FileDiff>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Number of files that differ.
    pub fn changed_files(&self) -> usize {
        self.files.len()
    }
}

// ─── Dispatch ─────────────────────────────────────────────────────────────────

/// Resolve the argument forms of `velo diff` and produce the comparison:
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
pub fn dispatch(repo: &Repo, args: &[String], paths: &[String]) -> Result<Diff> {
    let root = repo.root();
    let parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let parent = parent.trim().to_string();

    match args {
        [] => {
            if paths.is_empty() {
                run(repo, &None)
            } else {
                run_range(repo, &parent, None, paths)
            }
        }

        [one] => {
            if let Some((a, b)) = split_range(one) {
                return run_range(repo, a, Some(b), paths);
            }
            if is_path_like(repo, one) {
                // A file: fold it in with any explicit pathspec.
                if paths.is_empty() {
                    run(repo, &Some(one.clone()))
                } else {
                    let mut all = vec![one.clone()];
                    all.extend_from_slice(paths);
                    run_range(repo, &parent, None, &all)
                }
            } else {
                // A snapshot/tag/branch: compare it against the working tree.
                crate::commands::resolve_snapshot_id(repo, one).map_err(|_| {
                    VeloError::invalid(format!(
                        "'{}' is neither a file nor a snapshot, tag, or branch.\n  \
                         To diff a path that doesn't exist any more, use: velo diff -- {}",
                        one, one
                    ))
                })?;
                run_range(repo, one, None, paths)
            }
        }

        [a, b] => run_range(repo, a, Some(b), paths),

        _ => Err(VeloError::invalid(
            "velo diff takes at most two snapshots. Put file paths after '--'.",
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
fn is_path_like(repo: &Repo, arg: &str) -> bool {
    let root = repo.root();
    if root.join(arg).exists() {
        return true;
    }
    // Also treat a path that was tracked but has since been deleted as a path.
    let parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let normalised = db::normalise(arg);
    repo.conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM file_map
              WHERE snapshot_hash = ? AND (path = ? OR path LIKE ? || '/%'))",
            rusqlite::params![parent.trim(), normalised, normalised],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false)
}

// ─── Working tree vs last snapshot ────────────────────────────────────────────

/// Compare the working tree against the last snapshot, optionally for one file.
pub fn run(repo: &Repo, target_file: &Option<String>) -> Result<Diff> {
    let dirty = get_dirty_files(repo);

    let selected: Vec<String> = match target_file {
        Some(file) => vec![db::normalise(file)],
        None => {
            let mut keys: Vec<String> = dirty.keys().cloned().collect();
            keys.sort_unstable();
            keys
        }
    };

    let mut files = Vec::new();
    for path in selected {
        files.push(FileDiff {
            change: working_tree_change(repo, &path, &dirty)?,
            path,
        });
    }

    Ok(Diff {
        old_label: "last saved".into(),
        new_label: "working tree".into(),
        files,
    })
}

/// Classify one path as it stands on disk against the last snapshot.
fn working_tree_change(
    repo: &Repo,
    rel_path: &str,
    dirty: &HashMap<String, FileStatus>,
) -> Result<FileChange> {
    let root = repo.root();
    if dirty.get(rel_path) == Some(&FileStatus::Deleted) {
        return Ok(FileChange::Deleted);
    }

    let full_path = root.join(db::db_to_path(rel_path));
    if is_binary(&full_path) {
        return Ok(FileChange::BinaryChanged {
            added: dirty.get(rel_path) == Some(&FileStatus::New),
        });
    }

    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let stored: Option<String> = repo
        .conn()
        .query_row(
            "SELECT hash FROM file_map WHERE path = ? AND snapshot_hash = ?",
            [rel_path, parent_hash.trim()],
            |r| r.get(0),
        )
        .ok();

    let old = match stored {
        Some(h) => {
            let bytes = storage::read_object(&root.join(".velo/objects"), &h)?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        None => String::new(),
    };
    let new = fs::read_to_string(&full_path).unwrap_or_default();

    Ok(FileChange::Modified {
        hunks: build_hunks(&old, &new),
    })
}

// ─── Snapshot ranges ──────────────────────────────────────────────────────────

/// Compare snapshot `a_raw` against snapshot `b_raw`, or against the working tree
/// when `b_raw` is `None`. `paths`, when non-empty, restricts the comparison.
pub fn run_range(repo: &Repo, a_raw: &str, b_raw: Option<&str>, paths: &[String]) -> Result<Diff> {
    let root = repo.root();
    let conn = repo.conn();
    let objects_dir = root.join(".velo/objects");

    let a_hash = crate::commands::resolve_snapshot_id(repo, a_raw)?;
    let a_files = load_file_map(conn, &a_hash)?;
    let old_label = format!("{} ({})", a_raw, &a_hash[..8]);

    let matches_filter =
        |p: &str| paths.is_empty() || paths.iter().any(|f| p.starts_with(f.as_str()));

    let (new_label, files) = match b_raw {
        Some(b) => {
            let b_hash = crate::commands::resolve_snapshot_id(repo, b)?;
            let b_files = load_file_map(conn, &b_hash)?;
            let label = format!("{} ({})", b, &b_hash[..8]);

            let mut out = Vec::new();
            for path in union_paths(&a_files, &b_files) {
                if !matches_filter(&path) {
                    continue;
                }
                let old = read_opt(&objects_dir, a_files.get(&path))?;
                let new = read_opt(&objects_dir, b_files.get(&path))?;
                if old == new {
                    continue;
                }
                out.push(FileDiff {
                    change: FileChange::Modified {
                        hunks: build_hunks(&old, &new),
                    },
                    path,
                });
            }
            (label, out)
        }
        None => {
            let dirty = get_dirty_files(repo);
            let mut candidates: Vec<String> = a_files
                .keys()
                .cloned()
                .chain(dirty.keys().cloned())
                .collect();
            candidates.sort_unstable();
            candidates.dedup();

            let mut out = Vec::new();
            for path in candidates {
                if !matches_filter(&path) {
                    continue;
                }
                let old = read_opt(&objects_dir, a_files.get(&path))?;
                let new = fs::read_to_string(root.join(db::db_to_path(&path))).unwrap_or_default();
                if old == new {
                    continue;
                }
                out.push(FileDiff {
                    change: FileChange::Modified {
                        hunks: build_hunks(&old, &new),
                    },
                    path,
                });
            }
            ("working tree".to_string(), out)
        }
    };

    Ok(Diff {
        old_label,
        new_label,
        files,
    })
}

// ─── Snapshot vs its parent (used by `show` and `stash show`) ─────────────────

/// Diff `new_hash` against `old_hash`, distinguishing added and deleted files.
///
/// `old_hash` may be empty, meaning the snapshot has no parent, in which case
/// every file reads as added. `file_filter` matches by prefix so `src/auth`
/// selects `src/auth.py`.
pub(crate) fn snapshot_diff(
    repo: &Repo,
    conn: &rusqlite::Connection,
    old_hash: &str,
    new_hash: &str,
    file_filter: &Option<String>,
) -> Result<Diff> {
    let root = repo.root();
    let objects_dir = root.join(".velo/objects");
    let old_files = load_file_map(conn, old_hash)?;
    let new_files = load_file_map(conn, new_hash)?;

    let filter = file_filter.as_deref().map(db::normalise);
    let mut files = Vec::new();

    for path in union_paths(&old_files, &new_files) {
        if let Some(f) = &filter {
            if !path.starts_with(f.as_str()) {
                continue;
            }
        }

        let full_path = root.join(db::db_to_path(&path));
        let change = match (old_files.get(&path), new_files.get(&path)) {
            (None, Some(nh)) => {
                if is_binary(&full_path) {
                    FileChange::BinaryChanged { added: true }
                } else {
                    let bytes = storage::read_object(&objects_dir, nh)?;
                    FileChange::Added {
                        lines: String::from_utf8_lossy(&bytes)
                            .lines()
                            .map(str::to_string)
                            .collect(),
                    }
                }
            }
            (Some(_), None) => FileChange::Deleted,
            (Some(oh), Some(nh)) if oh != nh => {
                if is_binary(&full_path) {
                    FileChange::BinaryChanged { added: false }
                } else {
                    let old = read_text(&objects_dir, oh)?;
                    let new = read_text(&objects_dir, nh)?;
                    FileChange::Modified {
                        hunks: build_hunks(&old, &new),
                    }
                }
            }
            _ => continue, // identical, or absent from both
        };
        files.push(FileDiff { path, change });
    }

    Ok(Diff {
        old_label: if old_hash.is_empty() {
            "(empty)".into()
        } else {
            old_hash[..8.min(old_hash.len())].to_string()
        },
        new_label: new_hash[..8.min(new_hash.len())].to_string(),
        files,
    })
}

// ─── Hunk construction ────────────────────────────────────────────────────────

/// Build the hunks between two texts, with three lines of context.
///
/// Content is normalised first: a leading byte-order mark and CR characters are
/// stripped so a line-ending change alone doesn't read as a whole-file rewrite.
pub fn build_hunks(old: &str, new: &str) -> Vec<Hunk> {
    let old_n = normalise(old);
    let new_n = normalise(new);
    let diff = TextDiff::from_lines(&old_n, &new_n);

    diff.grouped_ops(3)
        .into_iter()
        .map(|ops| {
            let mut lines = Vec::new();
            for op in &ops {
                for change in diff.iter_changes(op) {
                    let tag = match change.tag() {
                        ChangeTag::Delete => LineTag::Removed,
                        ChangeTag::Insert => LineTag::Added,
                        ChangeTag::Equal => LineTag::Context,
                    };
                    // Removals only exist on the old side; everything else is
                    // numbered against the new file.
                    let line_no = match tag {
                        LineTag::Removed => change.old_index(),
                        _ => change.new_index(),
                    }
                    .map(|i| i + 1);
                    lines.push(DiffLine {
                        tag,
                        line_no,
                        text: change
                            .value()
                            .strip_suffix('\n')
                            .unwrap_or(change.value())
                            .to_string(),
                    });
                }
            }
            Hunk {
                old_start: ops.first().map(|o| o.old_range().start + 1).unwrap_or(1),
                old_count: ops.iter().map(|o| o.old_range().len()).sum(),
                new_start: ops.first().map(|o| o.new_range().start + 1).unwrap_or(1),
                new_count: ops.iter().map(|o| o.new_range().len()).sum(),
                lines,
            }
        })
        .collect()
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn load_file_map(conn: &rusqlite::Connection, snap_hash: &str) -> Result<HashMap<String, String>> {
    if snap_hash.is_empty() {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")?;
    let map: HashMap<String, String> = stmt
        .query_map([snap_hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(map)
}

/// Sorted, deduplicated union of the paths in both maps.
fn union_paths(a: &HashMap<String, String>, b: &HashMap<String, String>) -> Vec<String> {
    let mut all: Vec<String> = a.keys().chain(b.keys()).cloned().collect();
    all.sort_unstable();
    all.dedup();
    all
}

fn read_text(objects_dir: &Path, object: &str) -> Result<String> {
    let bytes = storage::read_object(objects_dir, object)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Content of an optional object, empty when absent.
fn read_opt(objects_dir: &Path, object: Option<&String>) -> Result<String> {
    match object {
        Some(h) => read_text(objects_dir, h),
        None => Ok(String::new()),
    }
}

fn normalise(s: &str) -> String {
    s.strip_prefix('\u{feff}')
        .unwrap_or(s)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}
