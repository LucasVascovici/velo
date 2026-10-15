//! `velo restore` — put the working tree back to a snapshot.
//!
//! Returns what it did as data. This matters more here than elsewhere: eleven
//! other commands call `run` as a step of their own work (merge, switch, undo,
//! redo, rebase, stash, sync), and while it printed, all of them leaked its
//! progress lines into the middle of their own output.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::params;

use crate::commands::require_working_tree;
use crate::commands::{dirty_scan, remove_empty_parents, DirtyScan};
use crate::error::{RefKind, Result, VeloError};
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::storage;
use crate::SnapshotId;
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
        /// Files actually written from the object store. Paths already equal
        /// to the target (same hash and mode, clean on disk) are not rewritten
        /// and not counted.
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
/// How to restore.
///
/// The observer and cancellation token live here rather than on the `Repo`
/// because this project's own anti-goals say so: *"Don't use globals for progress
/// or cancellation. Pass them per call."* An observer set once on the handle
/// fires for every operation, so a caller wanting a bar for one restore gets
/// callbacks from everything with no way to tell which is which.
#[derive(Default)]
pub struct Options<'a> {
    /// Overwrite files with unsaved changes instead of refusing.
    pub force: bool,
    /// Restore only these paths; empty means the whole snapshot.
    pub paths: &'a [&'a Path],
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between files. Cancelling leaves already-written files in place —
    /// the same position a killed process would.
    pub cancel: Option<&'a Cancel>,
}

/// How a full restore decides which files to write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Strategy {
    /// Write only paths that differ from the tree being left, are dirty, or
    /// whose mode on disk is not the target's.
    Delta,
    /// Write every file of the target. The reference behaviour, kept reachable
    /// so the property test can hold `Delta` to it.
    RewriteAll,
}

/// Put the working tree back to how it was at `snapshot`.
pub fn run(guard: &WriteGuard, snapshot: &SnapshotId, options: Options<'_>) -> Result<Outcome> {
    run_with(guard, snapshot, options, Strategy::Delta, None)
}

/// [`run`] with the write strategy chosen and, optionally, the dirty set the
/// caller has already computed (it must describe the working tree as it is now).
pub(crate) fn run_with(
    guard: &WriteGuard,
    snapshot: &SnapshotId,
    options: Options<'_>,
    strategy: Strategy,
    scan: Option<DirtyScan>,
) -> Result<Outcome> {
    crate::timing::begin();
    require_working_tree(guard.repo(), "restore")?;
    let Options {
        force,
        paths,
        observer,
        cancel,
    } = options;
    let snapshot_hash = snapshot.as_str();
    // Retained as owned strings: the matching below and the outcome both want
    // them, and a restore is not a hot loop.
    let paths: Vec<String> = paths
        .iter()
        .map(|p| crate::db::normalise(&p.to_string_lossy()))
        .collect();
    let paths = paths.as_slice();
    let root = guard.root();
    let partial = !paths.is_empty();

    // One scan serves the already-there check, the dirty check and the delta.
    let DirtyScan {
        status: dirty,
        modes: walked,
        ..
    } = scan.unwrap_or_else(|| dirty_scan(guard.repo()));
    crate::timing::lap("restore.scan");

    // Nothing to do when a full restore targets where we already are.
    if !partial {
        let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        if position.trim() == snapshot_hash && dirty.is_empty() {
            return Ok(Outcome::AlreadyThere {
                snapshot: snapshot_hash.to_string(),
            });
        }
    }

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
    crate::timing::lap("restore.target_tree");

    if partial {
        let wanted: Vec<String> = paths.iter().map(|p| crate::db::normalise(p)).collect();
        snapshot_files.retain(|(rel, _, _)| wanted.iter().any(|p| rel.starts_with(p.as_str())));
        if snapshot_files.is_empty() {
            return Ok(Outcome::NoMatchingPaths {
                snapshot: snapshot_hash.to_string(),
            });
        }
    }

    // Delta: the working tree is known to equal the tree at PARENT except for
    // the paths in `dirty`. A tracked path the scan's walk did not see (out of
    // scope, newly ignored, replaced by a directory) stays in its `db_files`
    // and is reported `Deleted`, so it is in `dirty` and gets written. A chmod
    // does not change the hash and is not dirty, hence the mode check.
    let to_write: Vec<(String, String, i64)> = if partial || strategy == Strategy::RewriteAll {
        snapshot_files.clone()
    } else {
        let source: HashMap<(String, String), i64> = {
            let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
            load_tree(conn, position.trim())?
                .into_iter()
                .map(|(p, h, m)| ((p, h), m))
                .collect()
        };
        snapshot_files
            .iter()
            .filter(|(rel, hash, mode)| {
                let same_in_source = source
                    .get(&(rel.clone(), hash.clone()))
                    .is_some_and(|m| m == mode);
                !(same_in_source
                    && !dirty.contains_key(rel)
                    && mode_matches(
                        walked.get(rel).copied().unwrap_or_else(|| {
                            storage::capture_mode(&root.join(crate::db::db_to_path(rel)))
                        }),
                        *mode,
                    ))
            })
            .cloned()
            .collect()
    };

    crate::timing::lap("restore.diff");
    let ghosts = if partial {
        Vec::new()
    } else {
        ghost_paths(root, conn, &snapshot_files, &walked)?
    };

    crate::timing::lap("restore.ghosts");

    // The whole target is validated before the first ghost is removed or file
    // written: a refusal leaves the working tree exactly as it was.
    {
        let objects = guard.repo().objects();
        // A ghost that is a symlink is gone by the time its directory is
        // written, so it must not make the paths below it unsafe.
        let ghosts = &ghosts;
        let replaced: HashSet<String> = ghosts.iter().cloned().collect();
        crate::safe_path::validate_write_set_replacing(
            root,
            snapshot_files.iter().map(|(p, _, _)| p.as_str()),
            to_write
                .iter()
                .map(|(p, h, m)| (p.as_str(), h.as_str(), *m)),
            |h| objects.get(h),
            &replaced,
        )?;
    }
    crate::timing::lap("restore.validate");

    crate::failpoint::hit("restore.before_write");
    let ghosts_removed = remove_ghosts(root, &ghosts);

    {
        let progress = PhaseGuard::new(
            observer.unwrap_or_else(|| guard.repo().observer()),
            Phase::Writing,
            Some(to_write.len() as u64),
        );
        write_files(
            guard,
            root,
            &guard.repo().objects(),
            &to_write,
            &progress,
            cancel,
        )?;
    }

    if partial {
        return Ok(Outcome::RestoredPaths {
            snapshot: snapshot_hash.to_string(),
            files: snapshot_files.len(),
            discarded,
        });
    }

    // A whole-tree rewrite lands a tree that no pending move describes, so the
    // moves are forgotten rather than attached to whatever gets saved next.
    crate::commands::mv::discard_pending(guard.conn())?;
    crate::failpoint::hit("restore.before_parent");
    storage::write_atomic(&root.join(".velo/PARENT"), snapshot_hash.as_bytes())?;
    crate::timing::lap("restore.parent_file");
    crate::failpoint::hit("restore.after_parent");
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
        files: to_write.len(),
        ghosts_removed,
        discarded,
    })
}

/// Whether a file's mode on disk already is the target mode. Where the
/// executable bit is not observable the two regular modes count as equal, the
/// same sticky-exec rule `save` uses.
fn mode_matches(on_disk: i64, target: i64) -> bool {
    if on_disk == target {
        return true;
    }
    !cfg!(unix)
        && matches!(on_disk, storage::MODE_REGULAR | storage::MODE_EXEC)
        && matches!(target, storage::MODE_REGULAR | storage::MODE_EXEC)
}

/// The files the snapshot we're leaving tracked and the target doesn't have.
///
/// Only *tracked* files qualify. Anything on disk belonging to neither snapshot
/// is untracked: it exists in no object, so removing it would be unrecoverable.
/// Git's `--force` discards tracked modifications and never touches untracked
/// files; Velo matches that.
fn ghost_paths(
    root: &Path,
    conn: &rusqlite::Connection,
    target: &[(String, String, i64)],
    walked: &HashMap<String, i64>,
) -> Result<Vec<String>> {
    let source_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let target_set: HashSet<&str> = target.iter().map(|(p, _, _)| p.as_str()).collect();
    // The candidates come from the database (tracked at the left tree, absent
    // from the target) and are then kept only if the dirty scan's walk saw them
    // on disk. That walk skips ignored and out-of-scope paths exactly as the
    // tracked-file walk this replaces did, so an ignored or untracked file is
    // still never a ghost, and no second directory walk is paid for.
    let mut stmt = conn.prepare("SELECT path FROM file_map WHERE snapshot_hash = ?")?;
    let ghosts: Vec<String> = stmt
        .query_map([source_hash.trim()], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .filter(|rel| walked.contains_key(rel) && !target_set.contains(rel.as_str()))
        .collect();
    Ok(ghosts)
}

/// Delete the ghosts found by [`ghost_paths`] and any directory they leave empty.
fn remove_ghosts(root: &Path, ghosts: &[String]) -> usize {
    if ghosts.is_empty() {
        return 0;
    }
    let ghosts: Vec<PathBuf> = ghosts
        .iter()
        .map(|rel| root.join(crate::db::db_to_path(rel)))
        .collect();
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
    ghosts.len()
}

/// One restored file's `index_cache` row: path, mtime (ns), size, hash.
type CacheRow = (String, i64, i64, String);

/// Write every file in parallel, collecting the failures rather than the first.
///
/// Each file that lands is stat-ed and its `index_cache` row is written, so the
/// next dirty check is a cache hit instead of a re-hash of the whole tree. The
/// seeded hash is the object name: `storage::apply_file` writes exactly the
/// object's normalised content (for a symlink, the target string), and
/// `storage::hash_for` of those bytes is that same name. The usual caveat of the
/// cache applies: an edit within one mtime tick that keeps the size is missed.
///
/// On error or cancellation the rows of files that were written are still
/// seeded, and the rows of every other path in the set are invalidated, so no
/// stale row can vouch for content that is no longer on disk.
fn write_files(
    guard: &crate::repo::WriteGuard<'_>,
    root: &Path,
    objects: &crate::storage::ObjectStore,
    files: &[(String, String, i64)],
    progress: &PhaseGuard<'_>,
    cancel: Option<&Cancel>,
) -> Result<()> {
    // Only the first file to land reports the crash point; the workers race, so
    // "first" means first in time, not first in the list.
    let first_written = std::sync::atomic::AtomicBool::new(false);
    let results: Vec<std::result::Result<CacheRow, String>> = objects.par_with_content(
        files,
        |(_, hash, _)| hash.as_str(),
        |(rel_path, hash, mode), content| {
            progress.tick();
            // Checked per file, so cancelling takes effect at the next one and
            // never part-way through writing a file. Workers already in flight
            // finish what they are holding.
            if cancel.is_some_and(Cancel::is_cancelled) {
                return None;
            }
            // Validated as a set before the first write; joined again through
            // the checked path in case the directory changed in between.
            let full_path = match crate::safe_path::join_checked(root, rel_path) {
                Ok(p) => p,
                Err(e) => return Some(Err(e.to_string())),
            };
            if let Some(parent) = full_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Some(Err(format!("mkdir '{}': {}", rel_path, e)));
                }
            }
            match content {
                Ok(data) => match storage::apply_file(&full_path, *mode, &data) {
                    Ok(_) => {
                        if !first_written.swap(true, std::sync::atomic::Ordering::SeqCst) {
                            crate::failpoint::hit("restore.after_first_file");
                        }
                        // A file that cannot be stat-ed is simply not seeded.
                        let meta = fs::symlink_metadata(&full_path).ok()?;
                        let mtime_ns = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos() as i64)
                            .unwrap_or(0);
                        Some(Ok((
                            rel_path.clone(),
                            mtime_ns,
                            meta.len() as i64,
                            hash.clone(),
                        )))
                    }
                    Err(e) => Some(Err(format!(
                        "write '{}': {} (is the file locked?)",
                        rel_path, e
                    ))),
                },
                Err(e) => Some(Err(format!("read object for '{}': {}", rel_path, e))),
            }
        },
    );

    crate::timing::lap("restore.write_files");
    let mut rows: Vec<CacheRow> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for r in results {
        match r {
            Ok(row) => rows.push(row),
            Err(e) => errors.push(e),
        }
    }
    crate::commands::seed_cache_entries(guard.repo(), &rows);
    if rows.len() != files.len() {
        let seeded: std::collections::HashSet<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let unwritten: Vec<String> = files
            .iter()
            .filter(|(p, _, _)| !seeded.contains(p.as_str()))
            .map(|(p, _, _)| p.clone())
            .collect();
        crate::commands::invalidate_cache_entries(guard.repo(), &unwritten);
    }
    crate::timing::lap("restore.cache_seed");

    // Reported before any write error: a cancelled restore skipped files rather
    // than failing on them, so "you asked me to stop" is the truthful answer.
    Cancel::check(cancel)?;

    if errors.is_empty() {
        return Ok(());
    }
    // Report every failure: a locked file part-way through a restore leaves the
    // tree half-written, and knowing which paths failed is what makes it fixable.
    Err(VeloError::invalid(format!(
        "{} file(s) could not be written:
  {}",
        errors.len(),
        errors.join(
            "
  "
        )
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
