//! Working-tree status, as data.
//!
//! Returns a [`Status`] describing where the branch is, how it relates to its
//! remote-tracking ref, and what is unsaved. Rendering lives in `velo-cli`.

use std::fs;

use crate::commands::{get_conflict_files, get_dirty_files, FileStatus};
use crate::error::Result;
use crate::Repo;

/// How the current branch relates to its remote-tracking ref.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tracking {
    /// The branch has no remote-tracking ref.
    Untracked,
    /// Tracked, but the remote's tip isn't in the local object store yet, so the
    /// comparison can't be made without fetching.
    Unfetched { remote: String, branch: String },
    /// Tracked and comparable. `ahead`/`behind` are commit counts relative to the
    /// last-fetched remote state — no network access is involved.
    Known {
        remote: String,
        branch: String,
        ahead: usize,
        behind: usize,
    },
}

impl Tracking {
    /// `<remote>/<branch>`, when there is one.
    pub fn label(&self) -> Option<String> {
        match self {
            Tracking::Untracked => None,
            Tracking::Unfetched { remote, branch } | Tracking::Known { remote, branch, .. } => {
                Some(format!("{}/{}", remote, branch))
            }
        }
    }
}

/// Everything `velo status` reports.
#[derive(Clone, Debug)]
pub struct Status {
    pub branch: String,
    /// The snapshot the working tree is based on; `None` before the first save.
    pub position: Option<String>,
    /// Message of the snapshot at `position`.
    pub position_message: Option<String>,
    pub tracking: Tracking,
    /// Paths with unresolved merge conflicts.
    pub conflicts: Vec<String>,
    /// Files added since `position`, sorted.
    pub new_files: Vec<String>,
    /// Files modified since `position`, sorted.
    pub modified: Vec<String>,
    /// Files deleted since `position`, sorted.
    pub deleted: Vec<String>,
}

impl Status {
    /// Total number of unsaved changes.
    pub fn change_count(&self) -> usize {
        self.new_files.len() + self.modified.len() + self.deleted.len()
    }

    /// Nothing unsaved and no conflicts outstanding.
    pub fn is_clean(&self) -> bool {
        self.change_count() == 0 && self.conflicts.is_empty()
    }
}

/// Collect the working-tree status, optionally restricted to `paths`.
pub fn run(repo: &Repo, paths: &[String]) -> Result<Status> {
    let root = repo.root();
    let branch = fs::read_to_string(root.join(".velo/HEAD"))
        .unwrap_or_else(|_| "main".into())
        .trim()
        .to_string();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let parent_hash = parent_hash.trim().to_string();
    let position = (!parent_hash.is_empty()).then(|| parent_hash.clone());

    let conn = repo.conn();

    let position_message = position.as_ref().and_then(|hash| {
        conn.query_row(
            "SELECT message FROM snapshots WHERE hash = ?",
            [hash],
            |r| r.get::<_, String>(0),
        )
        .ok()
    });

    let tracking = tracking_status(conn, &branch);
    let conflicts = get_conflict_files(repo);

    let dirty = get_dirty_files(repo);
    let dirty = if paths.is_empty() {
        dirty
    } else {
        dirty
            .into_iter()
            .filter(|(p, _)| paths.iter().any(|spec| p.starts_with(spec.as_str())))
            .collect()
    };

    let mut new_files = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    for (path, status) in dirty {
        match status {
            FileStatus::New => new_files.push(path),
            FileStatus::Modified => modified.push(path),
            FileStatus::Deleted => deleted.push(path),
        }
    }
    new_files.sort_unstable();
    modified.sort_unstable();
    deleted.sort_unstable();

    Ok(Status {
        branch,
        position,
        position_message,
        tracking,
        conflicts,
        new_files,
        modified,
        deleted,
    })
}

/// Classify the branch against its remote-tracking ref, using only local data.
fn tracking_status(conn: &rusqlite::Connection, branch: &str) -> Tracking {
    let (remote, remote_tip) = match crate::commands::tracking_ref(conn, branch) {
        Some(x) => x,
        None => return Tracking::Untracked,
    };
    let local_tip = match crate::commands::branch_tip(conn, branch) {
        Some(t) => t,
        None => return Tracking::Untracked,
    };

    // Without the remote's tip locally there is nothing to count against.
    let known: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
            [&remote_tip],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false);
    if !known {
        return Tracking::Unfetched {
            remote,
            branch: branch.to_string(),
        };
    }

    let (ahead, behind) = crate::commands::ahead_behind(conn, &local_tip, &remote_tip);
    Tracking::Known {
        remote,
        branch: branch.to_string(),
        ahead,
        behind,
    }
}
