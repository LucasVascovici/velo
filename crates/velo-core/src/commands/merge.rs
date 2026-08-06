//! `velo merge` — bring another branch's work into this one.
//!
//! Returns what happened as data: which branch was merged into which, what each
//! file's reconciliation decided, and which paths were left conflicted. Wording,
//! colour and the "here's how to resolve it" guidance live in `velo-cli`.

use std::fs;

use rusqlite::params;

use crate::commands::SnapshotIdentity;
use crate::commands::{apply, apply::Applied, get_dirty_files};
use crate::error::{InProgress, Result, VeloError};
use crate::storage;
use crate::{ObjectHash, Repo, SnapshotId, SnapshotMeta, WriteGuard};

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
/// What [`run`] should do.
///
/// Replaces a `(Option<&str>, bool)` pair whose fourth combination — no target
/// and no abort — was an error discovered at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode<'a> {
    /// Merge `source` into the current branch.
    ///
    /// A **spec**, not a resolved id, and deliberately so: an exact local branch
    /// tip wins over a hash prefix (so a short branch name is never mis-read as
    /// one), tags and remote refs fall out of the fallback, and the *name* is
    /// what gets written to `MERGE_HEAD` for the eventual save to resolve into a
    /// second parent. A `SnapshotId` would throw that away. See [`crate::ids`]
    /// for why specs stay `&str`.
    Bring { source: &'a str },
    /// Abandon a merge in progress and restore the pre-merge state.
    Abort,
}

/// Merge, or abort one.
pub fn run(guard: &WriteGuard, mode: Mode<'_>) -> Result<Outcome> {
    match mode {
        Mode::Abort => do_abort(guard),
        Mode::Bring { source } => do_merge(guard, source),
    }
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
    crate::commands::restore::run(
        guard,
        &SnapshotId::from_stored(pre_merge_hash),
        crate::commands::restore::Options {
            force: true,
            ..Default::default()
        },
    )?;
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
    let target_probe = crate::commands::branch_tip(conn, target_branch).or_else(|| {
        crate::commands::resolve_snapshot_id(guard.repo(), target_branch)
            .ok()
            .map(SnapshotId::into_string)
    });
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
                crate::commands::restore::run(
                    guard,
                    &SnapshotId::from_stored(target_hash.as_str()),
                    crate::commands::restore::Options {
                        force: true,
                        ..Default::default()
                    },
                )?;
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
    let timestamp_ms = crate::commands::snapshot_timestamp_ms();
    let new_hash = crate::commands::snapshot_id(SnapshotIdentity {
        tree: &tree,
        parent: current_hash,
        merge_parent: "",
        message: &msg,
        timestamp_ms,
        meta: &SnapshotMeta::new(),
    });

    let tx = guard.transaction()?;
    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, created_at_ms)
         VALUES (?, ?, ?, ?, ?)",
        params![new_hash, &msg, head_branch, current_hash, timestamp_ms],
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
    crate::commands::restore::run(
        guard,
        &SnapshotId::from_stored(new_hash.as_str()),
        crate::commands::restore::Options {
            force: true,
            ..Default::default()
        },
    )?;
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
/// What one file's merge would do, without doing it.
///
/// The public shape of what reconciliation decided: typed object hashes, and a
/// conflict carrying its three sides so a caller can fetch and present them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlannedChange {
    /// Theirs deleted a file ours left alone.
    Delete,
    /// Take theirs verbatim. `is_new` when the file is not in the base.
    Take {
        object: ObjectHash,
        mode: i64,
        is_new: bool,
    },
    /// Both sides changed non-overlapping regions; this is the merged content.
    ///
    /// Carried because deciding "auto-merged rather than conflicted" *computes*
    /// it — throwing it away would make every caller redo the merge.
    AutoMerge { content: Vec<u8>, mode: i64 },
    /// Theirs deleted a file ours modified. Ours is kept; whether that was right
    /// is the author's call.
    KeepOurs,
    /// Both sides changed the same region. The three sides are given by object,
    /// absent where that side does not have the file.
    Conflict {
        base: Option<ObjectHash>,
        ours: Option<ObjectHash>,
        theirs: Option<ObjectHash>,
    },
}

impl PlannedChange {
    /// The same vocabulary the working-tree merge reports in.
    pub fn action(&self) -> apply::FileAction {
        match self {
            PlannedChange::Delete => apply::FileAction::Deleted,
            PlannedChange::Take { is_new: true, .. } => apply::FileAction::Added,
            PlannedChange::Take { .. } => apply::FileAction::Updated,
            PlannedChange::AutoMerge { .. } => apply::FileAction::AutoMerged,
            PlannedChange::KeepOurs => apply::FileAction::KeptOurs,
            PlannedChange::Conflict { .. } => apply::FileAction::Conflicted,
        }
    }
}

/// One path the merge would touch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: String,
    pub change: PlannedChange,
}

/// Everything a merge would do, with nothing done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergePlan {
    /// The shared ancestor the three-way merge is against. `None` when the two
    /// snapshots have no common history, in which case every path is treated as
    /// added or conflicting.
    pub base: Option<SnapshotId>,
    /// Paths that would change, in path order. Untouched files are omitted, so
    /// an empty list means the merge is a no-op.
    pub files: Vec<PlannedFile>,
}

impl MergePlan {
    /// Paths that would need a human.
    pub fn conflicts(&self) -> impl Iterator<Item = &PlannedFile> {
        self.files
            .iter()
            .filter(|f| matches!(f.change, PlannedChange::Conflict { .. }))
    }

    /// Whether the merge would complete without asking anything.
    pub fn is_clean(&self) -> bool {
        self.conflicts().next().is_none()
    }
}

/// Work out what merging `theirs` into `ours` would do — and do none of it.
///
/// [`run`] requires a clean working tree, writes files to disk and records
/// conflict state in the database. An application with an interface can use none
/// of that: its buffers are dirty by definition, and nothing may touch disk
/// before the author has seen and accepted the result. This is the same
/// classification with no side effects, so a consumer can present a merge, let
/// the author decide, and only then apply it however its own storage works.
///
/// Reads objects (deciding auto-merge from conflict requires the content) but
/// writes nothing, needs no [`WriteGuard`], and leaves the repository untouched.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// # let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
/// # let ours = velo_core::commands::resolve_snapshot_id(&repo, "main")?;
/// # let theirs = velo_core::commands::resolve_snapshot_id(&repo, "main")?;
/// let plan = velo_core::commands::merge::plan(&repo, &ours, &theirs)?;
/// if plan.is_clean() {
///     for file in &plan.files {
///         println!("{} {:?}", file.path, file.change.action());
///     }
/// }
/// # Ok(()) }
/// ```
pub fn plan(repo: &Repo, ours: &SnapshotId, theirs: &SnapshotId) -> Result<MergePlan> {
    let conn = repo.conn();
    let objects_dir = repo.root().join(".velo/objects");

    let base = merge_base(repo, ours, theirs)?;
    let base_tree = apply::load_tree(conn, base.as_ref().map_or("", |b| b.as_str()))?;
    let our_tree = apply::load_tree(conn, ours.as_str())?;
    let their_tree = apply::load_tree(conn, theirs.as_str())?;

    // Every path any side knows about, sorted, so a plan is the same on every
    // run rather than the hasher's order.
    let all_paths: std::collections::BTreeSet<&str> = base_tree
        .keys()
        .chain(our_tree.keys())
        .chain(their_tree.keys())
        .map(String::as_str)
        .collect();

    let mut files = Vec::new();
    for path in all_paths {
        let side = |t: &apply::Tree| {
            t.get(path)
                .map(|(h, m)| (h.clone(), *m))
                .unwrap_or_default()
        };
        let (base_h, base_m) = side(&base_tree);
        let (our_h, our_m) = side(&our_tree);
        let (their_h, their_m) = side(&their_tree);

        let object = |h: &str| (!h.is_empty()).then(|| ObjectHash::from_stored(h));

        let change = match crate::commands::reconcile_file(
            &objects_dir,
            (base_h.as_str(), base_m),
            (our_h.as_str(), our_m),
            (their_h.as_str(), their_m),
        )? {
            crate::commands::Reconcile::Nothing => continue,
            crate::commands::Reconcile::Delete => PlannedChange::Delete,
            crate::commands::Reconcile::TakeTheirs { hash, mode, is_new } => PlannedChange::Take {
                object: ObjectHash::from_stored(hash),
                mode,
                is_new,
            },
            crate::commands::Reconcile::AutoMerged { content, mode } => {
                PlannedChange::AutoMerge { content, mode }
            }
            crate::commands::Reconcile::KeepOurs => PlannedChange::KeepOurs,
            crate::commands::Reconcile::Conflict => PlannedChange::Conflict {
                base: object(&base_h),
                ours: object(&our_h),
                theirs: object(&their_h),
            },
        };
        files.push(PlannedFile {
            path: path.to_string(),
            change,
        });
    }

    Ok(MergePlan { base, files })
}

/// The most recent snapshot reachable from both `a` and `b`.
///
/// The base a three-way merge is computed against. `None` when the two share no
/// history at all, which happens for unrelated roots.
///
/// Exposed because any consumer that merges needs it, and reimplementing it over
/// history rows in memory is both slower and easy to get subtly wrong — this is
/// an indexed recursive query.
pub fn merge_base(repo: &Repo, a: &SnapshotId, b: &SnapshotId) -> Result<Option<SnapshotId>> {
    Ok(lowest_common_ancestor(repo.conn(), a.as_str(), b.as_str()).map(SnapshotId::from_stored))
}

/// The best common ancestor of `current` and `target`.
///
/// Both ancestries follow **both** parents (see
/// [`ancestors`](crate::commands::ancestors)), so a branch a previous merge
/// absorbed counts as reachable — which is the whole point. Without it the base
/// is the shared root, and the next merge diffs against a baseline predating
/// work that was already reconciled, re-raising every conflict the author
/// settled.
///
/// # "Lowest" here means nearest, and that is an approximation
///
/// Of the common ancestors, this returns the one at the shortest distance from
/// `current`, ties broken by id so the answer is stable.
///
/// In a tree that is exactly the merge base. In a DAG with criss-cross merges it
/// need not be: the strict definition is a common ancestor with no *other*
/// common ancestor reachable from it, and several such maxima can exist, which is
/// why git computes a set and merges them recursively. Nearest-common-ancestor
/// picks one common ancestor that is always *a* valid base — the merge is
/// correct, it may just present more of the history as contested than the ideal
/// base would.
///
/// Stated rather than assumed: an approximation someone knows about is fine, one
/// they discover from a strange merge is not.
fn lowest_common_ancestor(
    conn: &rusqlite::Connection,
    current: &str,
    target: &str,
) -> Option<String> {
    let ours = crate::commands::ancestors(conn, current).ok()?;
    let theirs = crate::commands::ancestors(conn, target).ok()?;

    ours.iter()
        .filter(|(hash, _)| theirs.contains_key(*hash))
        .min_by(|(a_hash, a_depth), (b_hash, b_depth)| {
            a_depth.cmp(b_depth).then_with(|| a_hash.cmp(b_hash))
        })
        .map(|(hash, _)| hash.clone())
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
