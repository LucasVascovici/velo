//! `velo switch <branch>` — move to another branch, creating it if needed.
//!
//! Visiting a branch never moves it: a new branch is recorded but left unborn,
//! and only starts pointing somewhere when you save on it or merge into it.
//! Returns what happened as data; wording lives in `velo-cli`.

use std::fs;

use crate::commands::require_working_tree;
use crate::commands::{dirty_scan, FileStatus};
use crate::error::{Result, VeloError};
use crate::SnapshotId;
use crate::WriteGuard;

/// What a switch did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Already on that branch; nothing changed.
    AlreadyOn { branch: String },
    /// Moved to a branch with commits. The working tree was restored to its tip.
    Switched { branch: String, at: String },
    /// Moved to a branch with no commits of its own. The working tree carries
    /// over, and the branch stays unborn until something starts it.
    StartedUnborn {
        branch: String,
        /// True when the branch had been visited before rather than created now.
        existing: bool,
        /// Where a first save would start its history; `None` in a repository
        /// with no snapshots at all.
        inherits: Option<String>,
    },
}

/// Switch to `branch_name`, creating the branch if it doesn't exist.
///
/// `force` discards unsaved changes to tracked files.
pub fn run(guard: &WriteGuard, branch_name: &str, force: bool) -> Result<Outcome> {
    require_working_tree(guard.repo(), "switch")?;
    let root = guard.root();
    if !force {
        crate::commands::refuse_in_progress(root)?;
    }
    if branch_name.starts_with("_deleted_") {
        return Err(VeloError::invalid(format!(
            "Branch '{}' has been deleted.",
            branch_name.trim_start_matches("_deleted_")
        )));
    }

    let current_head = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    if current_head.trim() == branch_name {
        // "On the branch" is only true when the working position is its tip. An
        // import onto the checked-out branch (a bundle applied to a fresh repo)
        // or a `restore` of an older snapshot leaves PARENT empty or elsewhere,
        // and answering "already on" there would strand the user with no way to
        // get the files back short of naming a snapshot.
        let conn = guard.conn();
        let tip = crate::commands::branch_tip(conn, branch_name);
        let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
        let position = position.trim();
        let Some(tip) = tip.filter(|t| t != position) else {
            return Ok(Outcome::AlreadyOn {
                branch: branch_name.to_string(),
            });
        };

        let mut scan = dirty_scan(guard.repo());
        let dirty = &mut scan.status;
        if position.is_empty() {
            // With no position every file reads as New, which the normal rule
            // treats as safe. Here the tip is about to be written over them, so a
            // file that would be overwritten with different content is at risk.
            let tree = crate::commands::apply::load_tree(conn, &tip)?;
            for (path, status) in dirty.iter_mut() {
                if *status != FileStatus::New {
                    continue;
                }
                if let Some((hash, mode)) = tree.get(path) {
                    if crate::storage::hash_for(&root.join(path), *mode) != *hash {
                        *status = FileStatus::Modified;
                    }
                }
            }
        }
        if !force {
            let mut at_risk: Vec<std::path::PathBuf> = dirty
                .iter()
                .filter(|(_, status)| **status != FileStatus::New)
                .map(|(path, _)| std::path::PathBuf::from(path))
                .collect();
            if !at_risk.is_empty() {
                at_risk.sort();
                return Err(VeloError::DirtyWorkingTree { paths: at_risk });
            }
        }
        crate::commands::restore::run_with(
            guard,
            &SnapshotId::from_stored(tip.as_str()),
            crate::commands::restore::Options {
                force: true,
                ..Default::default()
            },
            crate::commands::restore::Strategy::Delta,
            Some(scan),
        )?;
        crate::commands::mv::discard_pending(guard.conn())?;
        return Ok(Outcome::Switched {
            branch: branch_name.to_string(),
            at: tip,
        });
    }

    // Kept so a newly created branch can report what a first save would build on.
    let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    let conn = guard.conn();
    let target_tip: Option<String> = crate::commands::branch_tip(conn, branch_name);

    // Only *tracked* changes are at risk: switching restores the target snapshot
    // over them. Brand-new files are in no snapshot, so they're carried along —
    // blocking on them (and then deleting them under --force) was both surprising
    // and destructive. Creating a branch that inherits the current position
    // restores nothing, so it never blocks.
    let dirty = target_tip.is_some().then(|| dirty_scan(guard.repo()));
    if let (Some(dirty), false) = (&dirty, force) {
        let mut at_risk: Vec<std::path::PathBuf> = dirty
            .status
            .iter()
            .filter(|(_, status)| **status != FileStatus::New)
            .map(|(path, _)| std::path::PathBuf::from(path))
            .collect();
        if !at_risk.is_empty() {
            at_risk.sort();
            // Previously this printed a message and returned Ok, so a refused
            // switch still exited 0 and scripts couldn't tell it hadn't happened.
            return Err(VeloError::DirtyWorkingTree { paths: at_risk });
        }
    }

    let existing = crate::commands::branch_exists(conn, branch_name);
    crate::commands::register_branch(conn, branch_name, "")?;
    crate::failpoint::hit("switch.before_head");
    crate::storage::write_atomic(&root.join(".velo/HEAD"), branch_name.as_bytes())?;
    crate::failpoint::hit("switch.after_head");

    match target_tip {
        Some(at) => {
            crate::commands::restore::run_with(
                guard,
                &SnapshotId::from_stored(at.as_str()),
                crate::commands::restore::Options {
                    force: true,
                    ..Default::default()
                },
                crate::commands::restore::Strategy::Delta,
                dirty,
            )?;
            // A whole-tree rewrite lands a tree that no pending move describes, so the
            // moves are forgotten rather than attached to whatever gets saved next.
            crate::commands::mv::discard_pending(guard.conn())?;
            Ok(Outcome::Switched {
                branch: branch_name.to_string(),
                at,
            })
        }
        None => Ok(Outcome::StartedUnborn {
            branch: branch_name.to_string(),
            existing,
            inherits: Some(position.trim().to_string()).filter(|p| !p.is_empty()),
        }),
    }
}
