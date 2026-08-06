//! `velo branches` — list branches, or delete one.
//!
//! Listing and deleting were one function taking an `Option<String>`; they are
//! separate entry points now, since one reads and one writes. Formatting lives
//! in `velo-cli`.

use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;

use rusqlite::params;

use crate::error::{RefKind, Result, VeloError};
use crate::{BranchName, Repo, SnapshotId, WriteGuard};

/// The snapshot a branch points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    pub hash: SnapshotId,
    pub message: String,
    /// Raw stored timestamp; formatting is the consumer's choice.
    pub created_at: DateTime<Utc>,
}

/// One branch and where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: BranchName,
    /// True for the checked-out branch.
    pub is_current: bool,
    /// `None` only when the tip snapshot is missing. A branch with no commits of
    /// its own still has a tip: the snapshot it was created from.
    pub tip: Option<Tip>,
}

/// Every branch, sorted by name, with the snapshot each points at.
///
/// The checked-out branch is always included even if nothing references it yet,
/// so a fresh repository still lists `main`.
pub fn list(repo: &Repo) -> Result<Vec<Branch>> {
    let conn = repo.conn();
    let current = current_branch(repo.root());

    let mut names = crate::commands::all_branch_names(conn);
    if !names.iter().any(|b| b.trim() == current) {
        names.push(current.clone());
    }
    names.sort();
    names.dedup();

    Ok(names
        .into_iter()
        .map(|name| {
            let tip = crate::commands::branch_tip(conn, &name).and_then(|hash| {
                conn.query_row(
                    "SELECT hash, message, created_at_ms FROM snapshots WHERE hash = ?",
                    [&hash],
                    |r| {
                        Ok(Tip {
                            hash: r.get(0)?,
                            message: r.get(1)?,
                            created_at: crate::commands::timestamp_from_ms(r.get(2)?),
                        })
                    },
                )
                .ok()
            });
            Branch {
                is_current: name.trim() == current,
                name: BranchName::from_stored(name),
                tip,
            }
        })
        .collect())
}

/// Delete a branch, keeping its snapshots.
///
/// The deletion is soft: snapshots are moved to a `_deleted_<name>` branch rather
/// than removed, which is what makes them recoverable. The ref itself is dropped
/// so the branch stops being listed — and so a branch with no commits of its own
/// can be deleted at all.
pub fn delete(guard: &WriteGuard, name: &BranchName) -> Result<()> {
    let conn = guard.conn();
    let current = current_branch(guard.root());

    if name.trim() == current {
        return Err(VeloError::invalid(format!(
            "Cannot delete the currently active branch '{}'. Switch to another branch first.",
            name
        )));
    }
    if name.as_str() == "main" {
        return Err(VeloError::invalid(
            "Cannot delete the default 'main' branch.",
        ));
    }

    let moved = conn.execute(
        "UPDATE snapshots SET branch = ?1 WHERE branch = ?2",
        params![format!("_deleted_{}", name), name],
    )?;
    let refs = conn.execute("DELETE FROM branches WHERE name = ?", [name])?;
    if moved == 0 && refs == 0 {
        return Err(VeloError::not_found(RefKind::Branch, name.as_str()));
    }
    Ok(())
}

/// Create `name`, optionally pointing at an existing snapshot.
///
/// Neither existing route did this. `switch::run` creates a branch but also makes
/// it current and leaves it unborn, inheriting wherever the position happens to
/// be; `save_tree` creates one only as a side effect of recording a snapshot, so
/// a branch could not exist before its first checkpoint. This is
/// `git branch <name> [<commit>]`: a ref, and nothing else moves.
///
/// `at = None` leaves the branch unborn — it exists, but has no tip until
/// something is saved on it.
///
/// # Errors
/// If the branch already exists, or `at` names a snapshot that does not.
pub fn create(guard: &WriteGuard, name: &BranchName, at: Option<&SnapshotId>) -> Result<()> {
    let conn = guard.conn();
    if crate::commands::branch_exists(conn, name.as_str()) {
        return Err(VeloError::invalid(format!(
            "Branch '{}' already exists.",
            name
        )));
    }
    match at {
        Some(at) => {
            require_snapshot(conn, at)?;
            crate::commands::set_branch_tip(conn, name.as_str(), at.as_str())?;
        }
        None => crate::commands::register_branch(conn, name.as_str(), "")?,
    }
    Ok(())
}

/// Point an existing branch at `to`.
///
/// Moving a branch does not rewrite or delete anything: snapshots that were only
/// reachable from the old tip remain in the repository and are still reachable by
/// id, which is why this is not a destructive operation.
///
/// # Errors
/// If the branch does not exist, or `to` names a snapshot that does not.
pub fn set_tip(guard: &WriteGuard, name: &BranchName, to: &SnapshotId) -> Result<()> {
    let conn = guard.conn();
    if !crate::commands::branch_exists(conn, name.as_str()) {
        return Err(VeloError::not_found(RefKind::Branch, name.as_str()));
    }
    require_snapshot(conn, to)?;
    crate::commands::set_branch_tip(conn, name.as_str(), to.as_str())?;
    Ok(())
}

/// A branch may only point at a snapshot that exists — otherwise the ref is
/// dangling and only `fsck` would find it.
fn require_snapshot(conn: &rusqlite::Connection, id: &SnapshotId) -> Result<()> {
    let known: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
            [id],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if known {
        Ok(())
    } else {
        Err(VeloError::not_found(RefKind::Snapshot, id.as_str()))
    }
}

fn current_branch(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/HEAD"))
        .unwrap_or_else(|_| "main".into())
        .trim()
        .to_string()
}
