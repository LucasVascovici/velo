//! `velo branches` — list branches, or delete one.
//!
//! Listing and deleting were one function taking an `Option<String>`; they are
//! separate entry points now, since one reads and one writes. Formatting lives
//! in `velo-cli`.

use std::fs;
use std::path::Path;

use rusqlite::params;

use crate::error::{RefKind, Result, VeloError};
use crate::Repo;
use crate::WriteGuard;

/// The snapshot a branch points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    pub hash: String,
    pub message: String,
    /// Raw stored timestamp; formatting is the consumer's choice.
    pub created_at: String,
}

/// One branch and where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
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
                    "SELECT hash, message, created_at FROM snapshots WHERE hash = ?",
                    [&hash],
                    |r| {
                        Ok(Tip {
                            hash: r.get(0)?,
                            message: r.get(1)?,
                            created_at: r.get(2)?,
                        })
                    },
                )
                .ok()
            });
            Branch {
                is_current: name.trim() == current,
                name,
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
pub fn delete(guard: &WriteGuard, name: &str) -> Result<()> {
    let conn = guard.conn();
    let current = current_branch(guard.root());

    if name.trim() == current {
        return Err(VeloError::invalid(format!(
            "Cannot delete the currently active branch '{}'. Switch to another branch first.",
            name
        )));
    }
    if name == "main" {
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
        return Err(VeloError::not_found(RefKind::Branch, name));
    }
    Ok(())
}

fn current_branch(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/HEAD"))
        .unwrap_or_else(|_| "main".into())
        .trim()
        .to_string()
}
