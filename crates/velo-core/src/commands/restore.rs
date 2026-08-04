//! `velo restore` — put the working tree back to a snapshot.
//!
//! Returns what it did as data. This matters more here than elsewhere: eleven
//! other commands call `run` as a step of their own work (merge, switch, undo,
//! redo, rebase, stash, sync), and while it printed, all of them leaked its
//! progress lines into the middle of their own output.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::params;

use crate::commands::{get_dirty_files, get_tracked_files, remove_empty_parents};
use crate::error::{RefKind, Result, VeloError};
use crate::storage;
use crate::WriteGuard;

/// What a restore did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Already at the target with a clean tree, so nothing was touched.
    AlreadyThere { snapshot: String },
    /// A full restore: the working tree and the recorded position both moved.
    Restored {
        snapshot: String,
        /// The branch we are on — not the one that created the snapshot, since
        /// branches legitimately share commits.
        branch: String,
        /// Message of the restored snapshot.
        message: String,
        /// Files written from the object store.
        files: usize,
        /// Tracked files removed because the target snapshot doesn't have them.
        ghosts_removed: usize,
        /// Unsaved changes discarded because `force` was set.
        discarded: usize,
    },
    /// A path-limited restore. The recorded position is deliberately left alone,
    /// so this is a file-level revert rather than a move through history.
    RestoredPaths {
        snapshot: String,
        files: usize,
        discarded: usize,
    },
    /// The pathspec matched nothing in that snapshot.
    NoMatchingPaths { snapshot: String },
}

/// Restore the working tree to `snapshot_hash`.
///
/// `force` discards unsaved changes. When `paths` is non-empty only those paths
/// are written and the recorded position is left where it is.
pub fn run(
    guard: &WriteGuard,
    snapshot_hash: &str,
    force: bool,
    paths: &[String],
) -> Result<Outcome> {
    let root = guard.root();
    let partial = !paths.is_empty();

    // Nothing to do when a full restore targets where we already are.
    if !partial {
        let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        if position.trim() == snapshot_hash && get_dirty_files(guard.repo()).is_empty() {
            return Ok(Outcome::AlreadyThere {
                snapshot: snapshot_hash.to_string(),
            });
        }
    }

    let dirty = get_dirty_files(guard.repo());
    if !force && !dirty.is_empty() {
        let mut paths: Vec<PathBuf> = dirty.keys().map(PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }
    let discarded = dirty.len();

    let conn = guard.conn();
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
        [snapshot_hash],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(VeloError::not_found(RefKind::Snapshot, snapshot_hash));
    }

    let mut snapshot_files = load_tree(conn, snapshot_hash)?;

    if partial {
        let wanted: Vec<String> = paths.iter().map(|p| crate::db::normalise(p)).collect();
        snapshot_files.retain(|(rel, _, _)| wanted.iter().any(|p| rel.starts_with(p.as_str())));
        if snapshot_files.is_empty() {
            return Ok(Outcome::NoMatchingPaths {
                snapshot: snapshot_hash.to_string(),
            });
        }
    }

    let ghosts_removed = if partial {
        0
    } else {
        remove_ghosts(root, conn, &snapshot_files)?
    };

    write_files(root, &snapshot_files)?;

    let written: Vec<String> = snapshot_files.iter().map(|(p, _, _)| p.clone()).collect();
    crate::commands::invalidate_cache_entries(guard.repo(), &written);

    if partial {
        return Ok(Outcome::RestoredPaths {
            snapshot: snapshot_hash.to_string(),
            files: snapshot_files.len(),
            discarded,
        });
    }

    storage::write_atomic(&root.join(".velo/PARENT"), snapshot_hash.as_bytes())?;
    let message: String = conn
        .query_row(
            "SELECT message FROM snapshots WHERE hash = ?",
            [snapshot_hash],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "(unknown)".into());
    let branch = fs::read_to_string(root.join(".velo/HEAD"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "(unknown)".into());

    Ok(Outcome::Restored {
        snapshot: snapshot_hash.to_string(),
        branch,
        message,
        files: snapshot_files.len(),
        ghosts_removed,
        discarded,
    })
}

/// Delete files the snapshot we're leaving tracked but the target doesn't have.
///
/// Only *tracked* files qualify. Anything on disk belonging to neither snapshot
/// is untracked: it exists in no object, so removing it would be unrecoverable.
/// Git's `--force` discards tracked modifications and never touches untracked
/// files; Velo matches that.
fn remove_ghosts(
    root: &Path,
    conn: &rusqlite::Connection,
    target: &[(String, String, i64)],
) -> Result<usize> {
    let source_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let source_set: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT path FROM file_map WHERE snapshot_hash = ?")?;
        let set = stmt
            .query_map([source_hash.trim()], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        set
    };
    let target_set: HashSet<&str> = target.iter().map(|(p, _, _)| p.as_str()).collect();

    let ghosts: Vec<PathBuf> = get_tracked_files(root)
        .into_iter()
        .filter(|p| {
            let Some(rel) = p
                .strip_prefix(root)
                .ok()
                .and_then(|r| r.to_str())
                .map(crate::db::normalise)
            else {
                return false;
            };
            source_set.contains(&rel) && !target_set.contains(rel.as_str())
        })
        .collect();

    if ghosts.is_empty() {
        return Ok(0);
    }
    let parents: Vec<PathBuf> = ghosts
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    ghosts.par_iter().for_each(|p| {
        let _ = fs::remove_file(p);
    });
    for dir in parents {
        remove_empty_parents(&dir, root);
    }
    Ok(ghosts.len())
}

/// Write every file in parallel, collecting the failures rather than the first.
fn write_files(root: &Path, files: &[(String, String, i64)]) -> Result<()> {
    let objects_dir = root.join(".velo/objects");
    let errors: Vec<String> = files
        .par_iter()
        .filter_map(|(rel_path, hash, mode)| {
            let full_path = root.join(crate::db::db_to_path(rel_path));
            if let Some(parent) = full_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Some(format!("mkdir '{}': {}", rel_path, e));
                }
            }
            match storage::read_object(&objects_dir, hash) {
                Ok(data) => match storage::apply_file(&full_path, *mode, &data) {
                    Ok(_) => None,
                    Err(e) => Some(format!("write '{}': {} (is the file locked?)", rel_path, e)),
                },
                Err(e) => Some(format!("read object for '{}': {}", rel_path, e)),
            }
        })
        .collect();

    if errors.is_empty() {
        return Ok(());
    }
    // Report every failure: a locked file part-way through a restore leaves the
    // tree half-written, and knowing which paths failed is what makes it fixable.
    Err(VeloError::invalid(format!(
        "{} file(s) could not be written:\n  {}",
        errors.len(),
        errors.join("\n  ")
    )))
}

fn load_tree(
    conn: &rusqlite::Connection,
    snapshot_hash: &str,
) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let tree: Vec<(String, String, i64)> = stmt
        .query_map(params![snapshot_hash], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tree)
}
