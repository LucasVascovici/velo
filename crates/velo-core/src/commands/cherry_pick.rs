//! `velo cherry-pick <hash>` — apply the changes from one snapshot onto the
//! current working tree.
//!
//! The snapshot's parent is the common ancestor, the current position is "ours",
//! and the snapshot itself is "theirs", so this is the same three-way
//! reconciliation `merge` performs — see [`crate::commands::apply`].
//!
//! Returns what happened as data; wording lives in `velo-cli`.

use std::fs;

use crate::commands::{apply, apply::Applied, get_dirty_files};
use crate::error::{InProgress, RefKind, Result, VeloError};
use crate::storage;
use crate::{SnapshotId, WriteGuard};

/// What a cherry-pick did.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The snapshot that was picked.
    pub snapshot: SnapshotId,
    /// Its original message.
    pub message: String,
    /// What each file's reconciliation decided.
    pub applied: Applied,
    /// The snapshot the pick was recorded as. `None` when conflicts remain (the
    /// user finishes with `velo save`) or when there was nothing to apply.
    pub saved_as: Option<SnapshotId>,
}

impl Outcome {
    /// Conflicts were left for the user to resolve.
    pub fn is_conflicted(&self) -> bool {
        !self.applied.is_clean()
    }

    /// The snapshot's changes were already present.
    pub fn applied_nothing(&self) -> bool {
        self.applied.applied_nothing()
    }
}

/// Apply `target`'s changes to the working tree, committing when clean.
pub fn run(guard: &WriteGuard, target: &str) -> Result<Outcome> {
    let root = guard.root();
    // Checked before the dirty-tree test: a paused merge or cherry-pick leaves
    // the tree dirty by design, so testing dirtiness first blames the wrong thing.
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
        });
    }

    let dirty = get_dirty_files(guard.repo());
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }

    let snapshot = crate::commands::resolve_snapshot_id(guard.repo(), target)?;
    let conn = guard.conn();

    let (message, parent_hash): (String, String) = conn
        .query_row(
            "SELECT message, parent_hash FROM snapshots WHERE hash = ?",
            [&snapshot],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| VeloError::not_found(RefKind::Snapshot, target))?;

    let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let applied = apply::reconcile_tree(
        guard,
        &apply::load_tree(conn, &parent_hash)?,
        &apply::load_tree(conn, position.trim())?,
        &apply::load_tree(conn, &snapshot)?,
    )?;

    if !applied.is_clean() {
        // MERGE_HEAD's "<pre-hash>:cherry-pick/<snap>" form is what lets
        // `velo merge --abort` unwind a half-applied pick.
        storage::write_atomic(
            &root.join(".velo/MERGE_HEAD"),
            format!("{}:cherry-pick/{}", position.trim(), &snapshot[..8]).as_bytes(),
        )?;
        applied.record_conflicts(conn)?;
        return Ok(Outcome {
            snapshot,
            message,
            applied,
            saved_as: None,
        });
    }

    // A clean pick is committed straight away. Delegating to `save` means
    // auto-merged content gets hashed correctly and there is one snapshot path.
    let commit_message = format!("Cherry-pick {}: {}", &snapshot[..8], message);
    let saved_as = crate::commands::save::run(guard, &commit_message, false)?
        .into_result()
        .map(|r| r.hash);

    Ok(Outcome {
        snapshot,
        message,
        applied,
        saved_as,
    })
}
