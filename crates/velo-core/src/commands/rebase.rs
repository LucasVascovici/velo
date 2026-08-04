//! `velo rebase <target>` — replay this branch's commits on top of another,
//! producing a linear history.
//!
//! Every commit on the current branch that isn't already in the target's
//! ancestry is applied onto the target in order, using the same three-way
//! reconciliation as merge and cherry-pick (see [`crate::commands::apply`]). A
//! conflict pauses the rebase; the remaining commits stay recorded so
//! `--continue` picks up where it stopped.
//!
//! State on disk:
//! * `REBASE_STATE` — remaining commits to replay, one hash per line, oldest first
//! * `REBASE_ONTO` — the hash being rebased onto
//! * `REBASE_ORIG_HEAD` — where the branch was, so `--abort` can put it back

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::commands::{apply, apply::Applied, get_dirty_files};
use crate::error::{InProgress, Result, VeloError};
use crate::storage;
use crate::Repo;
use crate::WriteGuard;

/// One commit that was replayed onto the new base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replayed {
    pub snapshot: String,
    pub message: String,
    /// 1-based position in this run's replay list.
    pub index: usize,
    /// How many commits this run set out to replay.
    pub total: usize,
}

/// How a rebase ended.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Nothing to replay: the branch is already on top of the target.
    AlreadyUpToDate,
    /// Every commit replayed cleanly.
    Completed {
        branch: String,
        /// The hash the branch was rebased onto.
        onto: String,
        /// Where the branch now points.
        head: String,
        replayed: Vec<Replayed>,
    },
    /// A commit conflicted; the rebase is paused for the user.
    Paused {
        branch: String,
        onto: String,
        /// Commits that landed before the conflict.
        replayed: Vec<Replayed>,
        /// The commit that could not be applied cleanly.
        stopped_at: Replayed,
        /// What that commit's reconciliation decided.
        applied: Applied,
    },
    /// `--abort` unwound the rebase.
    Aborted {
        /// Where the branch was put back to.
        restored_to: Option<String>,
        /// Replayed snapshots that were discarded.
        discarded: usize,
    },
}

/// Start, continue, or abort a rebase.
pub fn run(guard: &WriteGuard, target: &str, abort: bool, cont: bool) -> Result<Outcome> {
    if abort {
        return do_abort(guard);
    }
    if cont {
        return do_continue(guard);
    }
    do_start(guard, target)
}

// ── Start ─────────────────────────────────────────────────────────────────────

fn do_start(guard: &WriteGuard, target: &str) -> Result<Outcome> {
    let root = guard.root();
    if root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Rebase,
        });
    }

    let dirty = get_dirty_files(guard.repo());
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }

    let conn = guard.conn();
    let branch = read_trimmed(guard.root(), "HEAD");
    let head_hash = read_trimmed(guard.root(), "PARENT");

    let onto_hash = crate::commands::resolve_snapshot_id(guard.repo(), target)?;

    // Nothing to do when the branch already sits on top of the target. Testing
    // only `onto == head` was too narrow: after a successful rebase the branch
    // *contains* onto without equalling it, so re-running the same rebase
    // replayed every commit again onto fresh hashes, duplicating the branch.
    if ancestry(conn, &head_hash).contains(&onto_hash) {
        return Ok(Outcome::AlreadyUpToDate);
    }

    let onto_ancestors = ancestry(conn, &onto_hash);
    let commits = branch_linear_history(conn, &head_hash, &onto_ancestors);
    if commits.is_empty() {
        return Ok(Outcome::AlreadyUpToDate);
    }

    fs::write(root.join(".velo/REBASE_ONTO"), &onto_hash)?;
    fs::write(root.join(".velo/REBASE_ORIG_HEAD"), &head_hash)?;

    // Move onto the new base. `restore` sets PARENT, which is what the replay
    // builds on top of; HEAD keeps naming our branch.
    crate::commands::restore::run(guard, &onto_hash, true, &[])?;

    write_state(
        guard.root(),
        &commits.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>(),
    )?;
    replay(guard, &branch, &onto_hash, &commits)
}

// ── Continue after resolving a conflict ───────────────────────────────────────

fn do_continue(guard: &WriteGuard) -> Result<Outcome> {
    let root = guard.root();
    if !root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::NoOperationInProgress {
            what: InProgress::Rebase,
        });
    }
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::Conflicts {
            paths: conflict_paths(guard.repo()),
        });
    }

    let remaining = read_state(guard.root())?;
    let onto = read_trimmed(guard.root(), "REBASE_ONTO");
    if remaining.is_empty() {
        return finish(
            guard,
            &read_trimmed(guard.root(), "HEAD"),
            &onto,
            Vec::new(),
        );
    }

    let conn = guard.conn();
    let branch = read_trimmed(guard.root(), "HEAD");
    let commits: Vec<(String, String)> = remaining
        .iter()
        .map(|h| {
            let message = conn
                .query_row("SELECT message FROM snapshots WHERE hash = ?", [h], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap_or_default();
            (h.clone(), message)
        })
        .collect();

    replay(guard, &branch, &onto, &commits)
}

// ── Abort ─────────────────────────────────────────────────────────────────────

fn do_abort(guard: &WriteGuard) -> Result<Outcome> {
    let root = guard.root();
    if !root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::NoOperationInProgress {
            what: InProgress::Rebase,
        });
    }

    let orig_head = read_trimmed(guard.root(), "REBASE_ORIG_HEAD");
    let onto = read_trimmed(guard.root(), "REBASE_ONTO");
    let discarded = discard_replayed(guard, &onto, &orig_head);

    let _ = fs::remove_file(root.join(".velo/MERGE_HEAD"));
    clear_state(guard.root());

    let restored_to = if orig_head.is_empty() {
        None
    } else {
        crate::commands::restore::run(guard, &orig_head, true, &[])?;
        Some(orig_head)
    };

    Ok(Outcome::Aborted {
        restored_to,
        discarded,
    })
}

/// Delete the snapshots created while replaying.
///
/// Without this the replayed commits stay in the database sitting on top of
/// `onto`, and because branch-tip resolution orders by `created_at` they would
/// masquerade as the branch tip even though the position was restored.
fn discard_replayed(guard: &WriteGuard, onto: &str, orig_head: &str) -> usize {
    let conn = guard.conn();
    let _ = conn.execute("DELETE FROM hunk_decisions", []);
    let _ = conn.execute("DELETE FROM conflict_files", []);

    let branch = read_trimmed(guard.root(), "HEAD");
    let mut hash = read_trimmed(guard.root(), "PARENT");

    // Walk back from the current tip to `onto`, collecting this branch's replayed
    // commits — both auto-replayed and manually saved during a paused conflict.
    let mut doomed: Vec<String> = Vec::new();
    while !hash.is_empty() && hash != onto && hash != orig_head {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT branch, parent_hash FROM snapshots WHERE hash = ?",
                [&hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match row {
            // Only commits belonging to the branch being rebased.
            Some((b, parent)) if b == branch => {
                doomed.push(hash.clone());
                hash = parent.trim().to_string();
            }
            _ => break,
        }
    }
    for h in &doomed {
        let _ = conn.execute("DELETE FROM file_map  WHERE snapshot_hash = ?", [h]);
        let _ = conn.execute("DELETE FROM snapshots WHERE hash = ?", [h]);
    }
    doomed.len()
}

// ── Replay loop ───────────────────────────────────────────────────────────────

fn replay(
    guard: &WriteGuard,
    branch: &str,
    onto: &str,
    commits: &[(String, String)],
) -> Result<Outcome> {
    let root = guard.root();
    let total = commits.len();
    let mut replayed: Vec<Replayed> = Vec::new();

    for (idx, (snapshot, message)) in commits.iter().enumerate() {
        let step = Replayed {
            snapshot: snapshot.clone(),
            message: message.clone(),
            index: idx + 1,
            total,
        };
        let applied = apply_one(guard, snapshot)?;

        // Drop this commit from the remaining state either way. On a conflict the
        // user finishes it by hand, so `--continue` must resume with the *next*
        // commit rather than replaying this one again.
        advance_state(root)?;

        if !applied.is_clean() {
            return Ok(Outcome::Paused {
                branch: branch.to_string(),
                onto: onto.to_string(),
                replayed,
                stopped_at: step,
                applied,
            });
        }

        crate::commands::save::run(guard, message, false)?;
        replayed.push(step);
    }

    finish(guard, branch, onto, replayed)
}

fn finish(
    guard: &WriteGuard,
    branch: &str,
    onto: &str,
    replayed: Vec<Replayed>,
) -> Result<Outcome> {
    clear_state(guard.root());
    Ok(Outcome::Completed {
        branch: branch.to_string(),
        onto: onto.to_string(),
        head: read_trimmed(guard.root(), "PARENT"),
        replayed,
    })
}

/// Apply one commit's changes onto the current working tree.
fn apply_one(guard: &WriteGuard, snapshot: &str) -> Result<Applied> {
    let root = guard.root();
    let conn = guard.conn();
    let parent_hash: String = conn
        .query_row(
            "SELECT parent_hash FROM snapshots WHERE hash = ?",
            [snapshot],
            |r| r.get(0),
        )
        .map_err(|_| VeloError::not_found(crate::error::RefKind::Snapshot, snapshot))?;

    let position = read_trimmed(guard.root(), "PARENT");
    // "theirs" is the commit being replayed; "ours" is the tree built so far.
    let applied = apply::reconcile_tree(
        root,
        &apply::load_tree(conn, &parent_hash)?,
        &apply::load_tree(conn, &position)?,
        &apply::load_tree(conn, snapshot)?,
    )?;

    if applied.is_clean() {
        return Ok(applied);
    }

    // MERGE_HEAD's "<pre-hash>:rebase/<snap>" form drives resolve and abort.
    storage::write_atomic(
        &root.join(".velo/MERGE_HEAD"),
        format!("{}:rebase/{}", position, &snapshot[..8]).as_bytes(),
    )?;
    applied.record_conflicts(conn)?;
    Ok(applied)
}

// ── State files ───────────────────────────────────────────────────────────────

fn read_trimmed(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(".velo").join(name))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn read_state(root: &Path) -> Result<Vec<String>> {
    let raw = fs::read_to_string(root.join(".velo/REBASE_STATE"))?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect())
}

fn write_state(root: &Path, remaining: &[String]) -> Result<()> {
    fs::write(root.join(".velo/REBASE_STATE"), remaining.join("\n"))?;
    Ok(())
}

/// Drop the first remaining commit.
fn advance_state(root: &Path) -> Result<()> {
    let rest: Vec<String> = read_state(root)
        .unwrap_or_default()
        .into_iter()
        .skip(1)
        .collect();
    write_state(root, &rest)
}

fn clear_state(root: &Path) {
    for name in ["REBASE_STATE", "REBASE_ONTO", "REBASE_ORIG_HEAD"] {
        let _ = fs::remove_file(root.join(".velo").join(name));
    }
}

fn conflict_paths(repo: &Repo) -> Vec<std::path::PathBuf> {
    crate::commands::get_conflict_files(repo)
        .into_iter()
        .map(std::path::PathBuf::from)
        .collect()
}

// ── Ancestry ──────────────────────────────────────────────────────────────────

/// Walk history back from `start`, stopping at anything in `stop_set`.
/// Returns commits in replay order, oldest first.
fn branch_linear_history(
    conn: &rusqlite::Connection,
    start: &str,
    stop_set: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut cur = start.to_string();
    while !cur.is_empty() && !stop_set.contains(&cur) {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT message, parent_hash FROM snapshots WHERE hash = ?",
                [&cur],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        match row {
            Some((message, parent)) => {
                result.push((cur, message));
                cur = parent;
            }
            None => break,
        }
    }
    result.reverse();
    result
}

/// Every ancestor of `hash`, including itself.
fn ancestry(conn: &rusqlite::Connection, hash: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut stack = vec![hash.to_string()];
    while let Some(h) = stack.pop() {
        if h.is_empty() || !set.insert(h.clone()) {
            continue;
        }
        if let Ok(parent) = conn.query_row(
            "SELECT parent_hash FROM snapshots WHERE hash = ?",
            [&h],
            |r| r.get::<_, String>(0),
        ) {
            stack.push(parent);
        }
    }
    set
}
