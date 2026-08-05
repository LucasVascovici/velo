//! `velo merge` — bring another branch's work into this one.
//!
//! Returns what happened as data: which branch was merged into which, what each
//! file's reconciliation decided, and which paths were left conflicted. Wording,
//! colour and the "here's how to resolve it" guidance live in `velo-cli`.

use std::fs;

use rusqlite::params;

use crate::commands::{apply, apply::Applied, get_dirty_files};
use crate::error::{InProgress, Result, VeloError};
use crate::storage;
use crate::WriteGuard;

/// Re-exported so `merge::FileAction` keeps working: the vocabulary is shared
/// with cherry-pick and rebase, so it lives in [`crate::commands::apply`].
pub use crate::commands::apply::{FileAction, FileOutcome};

/// A three-way merge that actually reconciled files.
#[derive(Clone, Debug)]
pub struct ThreeWay {
    /// The branch merged in, as the user named it.
    pub source: String,
    /// The branch merged into.
    pub into: String,
    /// The common ancestor, or `None` when the histories are unrelated.
    pub ancestor: Option<String>,
    /// What each file's reconciliation decided.
    pub applied: Applied,
}

impl ThreeWay {
    /// Every file that changed, in path order.
    pub fn files(&self) -> &[FileOutcome] {
        &self.applied.files
    }

    /// Files taken from the incoming branch that we didn't have.
    pub fn added(&self) -> usize {
        self.applied.added()
    }

    /// Files whose content changed, whether taken wholesale or auto-merged.
    pub fn updated(&self) -> usize {
        self.applied.updated()
    }

    pub fn deleted(&self) -> usize {
        self.applied.deleted()
    }

    /// Paths needing resolution, in path order.
    pub fn conflicts(&self) -> Vec<&str> {
        self.applied.conflicts()
    }

    /// No conflicts were left behind.
    pub fn is_clean(&self) -> bool {
        self.applied.is_clean()
    }

    /// Nothing was applied — the branches already agreed on every file.
    pub fn applied_nothing(&self) -> bool {
        self.applied.applied_nothing()
    }
}

/// How a merge ended.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// `--abort` unwound an in-progress merge.
    Aborted {
        source: String,
        /// The snapshot the tree was put back to, when one was recorded.
        restored_to: Option<String>,
    },
    /// The current branch had no commits, so it simply starts at the source.
    StartedUnbornBranch { branch: String, at: String },
    /// Both branches already point at the same snapshot.
    AlreadyUpToDate { branch: String, other: String },
    /// Our tip was an ancestor of theirs, so no merge commit was needed.
    FastForwarded { branch: String, to: String },
    /// A real three-way merge ran.
    Merged(ThreeWay),
}

/// Merge `target_branch` into the current branch, or unwind one with `abort`.
pub fn run(guard: &WriteGuard, target_branch: Option<&str>, abort: bool) -> Result<Outcome> {
    if abort {
        return do_abort(guard);
    }
    let target = target_branch
        .ok_or_else(|| VeloError::invalid("Specify a branch to merge: velo merge <branch>"))?;
    do_merge(guard, target)
}

// ─── Abort ───────────────────────────────────────────────────────────────────

fn do_abort(guard: &WriteGuard) -> Result<Outcome> {
    let root = guard.root();
    let merge_head_path = root.join(".velo/MERGE_HEAD");
    let conn = guard.conn();
    let conflict_count: i64 = conn
        .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
        .unwrap_or(0);

    if !merge_head_path.exists() && conflict_count == 0 {
        return Err(VeloError::NoOperationInProgress {
            what: InProgress::Merge,
        });
    }

    // MERGE_HEAD stores "pre_merge_hash:source_branch".
    let merge_info = fs::read_to_string(&merge_head_path).unwrap_or_default();
    let merge_info = merge_info.trim();
    let (pre_merge_hash, source_branch) = merge_info
        .split_once(':')
        .unwrap_or((merge_info, "(unknown)"));
    let source = source_branch.to_string();

    conn.execute("DELETE FROM hunk_decisions", [])?;
    conn.execute("DELETE FROM conflict_files", [])?;
    let _ = fs::remove_file(&merge_head_path);

    if pre_merge_hash.is_empty() {
        return Ok(Outcome::Aborted {
            source,
            restored_to: None,
        });
    }
    crate::commands::restore::run(guard, pre_merge_hash, true, &[])?;
    Ok(Outcome::Aborted {
        source,
        restored_to: Some(pre_merge_hash.to_string()),
    })
}

// ─── Merge ───────────────────────────────────────────────────────────────────

fn do_merge(guard: &WriteGuard, target_branch: &str) -> Result<Outcome> {
    let root = guard.root();
    // Checked before the dirty-tree test on purpose: a conflicted merge leaves
    // the tree dirty by design, so testing dirtiness first reported "unsaved
    // changes" when the real answer was "you're mid-merge".
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

    let conn = guard.conn();
    let head_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let head_branch = head_raw.trim();
    // Recorded so `--abort` can put the working tree back.
    let pre_merge_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    if head_branch == target_branch {
        return Err(VeloError::invalid(format!(
            "Cannot merge branch '{}' into itself.",
            target_branch
        )));
    }

    // Resolve the merge source first, so an unborn current branch can adopt it.
    // An exact local-branch tip wins over a hash prefix, so a short branch name
    // can't be mis-read as one; tags and remote refs fall out of the fallback.
    let target_probe = crate::commands::branch_tip(conn, target_branch)
        .or_else(|| crate::commands::resolve_snapshot_id(guard.repo(), target_branch).ok());
    let not_found = || {
        VeloError::invalid(format!(
            "'{}' not found — expected a branch, tag, snapshot, or remote ref.",
            target_branch
        ))
    };

    let current_hash: String = match crate::commands::branch_tip(conn, head_branch) {
        Some(h) => h,
        None => {
            // Merging into a branch with no commits just starts it at the source
            // (Git does the same) — there is nothing of ours to reconcile.
            let target_hash = target_probe.ok_or_else(not_found)?;
            crate::commands::set_branch_tip(conn, head_branch, &target_hash)?;
            // The tree often already matches, having just branched from it; only
            // touch it when it genuinely differs.
            if pre_merge_parent.trim() != target_hash {
                crate::commands::restore::run(guard, &target_hash, true, &[])?;
            }
            return Ok(Outcome::StartedUnbornBranch {
                branch: head_branch.to_string(),
                at: target_hash,
            });
        }
    };

    let target_hash: String = target_probe.ok_or_else(not_found)?;

    // Common right after branching, before either side has diverged.
    if current_hash == target_hash {
        return Ok(Outcome::AlreadyUpToDate {
            branch: head_branch.to_string(),
            other: target_branch.to_string(),
        });
    }

    if is_ancestor(conn, &target_hash, &current_hash) {
        return do_fast_forward(
            guard,
            head_branch,
            &current_hash,
            &target_hash,
            target_branch,
        );
    }

    let ancestor_hash = lowest_common_ancestor(conn, &current_hash, &target_hash);

    let applied = apply::reconcile_tree(
        guard,
        &match &ancestor_hash {
            Some(h) => apply::load_tree(conn, h)?,
            None => apply::Tree::new(),
        },
        &apply::load_tree(conn, &current_hash)?,
        &apply::load_tree(conn, &target_hash)?,
    )?;

    let result = ThreeWay {
        source: target_branch.to_string(),
        into: head_branch.to_string(),
        ancestor: ancestor_hash,
        applied,
    };

    // MERGE_HEAD is written for a clean merge too, not just a conflicted one: the
    // finalising `velo save` reads it to stamp the second parent. Without it a
    // conflict-free merge would collapse to a single-parent commit and
    // `velo history --graph` would draw it as linear.
    if !result.is_clean() || !result.applied_nothing() {
        storage::write_atomic(
            &root.join(".velo/MERGE_HEAD"),
            format!("{}:{}", pre_merge_parent.trim(), target_branch).as_bytes(),
        )?;
    }
    result.applied.record_conflicts(conn)?;

    Ok(Outcome::Merged(result))
}

fn do_fast_forward(
    guard: &WriteGuard,
    head_branch: &str,
    current_hash: &str,
    target_hash: &str,
    target_branch: &str,
) -> Result<Outcome> {
    let msg = format!("Fast-forward merge from '{}'", target_branch);
    // The fast-forward snapshot's tree is exactly the target's, modes included.
    let tree = load_tree(guard.conn(), target_hash)?;
    let timestamp = crate::commands::snapshot_timestamp();
    let new_hash = crate::commands::snapshot_id(&tree, current_hash, "", &msg, &timestamp);

    let tx = guard.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, created_at)
         VALUES (?, ?, ?, ?, ?)",
        params![new_hash, &msg, head_branch, current_hash, timestamp],
    )?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;
        for (p, h, m) in &tree {
            ins.execute(params![new_hash, p, h, m])?;
        }
    }
    tx.commit()?;

    // restore::run writes PARENT itself.
    crate::commands::restore::run(guard, &new_hash, true, &[])?;
    Ok(Outcome::FastForwarded {
        branch: head_branch.to_string(),
        to: new_hash,
    })
}

// ─── Ancestry ─────────────────────────────────────────────────────────────────

/// Is `candidate` an ancestor of `tip` (following first parents)?
fn is_ancestor(conn: &rusqlite::Connection, tip: &str, candidate: &str) -> bool {
    conn.query_row(
        "WITH RECURSIVE anc(hash, parent_hash) AS (
            SELECT hash, parent_hash FROM snapshots WHERE hash = ?1
            UNION ALL
            SELECT s.hash, s.parent_hash FROM snapshots s JOIN anc a ON s.hash = a.parent_hash
         )
         SELECT EXISTS(SELECT 1 FROM anc WHERE hash = ?2)",
        params![tip, candidate],
        |r| r.get(0),
    )
    .unwrap_or(false)
}

/// The nearest snapshot that is an ancestor of both tips.
///
/// Walks both ancestries and returns the shallowest shared hash. The recursive
/// CTE is depth-limited so a (theoretically impossible) cycle can't hang it.
fn lowest_common_ancestor(
    conn: &rusqlite::Connection,
    current: &str,
    target: &str,
) -> Option<String> {
    conn.query_row(
        "WITH RECURSIVE
         anc_cur(hash, parent_hash, depth) AS (
             SELECT hash, parent_hash, 0 FROM snapshots WHERE hash = ?1
             UNION ALL
             SELECT s.hash, s.parent_hash, a.depth + 1
             FROM snapshots s JOIN anc_cur a ON s.hash = a.parent_hash
             WHERE a.depth < 10000
         ),
         anc_tgt(hash, parent_hash) AS (
             SELECT hash, parent_hash FROM snapshots WHERE hash = ?2
             UNION ALL
             SELECT s.hash, s.parent_hash
             FROM snapshots s JOIN anc_tgt a ON s.hash = a.parent_hash
         )
         SELECT ac.hash
         FROM anc_cur ac JOIN anc_tgt at ON ac.hash = at.hash
         ORDER BY ac.depth ASC
         LIMIT 1",
        params![current, target],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

// ─── Loaders ──────────────────────────────────────────────────────────────────

fn load_tree(
    conn: &rusqlite::Connection,
    snapshot_hash: &str,
) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let tree: Vec<(String, String, i64)> = stmt
        .query_map([snapshot_hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tree)
}
