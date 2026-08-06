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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::commands::{branch_tip, bundle, get_dirty_files, remote as remotemod};
use crate::error::{Result, VeloError};
use crate::progress::{Observer, Phase, PhaseGuard, Silent};
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
    Diverged { branch: String, remote: String },
}

// ─── clone ────────────────────────────────────────────────────────────────────

/// Copy a repository from `url` into `dir` (or a directory named after the URL).
///
/// `observer` is a parameter here, unlike everywhere else: clone has no
/// repository to configure until it has created one. It takes ownership, since
/// the repository it configures is created and dropped inside this call.
pub fn clone(
    url: &str,
    dir: Option<&str>,
    spawn: &transport::Spawn,
    observer: Option<Box<dyn Observer>>,
) -> Result<Cloned> {
    let target = PathBuf::from(dir.map(String::from).unwrap_or_else(|| default_dir(url)));
    if target.join(".velo").exists() {
        return Err(VeloError::invalid(format!(
            "'{}' already contains a Velo repository.",
            target.display()
        )));
    }

    // Pull everything from the remote (empty have-set = send it all).
    let mut remote = transport::open(url, spawn)?;
    let (refs, pack) = {
        // No repository exists yet, so the phase is driven from the observer the
        // caller handed us rather than from a `Repo`.
        let silent = Silent;
        let obs: &dyn Observer = match &observer {
            Some(o) => &**o,
            None => &silent,
        };
        let transfer = PhaseGuard::new(obs, Phase::Transferring, None);
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

    let (snaps, objs) = bundle::import_pack(&guard, &target.join(".velo/objects"), &pack)?;

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

// ─── fetch ────────────────────────────────────────────────────────────────────

/// Import a remote's history under `remotes/<remote>/<branch>` tracking
/// branches. Never touches local branches or the working tree.
pub fn fetch(guard: &WriteGuard, remote_name: &str, spawn: &transport::Spawn) -> Result<Fetched> {
    let root = guard.root();
    let url = remote_url(guard.repo(), remote_name)?;
    let mut remote = transport::open(&url, spawn)?;
    let have = local_snapshots(guard.repo())?;
    let (refs, mut pack) = {
        let transfer = guard.phase(Phase::Transferring, None);
        remote.fetch(&have, &transfer)?
    };

    // Namespace imported history under remotes/<remote>/<branch> so it never
    // collides with local branches. (Snapshots already present locally are
    // skipped by import, so only remote-only commits take the tracking name.)
    for s in &mut pack.snapshots {
        s.branch = format!("remotes/{}/{}", remote_name, s.branch);
    }
    let conn = guard.conn();
    let (snaps, objs) = bundle::import_pack(guard, &root.join(".velo/objects"), &pack)?;
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

/// Send a branch to a remote. Fast-forward only — the remote enforces it.
pub fn push(
    guard: &WriteGuard,
    remote_name: &str,
    branch: Option<&str>,
    spawn: &transport::Spawn,
) -> Result<Pushed> {
    let root = guard.root();
    let conn = guard.conn();
    let branch = branch
        .map(String::from)
        .unwrap_or_else(|| current_branch(guard.root()));
    let local_tip = branch_tip(conn, &branch).ok_or_else(|| {
        VeloError::invalid(format!(
            "Local branch '{}' has no snapshots to push.",
            branch
        ))
    })?;

    let url = remote_url(guard.repo(), remote_name)?;
    let objects_dir = root.join(".velo/objects");
    let mut remote = transport::open(&url, spawn)?;

    // Build the pack once the remote has told us what it already has, so we
    // send only new commits and only objects it lacks. The advertised refs also
    // tell us whether we're creating the branch there for the first time.
    let created_branch = std::cell::Cell::new(false);
    let remote_was_empty = std::cell::Cell::new(false);
    let mut build = |refs: &[transport::RemoteRef]| -> Result<bundle::Bundle> {
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
        let mut pack = bundle::build_pack_excluding(conn, &objects_dir, &snap_set, &peer_has)?;
        // Send commits under their real branch name: a commit we obtained via
        // `fetch` carries our local `remotes/<remote>/<branch>` label, which is
        // bookkeeping local to us and meaningless to the receiver.
        for s in &mut pack.snapshots {
            if let Some(rest) = s.branch.strip_prefix("remotes/") {
                if let Some((_, original)) = rest.split_once('/') {
                    s.branch = original.to_string();
                }
            }
        }
        Ok(pack)
    };

    let outcome = {
        let transfer = guard.phase(Phase::Transferring, None);
        remote.push(&branch, &local_tip, &mut build, &transfer)?
    };
    match outcome {
        PushOutcome::Ok {
            new_snapshots,
            new_objects,
        } => {
            remotemod::set_remote_ref(conn, remote_name, &branch, &local_tip)?;
            if new_snapshots == 0 && new_objects == 0 {
                return Ok(Pushed::AlreadyUpToDate {
                    branch,
                    remote: remote_name.to_string(),
                });
            }
            let created = created_branch.get().then(|| {
                if remote_was_empty.get() {
                    BranchCreated::RemoteWasEmpty
                } else {
                    BranchCreated::BranchWasMissing
                }
            });
            Ok(Pushed::Sent {
                branch,
                remote: remote_name.to_string(),
                snapshots: new_snapshots,
                objects: new_objects,
                created,
            })
        }
        // The remote has commits we don't. `NotFastForward` carries the branch
        // and remote, so the CLI can suggest the fix rather than each call site
        // embedding it in a message.
        PushOutcome::Rejected(_) => Err(VeloError::NotFastForward {
            branch,
            remote: remote_name.to_string(),
        }),
    }
}

// ─── pull ─────────────────────────────────────────────────────────────────────

/// Fetch the current branch and fast-forward onto it, or report divergence.
pub fn pull(guard: &WriteGuard, remote_name: &str, spawn: &transport::Spawn) -> Result<Pulled> {
    let root = guard.root();
    let dirty = get_dirty_files(guard.repo());
    if !dirty.is_empty() {
        let mut paths: Vec<std::path::PathBuf> =
            dirty.keys().map(std::path::PathBuf::from).collect();
        paths.sort();
        return Err(VeloError::DirtyWorkingTree { paths });
    }
    let branch = current_branch(guard.root());
    let url = remote_url(guard.repo(), remote_name)?;

    let mut remote = transport::open(&url, spawn)?;
    let have = local_snapshots(guard.repo())?;
    let (refs, pack) = {
        let transfer = guard.phase(Phase::Transferring, None);
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

    // Is the pull a fast-forward? i.e. is our tip an ancestor of the remote's?
    // The walk must span (pack ∪ local history): the pack omits commits we
    // already have, and a previous `velo fetch` may have already imported the
    // remote's commits locally (leaving the pack empty).
    let is_ff = match &local_tip {
        None => true,
        Some(lt) => transport::reaches(conn, &pack, &remote_tip, lt),
    };

    if is_ff {
        bundle::import_pack(guard, &root.join(".velo/objects"), &pack)?;
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
        bundle::import_pack(guard, &root.join(".velo/objects"), &pack)?;
        remotemod::set_remote_ref(conn, remote_name, &branch, &remote_tip)?;
        Ok(Pulled::Diverged {
            branch,
            remote: remote_name.to_string(),
        })
    }
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

fn current_branch(root: &Path) -> String {
    std::fs::read_to_string(root.join(".velo/HEAD"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "main".into())
}

fn default_dir(url: &str) -> String {
    let trimmed = url
        .trim_start_matches("ssh://")
        .trim_start_matches("child:")
        .trim_end_matches(['/', '\\']);
    Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or("velo-clone")
        .to_string()
}
