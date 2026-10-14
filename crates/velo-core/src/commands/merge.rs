//! `velo merge` — bring another branch's work into this one.
//!
//! Returns what happened as data: which branch was merged into which, what each
//! file's reconciliation decided, and which paths were left conflicted. Wording,
//! colour and the "here's how to resolve it" guidance live in `velo-cli`.

use crate::commands::moves::MovedBy;
use std::fs;

use crate::commands::require_working_tree;
use crate::commands::{apply, apply::Applied, get_dirty_files};
use crate::error::{InProgress, RefKind, Result, VeloError};
use crate::storage;
use crate::{Author, BranchName, ObjectHash, Repo, SnapshotId, SnapshotMeta, WriteGuard};

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

    /// Nothing was applied — the trees were already equal but the histories
    /// diverged. `MERGE_HEAD` is written and the next save records the merge.
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
    /// HEAD already contains the other tip (the same snapshot, or an ancestor).
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
    require_working_tree(guard.repo(), "merge")?;
    match mode {
        Mode::Abort => do_abort(guard),
        Mode::Bring { source } => do_merge(guard, source),
    }
}

// ─── Abort ───────────────────────────────────────────────────────────────────

fn do_abort(guard: &WriteGuard) -> Result<Outcome> {
    let out = abort_inner(guard);
    // Whatever happened above, the saved bytes are consumed: a stale
    // MERGE_ORIG must not survive into the next merge.
    if let Ok(Outcome::Aborted {
        restored_to: Some(_),
        ..
    }) = &out
    {
        restore_orig_bytes(guard);
    }
    let _ = fs::remove_file(guard.root().join(MERGE_ORIG));
    out
}

fn abort_inner(guard: &WriteGuard) -> Result<Outcome> {
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

    let conflict_paths: Vec<String> = conn
        .prepare("SELECT path FROM conflict_files")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();

    conn.execute("DELETE FROM hunk_decisions", [])?;
    conn.execute("DELETE FROM conflict_files", [])?;
    let _ = fs::remove_file(&merge_head_path);
    // A whole-tree rewrite lands a tree that no pending move describes, so the
    // moves are forgotten rather than attached to whatever gets saved next.
    crate::commands::mv::discard_pending(guard.conn())?;

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
    remove_merge_added(guard, pre_merge_hash, &source, &conflict_paths)?;
    Ok(Outcome::Aborted {
        source,
        restored_to: Some(pre_merge_hash.to_string()),
    })
}

/// Transient file holding the exact bytes of CR-ending files a merge may rewrite.
const MERGE_ORIG: &str = ".velo/MERGE_ORIG";

/// Save the pre-merge bytes of every CR-ending text file the merge could
/// rewrite or delete.
///
/// The object store keeps text with every `\r` removed (FORMAT.md 2.1), so a
/// restore can only write the LF form. Without this copy `merge --abort` after
/// a rewrite would hand back `a\nb\n` for a file that was `a\r\nb\r\n`. The
/// tree is clean here (`do_merge` refuses a dirty one), so the working bytes
/// are the pre-merge bytes. LF and binary files already round-trip exactly and
/// are not recorded.
fn save_orig_bytes(root: &std::path::Path, our: &apply::Tree, theirs: &apply::Tree) {
    let mut out: Vec<u8> = Vec::new();
    let mut paths: Vec<&String> = our
        .iter()
        .filter(|(p, e)| theirs.get(*p) != Some(*e))
        .map(|(p, _)| p)
        .collect();
    paths.sort();
    for path in paths {
        let full = root.join(path);
        let is_link = fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink());
        if is_link {
            continue;
        }
        let Ok(bytes) = fs::read(&full) else { continue };
        if !bytes.contains(&b'\r') || bytes.contains(&0u8) {
            continue;
        }
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    if !out.is_empty() {
        let _ = storage::write_atomic(&root.join(MERGE_ORIG), &out);
    }
}

/// Parse `MERGE_ORIG` into `(path, bytes)` records, stopping at a truncated one.
fn read_orig_records(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    fn take<'a>(d: &mut &'a [u8]) -> Option<&'a [u8]> {
        let (len, rest) = d.split_first_chunk::<4>()?;
        let n = u32::from_le_bytes(*len) as usize;
        if rest.len() < n {
            return None;
        }
        let (body, rest) = rest.split_at(n);
        *d = rest;
        Some(body)
    }
    let mut d = data;
    let mut out = Vec::new();
    while !d.is_empty() {
        let (Some(p), Some(b)) = (take(&mut d), take(&mut d)) else {
            break;
        };
        let Ok(p) = String::from_utf8(p.to_vec()) else {
            continue;
        };
        out.push((p, b.to_vec()));
    }
    out
}

/// Put the saved CR bytes back over the restored LF files.
///
/// A record is applied only when the file now on disk hashes to the saved
/// content with its CRs removed, i.e. restore really did bring back the
/// pre-merge content. Anything else is left as restore wrote it.
fn restore_orig_bytes(guard: &WriteGuard) {
    let root = guard.root();
    let Ok(data) = fs::read(root.join(MERGE_ORIG)) else {
        return;
    };
    let mut done = Vec::new();
    for (path, bytes) in read_orig_records(&data) {
        let full = root.join(&path);
        let now = storage::hash_for(&full, storage::capture_mode(&full));
        let want = blake3::hash(&storage::normalise_crlf(bytes.clone()))
            .to_hex()
            .to_string();
        if now == want && fs::write(&full, &bytes).is_ok() {
            done.push(path);
        }
    }
    crate::commands::invalidate_cache_entries(guard.repo(), &done);
}

/// Delete the files a merge wrote that the pre-merge snapshot does not have.
///
/// `restore` only rewrites paths that differ from the tree being left, and the
/// tree being left is still the pre-merge snapshot, so a file the merge added
/// is invisible to it. The candidates are the source's paths plus the conflict
/// paths, minus the pre-merge tree. A candidate is deleted only if the dirty
/// scan reports it as new: that skips ignored paths, and `do_merge` refuses a
/// dirty tree, so no untracked file of the user's can sit at such a path.
fn remove_merge_added(
    guard: &WriteGuard,
    pre_merge_hash: &str,
    source: &str,
    conflict_paths: &[String],
) -> Result<()> {
    let conn = guard.conn();
    let source_hash = crate::commands::branch_tip(conn, source).or_else(|| {
        let spec = source
            .strip_prefix("cherry-pick/")
            .or_else(|| source.strip_prefix("rebase/"))
            .unwrap_or(source);
        crate::commands::resolve_snapshot_id(guard.repo(), spec)
            .ok()
            .map(SnapshotId::into_string)
    });
    let pre_tree = apply::load_tree(conn, pre_merge_hash)?;
    let mut candidates: std::collections::BTreeSet<String> =
        conflict_paths.iter().cloned().collect();
    if let Some(h) = source_hash {
        candidates.extend(apply::load_tree(conn, &h)?.into_keys());
    }
    candidates.retain(|p| !pre_tree.contains_key(p));
    if candidates.is_empty() {
        return Ok(());
    }
    let root = guard.root();
    let dirty = get_dirty_files(guard.repo());
    for path in candidates {
        if dirty.get(&path) != Some(&crate::commands::FileStatus::New) {
            continue;
        }
        let full = root.join(&path);
        if fs::remove_file(&full).is_ok() {
            if let Some(parent) = full.parent() {
                crate::commands::remove_empty_parents(parent, root);
            }
        }
    }
    Ok(())
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
    let mut absent_ref: Option<VeloError> = None;
    let target_probe = crate::commands::branch_tip(conn, target_branch).or_else(|| {
        match crate::commands::resolve_snapshot_id(guard.repo(), target_branch) {
            Ok(id) => Some(id.into_string()),
            // Kept so the caller hears "fetch it", not a generic "not found".
            Err(e @ VeloError::RemoteRefAbsent { .. }) => {
                absent_ref = Some(e);
                None
            }
            Err(_) => None,
        }
    });
    let target_hash: String = match target_probe {
        Some(h) => {
            // Checked before anything is written: a merge must never record a
            // parent that is not a live snapshot (an undone one a tracking ref
            // still names, say).
            if !crate::commands::snapshot_is_live(conn, &h) {
                return Err(
                    match crate::commands::resolve_snapshot_id(guard.repo(), target_branch) {
                        Err(e @ VeloError::RemoteRefAbsent { .. }) => e,
                        _ => VeloError::not_found(RefKind::Snapshot, h),
                    },
                );
            }
            h
        }
        None => {
            return Err(absent_ref.unwrap_or_else(|| {
                VeloError::invalid(format!(
                    "'{}' not found — expected a branch, tag, snapshot, or remote ref.",
                    target_branch
                ))
            }))
        }
    };

    let current_hash: String = match crate::commands::branch_tip(conn, head_branch) {
        Some(h) => h,
        None => {
            // Merging into a branch with no commits just starts it at the source
            // (Git does the same) — there is nothing of ours to reconcile.
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

    // Common right after branching, before either side has diverged.
    if current_hash == target_hash {
        return Ok(Outcome::AlreadyUpToDate {
            branch: head_branch.to_string(),
            other: target_branch.to_string(),
        });
    }

    // Both parents count: a branch a previous merge joined in is an ancestor
    // through its `merge_parent`, and following first parents alone would call
    // it diverged.
    let theirs_ancestry = crate::commands::ancestors(conn, &target_hash)?;
    if theirs_ancestry.contains_key(&current_hash) {
        return do_fast_forward(
            guard,
            head_branch,
            &current_hash,
            &target_hash,
            target_branch,
        );
    }

    // HEAD already contains the other tip, so there is nothing to bring in. This
    // is deliberately not true of diverged histories with equal trees: those
    // fall through and record a merge.
    if crate::commands::ancestors(conn, &current_hash)?.contains_key(&target_hash) {
        return Ok(Outcome::AlreadyUpToDate {
            branch: head_branch.to_string(),
            other: target_branch.to_string(),
        });
    }

    let ancestor_hash = lowest_common_ancestor(conn, &current_hash, &target_hash);

    let mut base_tree = match &ancestor_hash {
        Some(h) => apply::load_tree(conn, h)?,
        None => apply::Tree::new(),
    };
    let mut our_tree = apply::load_tree(conn, &current_hash)?;
    let mut their_tree = apply::load_tree(conn, &target_hash)?;
    let moves = crate::commands::moves::follow_moves(
        conn,
        ancestor_hash.as_deref(),
        &current_hash,
        &target_hash,
        &mut base_tree,
        &mut our_tree,
        &mut their_tree,
    )?;
    let _ = fs::remove_file(root.join(MERGE_ORIG));
    save_orig_bytes(root, &our_tree, &their_tree);
    let applied = apply::reconcile_tree_moving(guard, &base_tree, &our_tree, &their_tree, &moves)?;

    let result = ThreeWay {
        source: target_branch.to_string(),
        into: head_branch.to_string(),
        ancestor: ancestor_hash,
        applied,
    };

    // MERGE_HEAD is written for a clean merge too, not just a conflicted one: the
    // finalising `velo save` reads it to stamp the second parent. Without it a
    // conflict-free merge would collapse to a single-parent commit and
    // `velo history --graph` would draw it as linear. Every `Merged` outcome
    // means the histories diverged, so it is written even when the trees were
    // already equal: the merge still has to be recorded to join the histories.
    storage::write_atomic(
        &root.join(".velo/MERGE_HEAD"),
        format!("{}:{}", pre_merge_parent.trim(), target_branch).as_bytes(),
    )?;
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
    use crate::tree::{SaveTree, TreeEntry};
    let msg = format!("Fast-forward merge from '{}'", target_branch);
    let current = SnapshotId::from_stored(current_hash);
    let target = SnapshotId::from_stored(target_hash);
    let repo = guard.repo();
    // The fast-forward snapshot's tree is exactly the target's, modes included.
    let entries: Vec<TreeEntry> = repo
        .tree_at(&target)?
        .into_iter()
        .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
        .collect();
    // velo keeps the branch as a column on each snapshot, so a ref cannot be
    // moved. Recording the target as the merge parent keeps its history
    // reachable from this branch, and its author is carried over because in a
    // ref-moving system the new tip would be the feature's own commit.
    let author = repo.snapshot_meta(&target)?.author();
    let new_id = guard.save_tree(SaveTree {
        branch: &BranchName::from_stored(head_branch),
        parent: Some(&current),
        merge_parent: Some(&target),
        message: &msg,
        entries,
        meta: SnapshotMeta::new(),
        author: author.as_ref(),
        renames: &[],
        timestamp_ms: None,
    })?;
    let new_hash = new_id.as_str().to_string();

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
    /// an empty list means the merge changes no content. That is still a merge
    /// to record when `base` is not `theirs`: diverged histories with equal
    /// trees join into a two-parent snapshot. Only `base == Some(theirs)` means
    /// there is nothing to merge.
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
    for id in [ours, theirs] {
        if !crate::commands::snapshot_is_live(conn, id.as_str()) {
            return Err(VeloError::not_found(RefKind::Snapshot, id.as_str()));
        }
    }
    let objects = repo.objects();

    let base = merge_base(repo, ours, theirs)?;
    let mut base_tree = apply::load_tree(conn, base.as_ref().map_or("", |b| b.as_str()))?;
    let mut our_tree = apply::load_tree(conn, ours.as_str())?;
    let mut their_tree = apply::load_tree(conn, theirs.as_str())?;
    let moves = crate::commands::moves::follow_moves(
        conn,
        base.as_ref().map(|b| b.as_str()),
        ours.as_str(),
        theirs.as_str(),
        &mut base_tree,
        &mut our_tree,
        &mut their_tree,
    )?;

    // Every path any side knows about, sorted, so a plan is the same on every
    // run rather than the hasher's order.
    let all_paths: std::collections::BTreeSet<&str> = base_tree
        .keys()
        .chain(our_tree.keys())
        .chain(their_tree.keys())
        .map(String::as_str)
        .collect();

    let mut files = Vec::new();
    // The old path of a move of theirs is deleted: ours' file lives at the new
    // path now.
    for m in moves.iter().filter(|m| m.by == MovedBy::Theirs) {
        files.push(PlannedFile {
            path: m.from.clone(),
            change: PlannedChange::Delete,
        });
    }
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
            &objects,
            (base_h.as_str(), base_m),
            (our_h.as_str(), our_m),
            (their_h.as_str(), their_m),
            repo.drivers().for_path(path),
        )? {
            crate::commands::Reconcile::Nothing => {
                // A pure move of theirs changes nothing at the new path in the
                // trees, but the path is new relative to our tip's old name.
                if moves
                    .iter()
                    .any(|m| m.by == MovedBy::Theirs && m.to == path)
                {
                    PlannedChange::Take {
                        object: ObjectHash::from_stored(our_h.as_str()),
                        mode: our_m,
                        is_new: true,
                    }
                } else {
                    continue;
                }
            }
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

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(MergePlan { base, files })
}

/// How one path the plan could not settle is settled.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolution {
    /// Keep our side; the path is removed if ours lacks it.
    Ours,
    /// Take their side; the path is removed if theirs lacks it.
    Theirs,
    /// Write exactly these bytes, with ours' mode (theirs' if ours lacks the
    /// file, regular if neither has it).
    Content(Vec<u8>),
    /// Remove the path.
    Delete,
}

/// A merge to record, store-only.
#[derive(Clone, Debug)]
pub struct MergeCommit<'a> {
    /// The branch the merge snapshot is recorded on.
    pub branch: &'a BranchName,
    pub ours: &'a SnapshotId,
    pub theirs: &'a SnapshotId,
    /// Settlements for conflicted (and optionally kept-ours) paths.
    pub resolutions: &'a [(String, Resolution)],
    pub message: &'a str,
    pub meta: SnapshotMeta,
    pub author: Option<&'a Author>,
    pub timestamp_ms: Option<i64>,
}

/// Record the merge of `theirs` into `ours` as a snapshot, with no working tree.
///
/// The apply half of [`plan`]: the plan is recomputed here rather than accepted,
/// because it is deterministic, so a caller cannot hand in a stale one. Every
/// conflicted path needs a [`Resolution`]; kept-ours paths may have one.
///
/// The snapshot has `ours` as parent and `theirs` as merge parent, so the next
/// merge finds `theirs` as the base and does not re-raise settled conflicts.
/// Rename edges are not inferred; the snapshot records none.
///
/// Store-only: `.velo/PARENT`, `MERGE_HEAD` and the working tree are untouched.
///
/// # Errors
/// [`VeloError::Conflicts`] (paths sorted) when a conflict has no resolution;
/// [`VeloError::InvalidInput`] when `theirs` is already contained in `ours`, a
/// resolution names a path the plan did not leave open, or a path is resolved
/// twice.
pub fn commit(guard: &WriteGuard, spec: MergeCommit<'_>) -> Result<SnapshotId> {
    use crate::tree::{Content, FileKind, SaveTree, TreeEntry};
    use std::collections::BTreeMap;

    let repo = guard.repo();
    if merge_base(repo, spec.ours, spec.theirs)?.as_ref() == Some(spec.theirs) {
        return Err(VeloError::invalid(format!(
            "{} already contains {}; there is nothing to merge.",
            spec.ours.short(),
            spec.theirs.short()
        )));
    }
    let plan = plan(repo, spec.ours, spec.theirs)?;

    let mut resolutions: BTreeMap<String, &Resolution> = BTreeMap::new();
    for (path, resolution) in spec.resolutions {
        let path = crate::db::normalise(path);
        if resolutions.insert(path.clone(), resolution).is_some() {
            return Err(VeloError::invalid(format!(
                "'{}' is resolved more than once.",
                path
            )));
        }
    }
    for path in resolutions.keys() {
        let open = plan.files.iter().any(|f| {
            f.path == *path
                && matches!(
                    f.change,
                    PlannedChange::Conflict { .. } | PlannedChange::KeepOurs
                )
        });
        if !open {
            return Err(VeloError::invalid(format!(
                "'{}' is neither conflicted nor kept from ours; it cannot be resolved.",
                path
            )));
        }
    }

    let mut unresolved: Vec<std::path::PathBuf> = plan
        .conflicts()
        .filter(|f| !resolutions.contains_key(&f.path))
        .map(|f| std::path::PathBuf::from(&f.path))
        .collect();
    if !unresolved.is_empty() {
        unresolved.sort();
        return Err(VeloError::Conflicts { paths: unresolved });
    }

    let mut tree: BTreeMap<String, TreeEntry> = repo
        .tree_at(spec.ours)?
        .into_iter()
        .map(|f| (f.path.clone(), TreeEntry::stored(f.path, f.object, f.kind)))
        .collect();
    // Their side must carry the same move remap `plan` used, or a conflict
    // reported at the new path would find nothing of theirs there.
    let (theirs_tree, their_moves): (BTreeMap<String, crate::tree::TreeFile>, Vec<_>) = {
        let base = merge_base(repo, spec.ours, spec.theirs)?;
        let mut base_tree =
            apply::load_tree(repo.conn(), base.as_ref().map_or("", |b| b.as_str()))?;
        let mut our_tree = apply::load_tree(repo.conn(), spec.ours.as_str())?;
        let mut their_tree = apply::load_tree(repo.conn(), spec.theirs.as_str())?;
        let moves = crate::commands::moves::follow_moves(
            repo.conn(),
            base.as_ref().map(|b| b.as_str()),
            spec.ours.as_str(),
            spec.theirs.as_str(),
            &mut base_tree,
            &mut our_tree,
            &mut their_tree,
        )?;
        let tree = their_tree
            .into_iter()
            .map(|(p, (object, mode))| {
                let file = crate::tree::TreeFile {
                    path: p.clone(),
                    object: crate::ids::ObjectHash::from_stored(object),
                    kind: FileKind::from_mode(mode),
                };
                (p, file)
            })
            .collect();
        (tree, moves)
    };
    // Our side gets the same remap as in `plan`: a file they moved sits at its
    // new path, so `Resolution::Ours` there keeps our content.
    for m in their_moves.iter().filter(|m| m.by == MovedBy::Theirs) {
        if let Some(mut entry) = tree.remove(&m.from) {
            entry.path = m.to.clone();
            tree.insert(m.to.clone(), entry);
        }
    }

    for file in &plan.files {
        let path = file.path.clone();
        match (&file.change, resolutions.get(&file.path)) {
            (PlannedChange::Conflict { .. } | PlannedChange::KeepOurs, Some(resolution)) => {
                match resolution {
                    Resolution::Ours => {}
                    Resolution::Theirs => match theirs_tree.get(&path) {
                        Some(t) => {
                            tree.insert(
                                path.clone(),
                                TreeEntry::stored(path, t.object.clone(), t.kind),
                            );
                        }
                        None => {
                            tree.remove(&path);
                        }
                    },
                    Resolution::Content(bytes) => {
                        let kind = tree
                            .get(&path)
                            .map(|e| e.kind)
                            .or_else(|| theirs_tree.get(&path).map(|t| t.kind))
                            .unwrap_or(FileKind::Regular);
                        tree.insert(
                            path.clone(),
                            TreeEntry {
                                path,
                                content: Content::Bytes(bytes.clone()),
                                kind,
                            },
                        );
                    }
                    Resolution::Delete => {
                        tree.remove(&path);
                    }
                }
            }
            (PlannedChange::KeepOurs, None) => {}
            (PlannedChange::Delete, _) => {
                tree.remove(&path);
            }
            (PlannedChange::Take { object, mode, .. }, _) => {
                tree.insert(
                    path.clone(),
                    TreeEntry::stored(path, object.clone(), FileKind::from_mode(*mode)),
                );
            }
            (PlannedChange::AutoMerge { content, mode }, _) => {
                tree.insert(
                    path.clone(),
                    TreeEntry {
                        path,
                        content: Content::Bytes(content.clone()),
                        kind: FileKind::from_mode(*mode),
                    },
                );
            }
            // Unresolved conflicts were rejected above.
            (PlannedChange::Conflict { .. }, None) => unreachable!("conflicts checked"),
        }
    }

    guard.save_tree(SaveTree {
        branch: spec.branch,
        parent: Some(spec.ours),
        merge_parent: Some(spec.theirs),
        message: spec.message,
        entries: tree.into_values().collect(),
        meta: spec.meta,
        author: spec.author,
        timestamp_ms: spec.timestamp_ms,
        renames: &[],
    })
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
/// `current`. Among equally near ones it drops any that another of them builds
/// on, then breaks the remaining tie by id so the answer is stable.
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

    let common: Vec<(&String, i64)> = ours
        .iter()
        .filter(|(hash, _)| theirs.contains_key(*hash))
        .map(|(hash, depth)| (hash, *depth))
        .collect();
    let nearest = common.iter().map(|(_, depth)| *depth).min()?;
    let mut tied: Vec<&String> = common
        .iter()
        .filter(|(_, depth)| *depth == nearest)
        .map(|(hash, _)| *hash)
        .collect();
    tied.sort();

    // Equally near is not equally good: a fast-forward snapshot has the old tip
    // and the target as parents, both at distance one, and the old tip is an
    // ancestor of the target. Of the tied candidates, drop any that another
    // one builds on, so the base is the newest of them whatever the ids are.
    if tied.len() > 1 {
        let mut dominated: std::collections::HashSet<&String> = std::collections::HashSet::new();
        for candidate in &tied {
            if let Ok(below) = crate::commands::ancestors(conn, candidate) {
                dominated.extend(
                    tied.iter()
                        .filter(|other| *other != candidate && below.contains_key(other.as_str()))
                        .copied(),
                );
            }
        }
        tied.retain(|hash| !dominated.contains(hash));
    }
    tied.first().map(|hash| (*hash).clone())
}
