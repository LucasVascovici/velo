//! `velo redo` — restore the most recently undone snapshot on this branch.
//!
//! Moves the snapshot back out of the trash, along with any tags `undo` shelved
//! with it. Returns what happened as data; wording lives in `velo-cli`.

use std::fs;

use rusqlite::OptionalExtension;

use crate::commands::dirty_scan;
use crate::commands::require_working_tree;
use crate::error::{InProgress, Result, VeloError};
use crate::SnapshotId;
use crate::WriteGuard;

/// What a redo did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The snapshot that was brought back, and where the branch now sits.
    pub snapshot: String,
    /// Its message.
    pub message: String,
}

/// The newest trashed snapshot on `branch`: hash, message and parent hash.
fn newest_trashed(
    conn: &rusqlite::Connection,
    branch: &str,
) -> Result<Option<(String, String, String)>> {
    Ok(conn
        .query_row(
            "SELECT hash, message, COALESCE(parent_hash, '') FROM trash
             WHERE branch = ? ORDER BY deleted_at_ms DESC LIMIT 1",
            [branch],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?)
}

/// Whether the branch is still where `undo` left it. Returns the branch's
/// current tip when it is not, so the caller can name it.
///
/// A redo re-inserts the trashed row as the branch's newest snapshot, which
/// is only coherent while the row's parent is the tip. Three cases are
/// coherent: the branch has no snapshot at all (the undone one was its first,
/// so nothing can have moved), the tip is the row's parent, or the tip is the
/// row itself (a fetch brought the same snapshot back). A root snapshot (empty
/// parent) matches no tip: it can only come back to a branch with no commits.
///
/// One more case is a repair rather than a rewind: the trashed row is missing
/// from `snapshots` while live snapshots still name it as a parent (it was
/// undone out from under them, see `undone_parent_recovery`). Bringing it back
/// completes their history whatever the branch tip is.
fn moved_tip(
    conn: &rusqlite::Connection,
    branch: &str,
    snapshot: &str,
    parent: &str,
) -> Option<Option<String>> {
    let tip = crate::commands::branch_tip(conn, branch);
    let unchanged = match &tip {
        Some(t) => t == snapshot || (!parent.is_empty() && t == parent),
        None => true,
    };
    if unchanged || is_needed_parent(conn, snapshot) {
        None
    } else {
        Some(tip)
    }
}

/// Whether live snapshots build on `snapshot` while it is absent from them.
fn is_needed_parent(conn: &rusqlite::Connection, snapshot: &str) -> bool {
    conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM snapshots WHERE hash = ?1)
            AND EXISTS (SELECT 1 FROM snapshots WHERE parent_hash = ?1 OR merge_parent = ?1)",
        [snapshot],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// What `run` would restore on the current branch, without changing anything.
///
/// Uses the same query as `run`, so a caller can tell the user "your last undo
/// removed X" and be right about what `redo` would bring back. Needs no write
/// lock: it only reads the trash. Returns `None` when `run` would refuse with
/// `RedoBranchMoved`, because then nothing would be brought back.
pub fn peek(repo: &crate::Repo) -> Result<Option<Outcome>> {
    let branch = repo.head_branch().into_string();
    let branch = branch.trim();
    let Some((snapshot, message, parent)) = newest_trashed(repo.conn(), branch)? else {
        return Ok(None);
    };
    if moved_tip(repo.conn(), branch, &snapshot, &parent).is_some() {
        return Ok(None);
    }
    Ok(Some(Outcome { snapshot, message }))
}

/// Restore the most recently undone snapshot on the current branch.
pub fn run(guard: &WriteGuard) -> Result<Outcome> {
    require_working_tree(guard.repo(), "redo")?;
    let root = guard.root();
    // Checked before dirtiness: a merge leaves the tree dirty by design, so
    // testing dirtiness first would blame the wrong thing.
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
        });
    }

    let scan = dirty_scan(guard.repo());
    let dirty = &scan.status;
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }

    let conn = guard.conn();
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());

    let snap = newest_trashed(conn, branch.trim())?;
    let (snapshot, message, parent) = snap.ok_or_else(|| {
        VeloError::invalid(format!("Nothing to redo on branch '{}'.", branch.trim()))
    })?;
    // Refused before any write: bringing the row back while the branch has
    // moved on would leave it as a non-tip whose child is the tip, and the
    // restore would then put the tree and PARENT on a snapshot `branches`
    // does not list.
    if let Some(tip) = moved_tip(conn, branch.trim(), &snapshot, &parent) {
        return Err(VeloError::RedoBranchMoved {
            branch: branch.trim().to_string(),
            snapshot: SnapshotId::from_stored(snapshot.as_str()),
            tip: tip.map(|t| SnapshotId::from_stored(t.as_str())),
        });
    }

    let tx = guard.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms
         FROM trash WHERE hash = ?
         ON CONFLICT(hash) DO UPDATE SET branch = excluded.branch",
        [&snapshot],
    )?;
    tx.execute("DELETE FROM trash WHERE hash = ?", [&snapshot])?;
    tx.execute(
        "INSERT OR REPLACE INTO tags (name, snapshot_hash)
         SELECT name, snapshot_hash FROM trash_tags WHERE snapshot_hash = ?",
        [&snapshot],
    )?;
    tx.execute(
        "DELETE FROM trash_tags WHERE snapshot_hash = ?",
        [&snapshot],
    )?;
    crate::failpoint::hit("redo.before_commit");
    tx.commit()?;
    crate::failpoint::hit("redo.after_commit");
    // The row came back from the trash, so it is a snapshot the repository did
    // not have a moment ago. Its branch and parents are read from the row itself.
    let row: Option<(String, Option<String>, Option<String>)> = guard
        .conn()
        .query_row(
            "SELECT branch, parent_hash, merge_parent FROM snapshots WHERE hash = ?",
            [&snapshot],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((snap_branch, parent, merge_parent)) = row {
        guard.repo().emit_saved_raw(
            &snapshot,
            &snap_branch,
            parent.as_deref().unwrap_or(""),
            merge_parent.as_deref().unwrap_or(""),
        );
    }

    // restore::run writes PARENT itself.
    crate::commands::restore::run_with(
        guard,
        &SnapshotId::from_stored(snapshot.as_str()),
        crate::commands::restore::Options {
            force: true,
            ..Default::default()
        },
        crate::commands::restore::Strategy::Delta,
        Some(scan),
    )?;
    Ok(Outcome { snapshot, message })
}
