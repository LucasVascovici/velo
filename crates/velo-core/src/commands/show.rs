//! `velo show <target>` — inspect a snapshot without restoring it.
//!
//! Returns the snapshot's metadata together with its diff against its parent.
//! The diff model itself lives in [`crate::commands::diff`], so `show`, `diff`
//! and `stash show` all describe changes the same way.

use crate::commands::diff::{self, Diff};
use crate::error::{RefKind, Result, VeloError};
use std::path::Path;

use crate::{Repo, SnapshotId};
use chrono::{DateTime, Utc};

/// A snapshot's metadata plus what it changed.
#[derive(Clone, Debug)]
pub struct SnapshotDetail {
    /// Full snapshot hash.
    pub hash: SnapshotId,
    /// Branch the snapshot was recorded on. Display only — it is not part of the
    /// snapshot's identity.
    pub branch: String,
    /// `None` for the root snapshot.
    pub parent: Option<String>,
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

    let (message, branch, parent_hash, created_at_ms): (String, String, String, i64) = conn
        .query_row(
            "SELECT message, branch, parent_hash, created_at_ms FROM snapshots WHERE hash = ?",
            [&hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| VeloError::not_found(RefKind::Snapshot, hash.as_str()))?;

    let filter = paths
        .first()
        .map(|p| crate::db::normalise(&p.to_string_lossy()));
    let diff = diff::snapshot_diff(repo, conn, &parent_hash, &hash, &filter)?;

    Ok(SnapshotDetail {
        hash,
        branch,
        parent: (!parent_hash.is_empty()).then_some(parent_hash),
        created_at: crate::commands::timestamp_from_ms(created_at_ms),
        message,
        diff,
    })
}
