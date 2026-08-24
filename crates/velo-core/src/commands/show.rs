//! `velo show <target>` — inspect a snapshot without restoring it.
//!
//! Returns the snapshot's metadata together with its diff against its parent.
//! The diff model itself lives in [`crate::commands::diff`], so `show`, `diff`
//! and `stash show` all describe changes the same way.

use crate::commands::diff::{self, Diff};
use crate::error::{RefKind, Result, VeloError};
use std::path::Path;

use crate::{BranchName, Repo, SnapshotId};
use chrono::{DateTime, Utc};

/// A snapshot's metadata plus what it changed.
#[derive(Clone, Debug)]
pub struct SnapshotDetail {
    /// Full snapshot hash.
    pub hash: SnapshotId,
    /// Branch the snapshot was recorded on. Display only — it is not part of the
    /// snapshot's identity.
    pub branch: BranchName,
    /// First parent; `None` for the root snapshot.
    pub parent: Option<SnapshotId>,
    /// Second parent, set on merge snapshots.
    pub merge_parent: Option<SnapshotId>,
    /// Paths this snapshot moved, as `(from, to)`.
    ///
    /// The same edges the diff folds into [`FileChange::Renamed`], listed
    /// whole so a consumer can report the moves without walking the file list.
    pub renames: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    /// Raw stored timestamp; formatting is the consumer's choice.
    pub created_at: DateTime<Utc>,
    pub message: String,
    /// Changes relative to `parent`, or every file when there is no parent.
    pub diff: Diff,
}

/// Look up `target` (hash, prefix, tag, or branch) and describe it.
/// Everything about one snapshot, including its diff against its parent.
///
/// Takes a resolved id: a caller holding one should not have to format it back
/// into text so this can resolve it again. Resolve a user's spec with
/// [`resolve_snapshot_id`](crate::commands::resolve_snapshot_id) first.
///
/// `paths` narrows the diff; empty means the whole snapshot.
pub fn run(repo: &Repo, hash: &SnapshotId, paths: &[&Path]) -> Result<SnapshotDetail> {
    let conn = repo.conn();
    let hash = hash.clone();

    let (message, branch, parent_hash, merge_parent, created_at_ms): (
        String,
        String,
        String,
        String,
        i64,
    ) = conn
        .query_row(
            "SELECT message, branch, parent_hash, merge_parent, created_at_ms
             FROM snapshots WHERE hash = ?",
            [&hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|_| VeloError::not_found(RefKind::Snapshot, hash.as_str()))?;

    let filter = paths
        .first()
        .map(|p| crate::db::normalise(&p.to_string_lossy()));
    let diff = diff::snapshot_diff(repo, conn, &parent_hash, &hash, &filter)?;

    let renames = crate::commands::paths::recorded_by(repo, &hash)?;

    Ok(SnapshotDetail {
        hash,
        branch: BranchName::from_stored(branch),
        parent: (!parent_hash.is_empty()).then(|| SnapshotId::from_stored(parent_hash)),
        merge_parent: (!merge_parent.is_empty()).then(|| SnapshotId::from_stored(merge_parent)),
        renames,
        created_at: crate::commands::timestamp_from_ms(created_at_ms),
        message,
        diff,
    })
}
