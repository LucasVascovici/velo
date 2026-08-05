//! `velo show <target>` — inspect a snapshot without restoring it.
//!
//! Returns the snapshot's metadata together with its diff against its parent.
//! The diff model itself lives in [`crate::commands::diff`], so `show`, `diff`
//! and `stash show` all describe changes the same way.

use crate::commands::diff::{self, Diff};
use crate::error::{RefKind, Result, VeloError};
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
pub fn run(repo: &Repo, target: &str, file_filter: &Option<String>) -> Result<SnapshotDetail> {
    let conn = repo.conn();
    let hash = crate::commands::resolve_snapshot_id(repo, target)?;

    let (message, branch, parent_hash, created_at_ms): (String, String, String, i64) = conn
        .query_row(
            "SELECT message, branch, parent_hash, created_at_ms FROM snapshots WHERE hash = ?",
            [&hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| VeloError::not_found(RefKind::Snapshot, target))?;

    let diff = diff::snapshot_diff(repo, conn, &parent_hash, &hash, file_filter)?;

    Ok(SnapshotDetail {
        hash,
        branch,
        parent: (!parent_hash.is_empty()).then_some(parent_hash),
        created_at: crate::commands::timestamp_from_ms(created_at_ms),
        message,
        diff,
    })
}
