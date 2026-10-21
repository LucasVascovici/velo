//! `velo undo` — remove the newest snapshot on this branch, keeping it recoverable.
//!
//! The snapshot is moved to the trash rather than deleted, along with any tags
//! pointing at it, so `velo redo` can put it back. Returns what happened as data;
//! wording lives in `velo-cli`.

use std::fs;

use rusqlite::OptionalExtension;

use crate::commands::dirty_scan;
use crate::commands::require_working_tree;
use crate::error::{InProgress, Result, VeloError};
use crate::events::{Event, Ref};
use crate::SnapshotId;
use crate::WriteGuard;

/// What an undo did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The snapshot that was shelved.
    pub snapshot: String,
    /// Its message.
    pub message: String,
    /// Where the branch now sits. `None` when the root snapshot was undone, in
    /// which case the working tree was emptied of tracked files.
    pub now_at: Option<String>,
}

impl Outcome {
    /// True when undoing removed the branch's only snapshot.
    pub fn cleared_working_tree(&self) -> bool {
        self.now_at.is_none()
    }
}

/// Refuse when another live snapshot, on any branch, builds on `snapshot`.
///
/// Shelving it would leave those snapshots pointing at a parent that is no
/// longer in the live set, which `fsck` reports as damage. The cheap
/// direct-children probe runs first; the descendant count is only paid for on
/// the refusal path. Internal labels (`_stash`, `_deleted_<name>`) count too,
/// because `fsck` checks every row.
fn refuse_if_depended_on(conn: &rusqlite::Connection, snapshot: &str) -> Result<()> {
    let has_child: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM snapshots
         WHERE hash <> ?1 AND (parent_hash = ?1 OR COALESCE(merge_parent,'') = ?1))",
        [snapshot],
        |r| r.get(0),
    )?;
    if !has_child {
        return Ok(());
    }
    // UNION (not UNION ALL) so a hand-edited cycle terminates; the depth bound
    // matches `commands::ancestors`.
    let mut stmt = conn.prepare(
        "WITH RECURSIVE down(hash, depth) AS (
             SELECT hash, 1 FROM snapshots
              WHERE hash <> ?1 AND (parent_hash = ?1 OR COALESCE(merge_parent,'') = ?1)
             UNION
             SELECT s.hash, down.depth + 1 FROM snapshots s, down
              WHERE s.hash <> ?1
                AND (s.parent_hash = down.hash OR COALESCE(s.merge_parent,'') = down.hash)
                AND down.depth < ?2
         )
         SELECT s.branch, COUNT(*) FROM snapshots s
          WHERE s.hash IN (SELECT hash FROM down)
          GROUP BY s.branch ORDER BY s.branch",
    )?;
    let branches: Vec<(crate::BranchName, usize)> = stmt
        .query_map(
            rusqlite::params![snapshot, crate::commands::MAX_ANCESTRY_DEPTH],
            |r| {
                Ok((
                    crate::BranchName::from_stored(r.get::<_, String>(0)?),
                    r.get::<_, i64>(1)? as usize,
                ))
            },
        )?
        .collect::<std::result::Result<_, _>>()?;
    let dependents = branches.iter().map(|(_, n)| n).sum();
    Err(VeloError::SnapshotHasDependents {
        snapshot: SnapshotId::from_stored(snapshot),
        branches,
        dependents,
    })
}

/// Shelve the newest snapshot on the current branch.
pub fn run(guard: &WriteGuard) -> Result<Outcome> {
    require_working_tree(guard.repo(), "undo")?;
    let root = guard.root();
    // A merge or rebase leaves MERGE_HEAD / REBASE_STATE and conflict rows
    // behind; removing the tip underneath them produces an inconsistent
    // repository. Checked before dirtiness, since both leave the tree dirty.
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
        });
    }
    if root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Rebase,
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

    let snap: Option<(String, String, String)> = conn
        .query_row(
            "SELECT hash, message, parent_hash
             FROM snapshots
             WHERE branch = ?
             ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (snapshot, message, parent_hash) = snap.ok_or_else(|| VeloError::NothingToUndo {
        branch: branch.trim().to_string(),
    })?;

    refuse_if_depended_on(conn, &snapshot)?;

    let now_at = parent_hash.trim().to_string();

    // The tree and position move first, the database last. A kill in between
    // leaves the snapshot still live, the position on its parent and a matching
    // tree: clean, consistent, and `undo` simply runs again. The other order
    // left `.velo/PARENT` naming a snapshot already in the trash, which `fsck`
    // reports as a dangling position.
    if now_at.is_empty() {
        // The root snapshot goes, so there is nothing to restore to: clear the
        // position and remove the files it tracked.
        crate::failpoint::hit("undo.before_parent");
        crate::storage::write_atomic(&root.join(".velo/PARENT"), b"")?;
        crate::failpoint::hit("undo.after_parent");
        for path in crate::commands::get_tracked_files(root, guard.repo().scope()) {
            let _ = fs::remove_file(&path);
        }
    } else {
        // restore::run writes PARENT itself.
        crate::commands::restore::run_with(
            guard,
            &SnapshotId::from_stored(now_at.as_str()),
            crate::commands::restore::Options {
                force: true,
                ..Default::default()
            },
            crate::commands::restore::Strategy::Delta,
            Some(scan),
        )?;
    }

    // One transaction: the snapshot and its tags move together, and file_map is
    // deliberately left in place so redo can restore the tree.
    let tx = guard.transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO trash (hash, message, branch, parent_hash, merge_parent, created_at_ms, deleted_at_ms)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms, ?
         FROM snapshots WHERE hash = ?",
        rusqlite::params![guard.repo().now_ms(), snapshot],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO trash_tags (name, snapshot_hash)
         SELECT name, snapshot_hash FROM tags WHERE snapshot_hash = ?",
        [&snapshot],
    )?;
    tx.execute("DELETE FROM tags WHERE snapshot_hash = ?", [&snapshot])?;
    tx.execute("DELETE FROM snapshots WHERE hash = ?", [&snapshot])?;
    crate::failpoint::hit("undo.before_commit");
    tx.commit()?;
    crate::failpoint::hit("undo.after_commit");

    guard.repo().emit(Event::RefMoved {
        reference: Ref::Branch(crate::BranchName::from_stored(branch.trim())),
        from: Some(SnapshotId::from_stored(snapshot.as_str())),
        to: (!now_at.is_empty()).then(|| SnapshotId::from_stored(now_at.as_str())),
    });
    Ok(Outcome {
        snapshot,
        message,
        now_at: (!now_at.is_empty()).then_some(now_at),
    })
}
