//! `velo clone` / `fetch` / `push` / `pull` — repository sync.
//!
//! Transport-agnostic: every verb talks to a [`transport::Remote`] obtained
//! from `transport::open(url)`, which is a local path (direct DB access), an
//! `ssh://…` URL, or a `child:…` local subprocess. The branch-namespacing and
//! `remote_refs` bookkeeping live here, on the client side.
//!
//! Model (deliberately Git-like):
//! - `clone` — copy all history, local branches = remote branches, checkout.
//! - `fetch` — import remote-only history under `remotes/<remote>/<branch>`
//!   tracking branches; update `remote_refs`. Never touches local branches or
//!   the working tree.
//! - `push` — fast-forward-only (the remote enforces it).
//! - `pull` — fetch the current branch, then fast-forward if behind, or (if
//!   diverged) leave `remotes/<remote>/<branch>` to `velo merge`.
//! - `sync` — one step: fetch, then push, fast-forward or merge as needed. See
//!   [`sync()`] and [`Synced`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::commands::require_working_tree;
use crate::commands::{branch_tip, bundle, get_dirty_files, remote as remotemod};
use crate::error::{InProgress, Result, VeloError};
use crate::progress::{Cancel, Observer, Phase, PhaseGuard, Silent};
use crate::storage;
use crate::transport::{self, PushOutcome};
use crate::Repo;
use crate::SnapshotId;
use crate::WriteGuard;

/// A branch the remote advertised, and where it points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteBranch {
    pub branch: String,
    pub hash: String,
}

/// What `clone` produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cloned {
    pub url: String,
    /// The directory the repository was created in.
    pub into: PathBuf,
    pub snapshots: usize,
    pub objects: usize,
    pub branches: usize,
    /// The branch that ended up checked out.
    pub branch: String,
}

/// What `fetch` imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub remote: String,
    pub snapshots: usize,
    pub objects: usize,
    /// Every branch the remote advertised, with its tip.
    pub refs: Vec<RemoteBranch>,
}

/// Why a push created a branch on the remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchCreated {
    /// The remote had no history at all; this push started it.
    RemoteWasEmpty,
    /// The remote had history but not this branch.
    BranchWasMissing,
}

/// What `push` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// The remote already had everything on this branch.
    AlreadyUpToDate { branch: String, remote: String },
    Sent {
        branch: String,
        remote: String,
        snapshots: usize,
        objects: usize,
        /// Set when this push brought the branch into existence there.
        created: Option<BranchCreated>,
    },
}

/// What `pull` did.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pulled {
    /// Local and remote already agree.
    AlreadyUpToDate { branch: String, remote: String },
    /// We were strictly behind, so the branch moved forward.
    FastForwarded {
        branch: String,
        remote: String,
        to: String,
    },
    /// Both sides have commits the other doesn't. The remote's history was
    /// imported under a tracking branch for the user to reconcile.
    Diverged {
        branch: String,
        remote: String,
        /// Snapshots only the local branch has.
        ahead: usize,
        /// Snapshots only the remote branch has.
        behind: usize,
    },
    /// The remote's tip is already part of our history: there is nothing to
    /// pull, and the local branch has `ahead` snapshots the remote lacks.
    Ahead {
        branch: String,
        remote: String,
        ahead: usize,
    },
}

// ─── clone ────────────────────────────────────────────────────────────────────

/// Where to put a clone, and how to watch it.
///
/// `observer` is owned here, unlike everywhere else: clone has no repository to
/// configure until it has created one, and the repository it configures is
/// created and dropped inside the call.
#[derive(Default)]
pub struct CloneOptions<'a> {
    /// Where to create the repository. `None` names a directory after the URL.
    pub dir: Option<&'a Path>,
    /// Where to report progress.
    pub observer: Option<Box<dyn Observer>>,
    /// Checked between transfer chunks. A cancelled clone leaves no repository
    /// behind: the directory is created only once the history has arrived, so
    /// stopping during the transfer — which is nearly all of the wait — cannot
    /// leave a half-cloned tree.
    pub cancel: Option<&'a Cancel>,
}

/// Copy a repository from `url`.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// use velo_core::commands::sync;
///
/// let spawn = velo_core::transport::Spawn::new("velo");
/// let cloned = sync::clone("velo://example.com/repo", &spawn, sync::CloneOptions {
///     dir: Some(std::path::Path::new("repo")),
///     ..Default::default()
/// })?;
/// # let _ = cloned;
/// # Ok(()) }
/// ```
pub fn clone(url: &str, spawn: &transport::Spawn, options: CloneOptions<'_>) -> Result<Cloned> {
    clone_with_token(url, spawn, options, None)
}

/// [`clone`] with a bearer token for an HTTP(S) source. The token comes from
/// the caller (never the URL) and is ignored by other transports.
pub fn clone_with_token(
    url: &str,
    spawn: &transport::Spawn,
    options: CloneOptions<'_>,
    token: Option<&str>,
) -> Result<Cloned> {
    let CloneOptions {
        dir,
        observer,
        cancel,
    } = options;
    let target = dir.map_or_else(|| PathBuf::from(default_dir(url)), PathBuf::from);
    if target.join(".velo").exists() {
        return Err(VeloError::invalid(format!(
            "'{}' already contains a Velo repository.",
            target.display()
        )));
    }

    // Pull everything from the remote (empty have-set = send it all).
    let mut remote = transport::open_with_token(url, spawn, token)?;
    let (refs, pack) = {
        // No repository exists yet, so the phase is driven from the observer the
        // caller handed us rather than from a `Repo`.
        let silent = Silent;
        let obs: &dyn Observer = match &observer {
            Some(o) => &**o,
            None => &silent,
        };
        let transfer = PhaseGuard::cancellable(obs, Phase::Transferring, None, cancel);
        remote.fetch(&HashSet::new(), &transfer)?
    };
    if refs.is_empty() {
        return Err(VeloError::invalid(format!(
            "'{}' has no commits yet, so there is nothing to clone.\n  \
             Publish to it first from a repository that has history:\n    \
             velo remote add origin {}\n    \
             velo push",
            url, url
        )));
    }

    std::fs::create_dir_all(&target)?;
    crate::commands::init::run(&target)?;
    crate::failpoint::hit("clone.after_init");

    // Clone creates the repository, so it opens the result and holds the write
    // lock for the import. Nothing else can be touching it yet, but the guard is
    // what grants write access at all.
    let repo = crate::Repo::open(&target)?;
    let repo = match observer {
        Some(o) => repo.observing(o),
        None => repo,
    };
    let guard = repo.write()?;
    let conn = guard.conn();

    let (snaps, objs) = bundle::import_pack(&guard, &pack)?;
    guard.repo().emit_imported(snaps);
    crate::failpoint::hit("clone.after_import");

    conn.execute(
        "INSERT OR REPLACE INTO remotes (name, url) VALUES ('origin', ?)",
        [url],
    )?;
    for r in &refs {
        remotemod::set_remote_ref(conn, "origin", &r.branch, &r.hash)?;
    }

    // Checkout: prefer 'main', else the first advertised branch.
    let default = refs
        .iter()
        .find(|r| r.branch == "main")
        .or_else(|| refs.first())
        .unwrap();
    storage::write_atomic(&target.join(".velo/HEAD"), default.branch.as_bytes())?;
    crate::commands::restore::run(
        &guard,
        &SnapshotId::from_stored(default.hash.as_str()),
        crate::commands::restore::Options {
            force: true,
            ..Default::default()
        },
    )?;

    let branch = default.branch.clone();
    // Release the repository before reporting: `target` is moved into the result,
    // and the caller shouldn't inherit a lock it didn't ask for.
    drop(guard);
    drop(repo);
    Ok(Cloned {
        url: url.to_string(),
        branch,
        into: target,
        snapshots: snaps,
        objects: objs,
        branches: refs.len(),
    })
}

/// How to run a transfer against a remote.
///
/// `fetch`, `push` and `pull` reported progress through the handle observer and
/// took no cancellation at all, so the operations that wait on a network were
/// the ones that could not be stopped.
#[derive(Default)]
pub struct Options<'a> {
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between transfer chunks.
    ///
    /// Cancelling stops the transfer and the import never runs, so nothing is
    /// written: the remote process is killed and the transaction is not opened.
    /// A cancelled `pull` may still have restored files if it is stopped after
    /// the merge begins, which `velo status` describes accurately.
    pub cancel: Option<&'a Cancel>,
}

impl Options<'_> {
    /// The phase these options ask for, falling back to the handle's observer.
    fn phase<'g>(&'g self, guard: &'g WriteGuard, phase: Phase) -> PhaseGuard<'g> {
        PhaseGuard::cancellable(
            self.observer.unwrap_or_else(|| guard.repo().observer()),
            phase,
            None,
            self.cancel,
        )
    }
}

// ─── fetch ────────────────────────────────────────────────────────────────────

/// Import a remote's history under `remotes/<remote>/<branch>` tracking
/// branches. Never touches local branches or the working tree.
pub fn fetch(
    guard: &WriteGuard,
    remote_name: &str,
    spawn: &transport::Spawn,
    options: Options<'_>,
) -> Result<Fetched> {
    let url = remote_url(guard.repo(), remote_name)?;
    let token = guard.repo().token_for_url(&url);
    let mut remote = transport::open_with_token(&url, spawn, token.as_deref())?;
    let have = local_snapshots(guard.repo())?;
    let (refs, mut pack) = {
        let transfer = options.phase(guard, Phase::Transferring);
        remote.fetch(&have, &transfer)?
    };

    // Namespace imported history under remotes/<remote>/<branch> so it never
    // collides with local branches. (Snapshots already present locally are
    // skipped by import, so only remote-only commits take the tracking name.)
    for s in &mut pack.snapshots {
        s.branch = format!("remotes/{}/{}", remote_name, s.branch);
    }
    let conn = guard.conn();
    // Tags, meta and renames only travel with their snapshots, so a pack with no
    // snapshots, objects or tags has nothing to import.
    let (snaps, objs) =
        if pack.snapshots.is_empty() && pack.objects.is_empty() && pack.tags.is_empty() {
            (0, 0)
        } else {
            bundle::import_pack(guard, &pack)?
        };
    guard.repo().emit_imported(snaps);
    for r in &refs {
        remotemod::set_remote_ref(conn, remote_name, &r.branch, &r.hash)?;
    }

    Ok(Fetched {
        remote: remote_name.to_string(),
        snapshots: snaps,
        objects: objs,
        refs: refs
            .iter()
            .map(|r| RemoteBranch {
                branch: r.branch.clone(),
                hash: r.hash.clone(),
            })
            .collect(),
    })
}

// ─── push ─────────────────────────────────────────────────────────────────────

/// What a push did, plus the side branches its history published.
///
/// A branch is a label stored on each snapshot and the remote derives its
/// branch tips from those labels, so pushing a branch that merged a scratch
/// branch also publishes the scratch branch under its own name. Relabelling
/// would risk moving the remote's tip of the pushed branch and would change
/// what older clients see, so the push says so instead.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PushReport {
    /// The outcome of the push itself.
    pub pushed: Pushed,
    /// Branch labels other than the pushed branch carried by the snapshots
    /// that were sent, sorted and deduplicated. Empty unless snapshots were sent.
    pub also_published: Vec<String>,
}

/// Send a branch to a remote. Fast-forward only — the remote enforces it.
pub fn push(
    guard: &WriteGuard,
    remote_name: &str,
    branch: Option<&str>,
    spawn: &transport::Spawn,
    options: Options<'_>,
) -> Result<Pushed> {
    push_report(guard, remote_name, branch, spawn, options).map(|r| r.pushed)
}

/// Like [`push`], but also reports the side branches the push published.
pub fn push_report(
    guard: &WriteGuard,
    remote_name: &str,
    branch: Option<&str>,
    spawn: &transport::Spawn,
    options: Options<'_>,
) -> Result<PushReport> {
    let conn = guard.conn();
    let branch = branch
        .map(String::from)
        .unwrap_or_else(|| guard.repo().head_branch().into_string());
    let local_tip = branch_tip(conn, &branch).ok_or_else(|| {
        VeloError::invalid(format!(
            "Local branch '{}' has no snapshots to push.",
            branch
        ))
    })?;

    let url = remote_url(guard.repo(), remote_name)?;
    let objects = guard.repo().objects();
    let token = guard.repo().token_for_url(&url);
    let mut remote = transport::open_with_token(&url, spawn, token.as_deref())?;

    // Build the pack once the remote has told us what it already has, so we
    // send only new commits and only objects it lacks. The advertised refs also
    // tell us whether we're creating the branch there for the first time.
    let created_branch = std::cell::Cell::new(false);
    let remote_was_empty = std::cell::Cell::new(false);
    let advertised_tip: std::cell::Cell<Option<String>> = std::cell::Cell::new(None);
    let side: std::cell::RefCell<std::collections::BTreeSet<String>> = Default::default();
    let mut build = |refs: &[transport::RemoteRef]| -> Result<bundle::Bundle> {
        advertised_tip.set(
            refs.iter()
                .find(|r| r.branch == branch)
                .map(|r| r.hash.clone()),
        );
        created_branch.set(!refs.iter().any(|r| r.branch == branch));
        remote_was_empty.set(refs.is_empty());
        let mut peer_has: HashSet<String> = HashSet::new();
        for r in refs {
            // Only walk tips we actually have locally; ancestry of an unknown
            // hash tells us nothing (and must not be claimed as "has").
            if snapshot_exists(conn, &r.hash) {
                peer_has.extend(bundle::reachable_ancestry(conn, &r.hash));
            }
        }
        let mut snap_set = bundle::reachable_ancestry(conn, &local_tip);
        for h in &peer_has {
            snap_set.remove(h);
        }
        let _packing = guard.phase(Phase::Packing, Some(snap_set.len() as u64));
        let mut pack = bundle::build_pack_excluding(conn, &objects, &snap_set, &peer_has)?;
        // Send commits under their real branch name: a commit we obtained via
        // `fetch` carries our local `remotes/<remote>/<branch>` label, which is
        // bookkeeping local to us and meaningless to the receiver.
        for s in &mut pack.snapshots {
            if let Some(rest) = s.branch.strip_prefix("remotes/") {
                if let Some((_, original)) = rest.split_once('/') {
                    s.branch = original.to_string();
                }
            }
            // Internal labels (`_stash`, `_deleted_*`) are not branches.
            if s.branch != branch && !s.branch.starts_with('_') {
                side.borrow_mut().insert(s.branch.clone());
            }
        }
        Ok(pack)
    };

    let outcome = {
        let transfer = options.phase(guard, Phase::Transferring);
        remote.push(&branch, &local_tip, &mut build, &transfer)?
    };
    match outcome {
        PushOutcome::Ok {
            new_snapshots,
            new_objects,
        } => {
            remotemod::set_remote_ref(conn, remote_name, &branch, &local_tip)?;
            if new_snapshots == 0 && new_objects == 0 {
                return Ok(PushReport {
                    pushed: Pushed::AlreadyUpToDate {
                        branch,
                        remote: remote_name.to_string(),
                    },
                    also_published: Vec::new(),
                });
            }
            let created = created_branch.get().then(|| {
                if remote_was_empty.get() {
                    BranchCreated::RemoteWasEmpty
                } else {
                    BranchCreated::BranchWasMissing
                }
            });
            Ok(PushReport {
                pushed: Pushed::Sent {
                    branch,
                    remote: remote_name.to_string(),
                    snapshots: new_snapshots,
                    objects: new_objects,
                    created,
                },
                also_published: side.take().into_iter().collect(),
            })
        }
        // The remote has commits we don't. `NotFastForward` carries the branch
        // and remote, so the CLI can suggest the fix rather than each call site
        // embedding it in a message.
        PushOutcome::Rejected(_) => {
            // Refresh the tracking ref with what the remote just told us, so
            // `status` stops advertising a push that will be refused.
            if let Some(tip) = advertised_tip.take() {
                remotemod::set_remote_ref(conn, remote_name, &branch, &tip)?;
            }
            Err(VeloError::NotFastForward {
                branch,
                remote: remote_name.to_string(),
            })
        }
    }
}

// ─── pull ─────────────────────────────────────────────────────────────────────

/// Fetch the current branch and fast-forward onto it, or report divergence.
pub fn pull(
    guard: &WriteGuard,
    remote_name: &str,
    spawn: &transport::Spawn,
    options: Options<'_>,
) -> Result<Pulled> {
    require_working_tree(guard.repo(), "pull")?;
    let dirty = get_dirty_files(guard.repo());
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }
    let branch = guard.repo().head_branch().into_string();
    let url = remote_url(guard.repo(), remote_name)?;

    let token = guard.repo().token_for_url(&url);
    let mut remote = transport::open_with_token(&url, spawn, token.as_deref())?;
    let have = local_snapshots(guard.repo())?;
    let (refs, pack) = {
        let transfer = options.phase(guard, Phase::Transferring);
        remote.fetch(&have, &transfer)?
    };

    let remote_tip = refs
        .iter()
        .find(|r| r.branch == branch)
        .map(|r| r.hash.clone())
        .ok_or_else(|| {
            VeloError::invalid(format!(
                "Remote '{}' has no branch '{}'.",
                remote_name, branch
            ))
        })?;

    let conn = guard.conn();
    let local_tip = branch_tip(conn, &branch);

    if local_tip.as_deref() == Some(remote_tip.as_str()) {
        remotemod::set_remote_ref(conn, remote_name, &branch, &remote_tip)?;
        return Ok(Pulled::AlreadyUpToDate {
            branch,
            remote: remote_name.to_string(),
        });
    }

    // Is the remote's tip already part of our history? Then we are strictly
    // ahead and there is nothing to bring in.
    if let Some(lt) = &local_tip {
        if transport::reaches(conn, &pack, lt, &remote_tip) {
            remotemod::set_remote_ref(conn, remote_name, &branch, &remote_tip)?;
            let ahead = crate::commands::ahead_behind(conn, lt, &remote_tip).0;
            return Ok(Pulled::Ahead {
                branch,
                remote: remote_name.to_string(),
                ahead,
            });
        }
    }

    // Is the pull a fast-forward? i.e. is our tip an ancestor of the remote's?
    // The walk must span (pack ∪ local history): the pack omits commits we
    // already have, and a previous `velo fetch` may have already imported the
    // remote's commits locally (leaving the pack empty).
    let is_ff = match &local_tip {
        None => true,
        Some(lt) => transport::reaches(conn, &pack, &remote_tip, lt),
    };

    if is_ff {
        let (snaps, _) = bundle::import_pack(guard, &pack)?;
        guard.repo().emit_imported(snaps);
        // A previous `velo fetch` may already have imported these commits under
        // the remote-tracking branch. Since Velo derives branch tips from the
        // `branch` column, the label has to move with the branch — otherwise the
        // fast-forward wouldn't actually advance `main`.
        adopt_tracking_commits(conn, remote_name, &branch, &remote_tip)?;
        remotemod::set_remote_ref(conn, remote_name, &branch, &remote_tip)?;
        crate::commands::restore::run(
            guard,
            &SnapshotId::from_stored(remote_tip.as_str()),
            crate::commands::restore::Options::default(),
        )?;
        Ok(Pulled::FastForwarded {
            branch,
            remote: remote_name.to_string(),
            to: remote_tip,
        })
    } else {
        // Diverged: import under a tracking branch and let the user reconcile.
        let mut pack = pack;
        for s in &mut pack.snapshots {
            s.branch = format!("remotes/{}/{}", remote_name, s.branch);
        }
        let conn = guard.conn();
        let (snaps, _) = bundle::import_pack(guard, &pack)?;
        guard.repo().emit_imported(snaps);
        remotemod::set_remote_ref(conn, remote_name, &branch, &remote_tip)?;
        let (ahead, behind) = match &local_tip {
            Some(lt) => crate::commands::ahead_behind(conn, lt, &remote_tip),
            None => (0, 0),
        };
        Ok(Pulled::Diverged {
            branch,
            remote: remote_name.to_string(),
            ahead,
            behind,
        })
    }
}

// ─── sync (one step) ──────────────────────────────────────────────────────────

/// What a one-step [`sync()`] did. It models what happened, not what to print.
///
/// | Situation after the fetch | Result |
/// |---|---|
/// | the remote has no such branch | [`Synced::Pushed`] (branch created there) |
/// | both tips are the same | [`Synced::Nothing`] |
/// | the remote's tip is in our history | [`Synced::Pushed`] |
/// | our tip is in the remote's history | [`Synced::FastForwarded`] |
/// | diverged, merge is clean | [`Synced::Merged`] (saved, then pushed) |
/// | diverged, merge conflicts | [`Synced::Conflicted`] (nothing pushed) |
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Synced {
    /// Local and remote already agree; nothing moved.
    Nothing { branch: String, remote: String },
    /// Local was ahead (or the remote lacked the branch) and was pushed.
    Pushed { report: PushReport },
    /// Local was behind; the branch and working tree moved to `to`.
    FastForwarded {
        branch: String,
        remote: String,
        to: SnapshotId,
    },
    /// Histories had diverged; the merge was clean, saved as `merge`, and pushed.
    Merged {
        branch: String,
        remote: String,
        merge: SnapshotId,
        report: PushReport,
    },
    /// Histories had diverged and the merge conflicts. The repository is left
    /// exactly as `velo merge` leaves it and nothing was pushed: resolve, save,
    /// and sync again.
    Conflicted {
        branch: String,
        remote: String,
        paths: Vec<PathBuf>,
    },
}

/// How to run [`sync()`]. A separate type from [`Options`], which is a plain
/// public struct that must not gain fields.
#[derive(Default)]
#[non_exhaustive]
pub struct SyncOptions<'a> {
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between transfer chunks.
    pub cancel: Option<&'a Cancel>,
    /// Who is recorded on a merge snapshot the sync creates.
    pub author: Option<&'a crate::Author>,
    /// The merge snapshot's message; defaults to `Merge <remote>/<branch> into <branch>`.
    pub message: Option<&'a str>,
}

impl<'a> SyncOptions<'a> {
    /// Report progress to `observer`.
    pub fn observer(mut self, observer: &'a dyn Observer) -> Self {
        self.observer = Some(observer);
        self
    }
    /// Allow the transfers to be cancelled.
    pub fn cancel(mut self, cancel: &'a Cancel) -> Self {
        self.cancel = Some(cancel);
        self
    }
    /// Record `author` on a merge snapshot.
    pub fn author(mut self, author: &'a crate::Author) -> Self {
        self.author = Some(author);
        self
    }
    /// Use `message` for a merge snapshot.
    pub fn message(mut self, message: &'a str) -> Self {
        self.message = Some(message);
        self
    }
}

/// Fetch, then push, fast-forward or merge as needed, under the caller's lock.
///
/// The outcomes are tabulated on [`Synced`]. A merge is saved and pushed only
/// when it is clean; a conflicted one stops in the state `velo merge` leaves, so
/// no side is ever chosen and nothing unseen is pushed. The push is
/// fast-forward-only: if the remote moved again, [`VeloError::NotFastForward`]
/// is returned and the local merge snapshot stays, so running `sync` again
/// continues. `branch` must be the checked-out branch (default), because a
/// merge needs the working tree.
///
/// A merge, rebase or cherry-pick in progress is [`VeloError::OperationInProgress`]
/// and unsaved changes are [`VeloError::DirtyWorkingTree`], both before any
/// network I/O.
pub fn sync(
    guard: &WriteGuard,
    remote_name: &str,
    branch: Option<&str>,
    spawn: &transport::Spawn,
    options: SyncOptions<'_>,
) -> Result<Synced> {
    use crate::commands::{merge, save};
    let repo = guard.repo();
    require_working_tree(repo, "sync")?;
    let root = guard.root();
    if root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
        });
    }
    if root.join(".velo/REBASE_STATE").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Rebase,
        });
    }
    let dirty = get_dirty_files(repo);
    if !dirty.is_empty() {
        let mut paths: Vec<PathBuf> = dirty.keys().map(PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }
    let head = repo.head_branch().into_string();
    let branch = match branch {
        Some(b) if b != head => {
            return Err(VeloError::invalid(format!(
                "Cannot sync branch '{}' while '{}' is checked out; switch to it first.",
                b, head
            )));
        }
        _ => head,
    };

    let transfer = || Options {
        observer: options.observer,
        cancel: options.cancel,
    };
    let fetched = fetch(guard, remote_name, spawn, transfer())?;
    let remote = remote_name.to_string();
    let conn = guard.conn();
    let push_now = || push_report(guard, remote_name, Some(&branch), spawn, transfer());

    let Some(remote_tip) = fetched
        .refs
        .iter()
        .find(|r| r.branch == branch)
        .map(|r| r.hash.clone())
    else {
        return Ok(Synced::Pushed {
            report: push_now()?,
        });
    };
    let local_tip = branch_tip(conn, &branch);
    let (ahead, behind) = match &local_tip {
        Some(lt) if *lt == remote_tip => {
            return Ok(Synced::Nothing { branch, remote });
        }
        Some(lt) => crate::commands::ahead_behind(conn, lt, &remote_tip),
        None => (0, 1),
    };

    if behind == 0 {
        return Ok(Synced::Pushed {
            report: push_now()?,
        });
    }
    if ahead == 0 {
        let to = fast_forward_to(guard, &remote, &branch, &remote_tip)?;
        return Ok(Synced::FastForwarded { branch, remote, to });
    }

    let source = format!("{}/{}", remote, branch);
    match merge::run(guard, merge::Mode::Bring { source: &source })? {
        merge::Outcome::Merged(tw) => {
            if !tw.is_clean() {
                let paths = tw.conflicts().into_iter().map(PathBuf::from).collect();
                return Ok(Synced::Conflicted {
                    branch,
                    remote,
                    paths,
                });
            }
            let default = format!("Merge {} into {}", source, branch);
            let message = options.message.unwrap_or(&default);
            let saved = save::run(
                guard,
                Some(message),
                save::Options {
                    author: options.author,
                    ..Default::default()
                },
            )?
            .into_result()
            .ok_or_else(|| VeloError::invalid("The clean merge produced no snapshot."))?;
            let report = push_now()?;
            Ok(Synced::Merged {
                branch,
                remote,
                merge: saved.hash,
                report,
            })
        }
        // Another writer cannot interleave under the lock, but the merge's own
        // classification is authoritative, so map it faithfully.
        merge::Outcome::FastForwarded { to, .. } => Ok(Synced::FastForwarded {
            branch,
            remote,
            to: SnapshotId::from_stored(to.as_str()),
        }),
        merge::Outcome::AlreadyUpToDate { .. } => Ok(Synced::Pushed {
            report: push_now()?,
        }),
        other => Err(VeloError::invalid(format!(
            "Unexpected merge outcome during sync: {:?}",
            other
        ))),
    }
}

/// Move `branch` and the working tree to `remote_tip`, which `fetch` has already
/// imported under the tracking name: relabel its commits, record the ref, restore.
fn fast_forward_to(
    guard: &WriteGuard,
    remote: &str,
    branch: &str,
    remote_tip: &str,
) -> Result<SnapshotId> {
    let conn = guard.conn();
    adopt_tracking_commits(conn, remote, branch, remote_tip)?;
    remotemod::set_remote_ref(conn, remote, branch, remote_tip)?;
    let to = SnapshotId::from_stored(remote_tip);
    crate::commands::restore::run(guard, &to, crate::commands::restore::Options::default())?;
    Ok(to)
}

// ─── helpers ────────────────────────────────────────────────────────────────────

fn remote_url(repo: &Repo, remote_name: &str) -> Result<String> {
    remotemod::remote_url(repo.conn(), remote_name)
}

fn snapshot_exists(conn: &rusqlite::Connection, hash: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
        [hash],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn local_snapshots(repo: &Repo) -> Result<HashSet<String>> {
    let conn = repo.conn();
    let mut stmt = conn.prepare("SELECT hash FROM snapshots")?;
    let set = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(set)
}

/// Re-label commits that a previous `fetch` parked on `remotes/<remote>/<branch>`
/// onto the local `branch`, for everything reachable from `remote_tip`.
///
/// Only the tracking branch for *this* branch is touched, so commits that
/// arrived on other tracking branches (e.g. a merged-in feature) keep their own
/// labels.
fn adopt_tracking_commits(
    conn: &rusqlite::Connection,
    remote_name: &str,
    branch: &str,
    remote_tip: &str,
) -> Result<()> {
    let tracking = format!("remotes/{}/{}", remote_name, branch);
    let reach = bundle::reachable_ancestry(conn, remote_tip);
    let mut stmt = conn.prepare("UPDATE snapshots SET branch = ? WHERE hash = ? AND branch = ?")?;
    for h in &reach {
        stmt.execute(rusqlite::params![branch, h, tracking])?;
    }
    Ok(())
}

pub(crate) fn default_dir(url: &str) -> String {
    let trimmed = url
        .trim_start_matches("ssh://")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("child:")
        .trim_end_matches(['/', '\\']);
    Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or("velo-clone")
        .to_string()
}

/// The `(remote, branch)` tracking refs that point at exactly `snapshot`,
/// ordered by remote then branch.
///
/// `undo` does not touch `remote_refs`, so after one a tracking ref can still
/// name the snapshot it removed; a caller uses this to say the remote still has
/// it and that a later `fetch` brings it back.
pub fn tracking_refs_at(repo: &Repo, snapshot: &SnapshotId) -> Result<Vec<(String, String)>> {
    let mut stmt = repo
        .conn()
        .prepare("SELECT remote, branch FROM remote_refs WHERE hash = ? ORDER BY remote, branch")?;
    let rows = stmt
        .query_map([snapshot.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Whether rewriting the snapshots from `tip` down to (but excluding) `base`
/// would replace history a remote-tracking ref already contains.
///
/// Returns the first `<remote>/<branch>` label, ordered by remote then branch,
/// whose last-known tip has any of those snapshots in its ancestry. `base` of
/// `None` means the whole history below `tip` is at stake. Read-only and
/// offline: it trusts `remote_refs`, so it can be stale, which is acceptable
/// for a warning.
pub fn published(
    repo: &Repo,
    tip: &SnapshotId,
    base: Option<&SnapshotId>,
) -> Result<Option<String>> {
    let conn = repo.conn();
    let mut at_stake = bundle::reachable_ancestry(conn, tip.as_ref());
    if let Some(base) = base {
        for h in bundle::reachable_ancestry(conn, base.as_ref()) {
            at_stake.remove(&h);
        }
    }
    let mut stmt =
        conn.prepare("SELECT remote, branch, hash FROM remote_refs ORDER BY remote, branch")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (remote, branch, hash) in rows {
        if !snapshot_exists(conn, &hash) {
            continue;
        }
        if bundle::reachable_ancestry(conn, &hash)
            .iter()
            .any(|h| at_stake.contains(h))
        {
            return Ok(Some(format!("{remote}/{branch}")));
        }
    }
    Ok(None)
}
