//! `velo stash` — shelve and restore dirty working-tree state.
//!
//! Unlike Git's stash (a hidden ref with cryptic `stash@{N}` names), Velo's
//! shelves are named explicitly and listed in their own table.
//!
//! ```text
//! velo stash [<name>]      push the current dirty state onto a named shelf
//! velo stash list          list all shelves
//! velo stash pop [<name>]  restore the most recent shelf (or a named one)
//! velo stash drop [<name>] delete a shelf without restoring it
//! velo stash show [<name>] show what a shelf contains
//! ```
//!
//! Returns what happened as data; wording lives in `velo-cli`.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fs;

use rayon::prelude::*;
use rusqlite::params;

use crate::commands::SnapshotIdentity;
use crate::commands::{get_dirty_files, get_tracked_files, FileStatus};
use crate::db;
use crate::error::{RefKind, Result, VeloError};
use crate::progress::Phase;
use crate::storage;
use crate::Repo;
use crate::SnapshotMeta;
use crate::WriteGuard;

/// A shelf, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shelf {
    pub name: String,
    /// Branch the shelf was created on.
    pub branch: String,
    /// Raw stored timestamp_ms; formatting is the consumer's choice.
    pub created_at: DateTime<Utc>,
    /// The hidden snapshot holding the shelved tree.
    pub snapshot: String,
}

/// What `push` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// Nothing was unsaved, so nothing was shelved.
    NothingToStash,
    Shelved {
        name: String,
        modified: usize,
        new: usize,
        deleted: usize,
        /// The snapshot the working tree was returned to. `None` when the branch
        /// had no snapshots, in which case tracked files were simply removed.
        restored_to: Option<String>,
    },
}

/// The shelf was made somewhere other than where it's being applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchMismatch {
    /// Branch the shelf was created on.
    pub shelf: String,
    /// Branch currently checked out.
    pub current: String,
}

/// What `pop` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Popped {
    pub name: String,
    /// Set when the shelf came from a different branch — worth mentioning, not
    /// worth refusing over.
    pub branch_mismatch: Option<BranchMismatch>,
    /// True when the position has moved since the shelf was created.
    pub position_moved: bool,
    /// Files written back onto the working tree.
    pub restored: usize,
    /// Files removed because the shelf recorded them as deleted.
    pub removed: usize,
}

// ─── push ────────────────────────────────────────────────────────────────────

/// Shelve the current dirty state, returning the working tree to its last
/// snapshot.
pub fn push(guard: &WriteGuard, name: Option<String>) -> Result<Pushed> {
    let root = guard.root();
    let dirty = get_dirty_files(guard.repo());
    if dirty.is_empty() {
        return Ok(Pushed::NothingToStash);
    }

    let conn = guard.conn();
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let parent_hash = parent_hash.trim().to_string();

    let shelf_name =
        name.unwrap_or_else(|| format!("stash-{}", Utc::now().format("%Y%m%d-%H%M%S")));

    let taken: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM stash WHERE name = ?)",
        [&shelf_name],
        |r| r.get(0),
    )?;
    if taken {
        return Err(VeloError::invalid(format!(
            "A shelf named '{}' already exists. Use a different name or drop it first.",
            shelf_name
        )));
    }

    // Hash and compress every dirty file that still exists, in parallel.
    let objects_dir = root.join(".velo/objects");
    let to_hash: Vec<String> = dirty
        .iter()
        .filter(|(_, status)| **status != FileStatus::Deleted)
        .map(|(path, _)| path.clone())
        .collect();
    let progress = guard.phase(Phase::Hashing, Some(to_hash.len() as u64));
    let hashed: Vec<(String, String, i64)> = to_hash
        .into_par_iter()
        .inspect(|_| progress.tick())
        .map(|rel| {
            let full = root.join(db::db_to_path(&rel));
            let mode = storage::capture_mode(&full);
            let hash = if mode == storage::MODE_SYMLINK {
                storage::store_raw(&objects_dir, &storage::read_symlink_target(&full)?)?
            } else {
                storage::hash_and_compress(&full, &objects_dir)?
            };
            Ok((rel, hash, mode))
        })
        .collect::<Result<Vec<_>>>()?;

    // The shelf's tree: unchanged files carried from the parent, plus the freshly
    // hashed dirty ones. Deleted files are simply absent, which is what records
    // the deletion.
    let mut tree: Vec<(String, String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
        let carried: Vec<(String, String, i64)> = stmt
            .query_map([&parent_hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .filter_map(|r| r.ok())
            .filter(|(p, _, _): &(String, String, i64)| {
                dirty.get(p.as_str()) != Some(&FileStatus::Deleted)
                    && !hashed.iter().any(|(hp, _, _)| hp == p)
            })
            .collect();
        carried
    };
    tree.extend(hashed.iter().cloned());

    let message = format!("stash: {}", shelf_name);
    let timestamp_ms = crate::commands::snapshot_timestamp_ms();
    let snapshot = crate::commands::snapshot_id(SnapshotIdentity {
        tree: &tree,
        parent: &parent_hash,
        merge_parent: "",
        message: &message,
        timestamp_ms,
        meta: &SnapshotMeta::new(),
    });

    let tx = guard.transaction()?;
    // The shelf's snapshot lives on a hidden '_stash' branch so it never shows
    // up in history.
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, created_at_ms)
         VALUES (?, ?, '_stash', ?, ?)",
        params![snapshot, message, parent_hash, timestamp_ms],
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;
        for (p, h, m) in &tree {
            ins.execute(params![snapshot, p, h, m])?;
        }
    }
    tx.execute(
        "INSERT INTO stash (name, snapshot_hash, branch, parent_hash) VALUES (?, ?, ?, ?)",
        params![shelf_name, snapshot, branch.trim(), parent_hash],
    )?;
    tx.commit()?;

    // Clear the brand-new files just shelved. `restore` deliberately leaves
    // untracked files alone — they exist in no snapshot, so removing them would
    // be unrecoverable — but here they *are* safely stored in the shelf, which is
    // exactly what "shelve my work" means.
    for (rel, status) in &dirty {
        if *status == FileStatus::New {
            let full = root.join(db::db_to_path(rel));
            let _ = fs::remove_file(&full);
            if let Some(parent) = full.parent() {
                crate::commands::remove_empty_parents(parent, root);
            }
        }
    }

    let restored_to = if parent_hash.is_empty() {
        // Nothing to restore to: just clear what was tracked.
        for path in get_tracked_files(root) {
            let _ = fs::remove_file(&path);
        }
        None
    } else {
        crate::commands::restore::run(guard, &parent_hash, true, &[])?;
        Some(parent_hash)
    };

    let count = |want: FileStatus| dirty.values().filter(|s| **s == want).count();
    Ok(Pushed::Shelved {
        name: shelf_name,
        modified: count(FileStatus::Modified),
        new: count(FileStatus::New),
        deleted: count(FileStatus::Deleted),
        restored_to,
    })
}

// ─── list ─────────────────────────────────────────────────────────────────────

/// Every shelf, newest first.
pub fn list(repo: &Repo) -> Result<Vec<Shelf>> {
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT name, branch, created_at_ms, snapshot_hash FROM stash ORDER BY id DESC")?;
    let shelves: Vec<Shelf> = stmt
        .query_map([], |r| {
            Ok(Shelf {
                name: r.get(0)?,
                branch: r.get(1)?,
                created_at: crate::commands::timestamp_from_ms(r.get(2)?),
                snapshot: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(shelves)
}

// ─── pop / drop ───────────────────────────────────────────────────────────────

/// Apply a shelf to the working tree and forget it.
pub fn pop(guard: &WriteGuard, name: Option<String>) -> Result<Popped> {
    let root = guard.root();
    let conn = guard.conn();
    let shelf = find_shelf(conn, name)?;

    let dirty = get_dirty_files(guard.repo());
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }

    let current_branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let current_branch = current_branch.trim().to_string();
    let current_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    let branch_mismatch = (current_branch != shelf.branch).then(|| BranchMismatch {
        shelf: shelf.branch.clone(),
        current: current_branch,
    });
    let position_moved = current_parent.trim() != shelf.parent;

    let (restored, removed) = apply_tree(guard, &shelf)?;

    let tx = guard.transaction()?;
    forget(&tx, &shelf)?;
    tx.commit()?;

    Ok(Popped {
        name: shelf.name,
        branch_mismatch,
        position_moved,
        restored,
        removed,
    })
}

/// Forget a shelf without applying it. Returns the shelf's name.
pub fn drop_shelf(guard: &WriteGuard, name: Option<String>) -> Result<String> {
    let conn = guard.conn();
    let shelf = find_shelf(conn, name)?;
    let tx = guard.transaction()?;
    forget(&tx, &shelf)?;
    tx.commit()?;
    Ok(shelf.name)
}

/// Write a shelf's files onto the working tree and remove what it recorded as
/// deleted. Returns (written, removed).
fn apply_tree(guard: &WriteGuard, shelf: &ShelfRow) -> Result<(usize, usize)> {
    let root = guard.root();
    let conn = guard.conn();
    let objects_dir = root.join(".velo/objects");

    // The mode is read alongside the hash so exec bits and symlinks survive a
    // round trip: this used to `fs::write` the bytes and drop the mode entirely.
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let files: Vec<(String, String, i64)> = stmt
        .query_map([&shelf.snapshot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let progress = guard.phase(Phase::Writing, Some(files.len() as u64));
    let errors: Vec<String> = files
        .par_iter()
        .inspect(|_| progress.tick())
        .filter_map(|(rel, hash, mode)| {
            let full = root.join(db::db_to_path(rel));
            if let Some(parent) = full.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Some(format!("{}: {}", rel, e));
                }
            }
            match storage::read_object(&objects_dir, hash) {
                Ok(data) => storage::apply_file(&full, *mode, &data)
                    .err()
                    .map(|e| format!("{}: {}", rel, e)),
                Err(e) => Some(format!("{}: {}", rel, e)),
            }
        })
        .collect();
    if !errors.is_empty() {
        return Err(VeloError::invalid(format!(
            "{} file(s) could not be restored from the shelf: {}",
            errors.len(),
            errors.join("; ")
        )));
    }

    // A file the shelf's parent tracked but the shelf itself omits was deleted
    // before shelving, so popping has to delete it again. Without this the
    // deletion was silently lost: pop only ever wrote files.
    let shelved: HashMap<&str, ()> = files.iter().map(|(p, _, _)| (p.as_str(), ())).collect();
    let mut stmt = conn.prepare("SELECT path FROM file_map WHERE snapshot_hash = ?")?;
    let parent_paths: Vec<String> = stmt
        .query_map([&shelf.parent], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut removed = 0usize;
    for path in parent_paths {
        if shelved.contains_key(path.as_str()) {
            continue;
        }
        let full = root.join(db::db_to_path(&path));
        if full.exists() && fs::remove_file(&full).is_ok() {
            removed += 1;
            if let Some(parent) = full.parent() {
                crate::commands::remove_empty_parents(parent, root);
            }
        }
    }

    Ok((files.len(), removed))
}

/// Delete a shelf's registration and the hidden snapshot behind it.
fn forget(tx: &rusqlite::Transaction, shelf: &ShelfRow) -> Result<()> {
    tx.execute("DELETE FROM stash WHERE name = ?", [&shelf.name])?;
    tx.execute(
        "DELETE FROM file_map WHERE snapshot_hash = ?",
        [&shelf.snapshot],
    )?;
    tx.execute("DELETE FROM snapshots WHERE hash = ?", [&shelf.snapshot])?;
    Ok(())
}

/// A shelf's stored row.
struct ShelfRow {
    name: String,
    snapshot: String,
    branch: String,
    parent: String,
}

/// Look up a shelf by name, or take the most recent one.
fn find_shelf(conn: &rusqlite::Connection, name: Option<String>) -> Result<ShelfRow> {
    let row = |r: &rusqlite::Row| {
        Ok(ShelfRow {
            name: r.get(0)?,
            snapshot: r.get(1)?,
            branch: r.get(2)?,
            parent: r.get(3)?,
        })
    };
    match name {
        Some(n) => conn
            .query_row(
                "SELECT name, snapshot_hash, branch, parent_hash FROM stash WHERE name = ?",
                [&n],
                row,
            )
            .map_err(|_| VeloError::not_found(RefKind::Stash, &n)),
        None => conn
            .query_row(
                "SELECT name, snapshot_hash, branch, parent_hash FROM stash
                 ORDER BY id DESC LIMIT 1",
                [],
                row,
            )
            .map_err(|_| VeloError::invalid("No shelves found.")),
    }
}

// ─── show ─────────────────────────────────────────────────────────────────────

/// A shelf's identity plus the changes it holds.
#[derive(Clone, Debug)]
pub struct ShelfDetail {
    pub name: String,
    /// Branch the shelf was created on.
    pub branch: String,
    /// Snapshot the shelf was taken against.
    pub parent: String,
    pub diff: crate::commands::diff::Diff,
}

/// Describe what a shelf contains, as a diff against the snapshot it was taken
/// from.
pub fn show_shelf(repo: &Repo, name: Option<String>) -> Result<ShelfDetail> {
    let conn = repo.conn();
    let shelf = find_shelf(conn, name)?;
    let diff =
        crate::commands::diff::snapshot_diff(repo, conn, &shelf.parent, &shelf.snapshot, &None)?;

    Ok(ShelfDetail {
        name: shelf.name,
        branch: shelf.branch,
        parent: shelf.parent,
        diff,
    })
}
