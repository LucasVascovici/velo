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

use console::style;

use crate::commands::{branch_tip, bundle, get_dirty_files, remote as remotemod};
use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;
use crate::transport::{self, PushOutcome};

// ─── clone ────────────────────────────────────────────────────────────────────

pub fn clone(url: &str, dir: Option<&str>) -> Result<()> {
    let target = PathBuf::from(dir.map(String::from).unwrap_or_else(|| default_dir(url)));
    if target.join(".velo").exists() {
        return Err(VeloError::InvalidInput(format!(
            "'{}' already contains a Velo repository.",
            target.display()
        )));
    }

    println!(
        "Cloning {} → {}…",
        style(url).cyan(),
        style(target.display()).cyan()
    );

    // Pull everything from the remote (empty have-set = send it all).
    let mut remote = transport::open(url)?;
    let (refs, pack) = remote.fetch(&HashSet::new())?;
    if refs.is_empty() {
        return Err(VeloError::InvalidInput(format!(
            "'{}' has no commits yet, so there is nothing to clone.\n  \
             Publish to it first from a repository that has history:\n    \
             velo remote add origin {}\n    \
             velo push",
            url, url
        )));
    }

    std::fs::create_dir_all(&target)?;
    crate::commands::init::run(&target)?;
    let mut conn = db::get_conn_at_path(&target.join(".velo/velo.db"))?;
    let (snaps, objs) = bundle::import_pack(&mut conn, &target.join(".velo/objects"), &pack)?;

    conn.execute(
        "INSERT OR REPLACE INTO remotes (name, url) VALUES ('origin', ?)",
        [url],
    )?;
    for r in &refs {
        remotemod::set_remote_ref(&conn, "origin", &r.branch, &r.hash)?;
    }

    // Checkout: prefer 'main', else the first advertised branch.
    let default = refs
        .iter()
        .find(|r| r.branch == "main")
        .or_else(|| refs.first())
        .unwrap();
    storage::write_atomic(&target.join(".velo/HEAD"), default.branch.as_bytes())?;
    crate::commands::restore::run(&target, &default.hash, true, &[])?;

    println!(
        "{} Cloned {} snapshot(s), {} object(s), {} branch(es) into {}",
        style("✔").green().bold(),
        snaps,
        objs,
        refs.len(),
        style(target.display()).cyan()
    );
    Ok(())
}

// ─── fetch ────────────────────────────────────────────────────────────────────

pub fn fetch(root: &Path, remote_name: &str) -> Result<()> {
    let url = remote_url(root, remote_name)?;
    let mut remote = transport::open(&url)?;
    let have = local_snapshots(root)?;
    let (refs, mut pack) = remote.fetch(&have)?;

    // Namespace imported history under remotes/<remote>/<branch> so it never
    // collides with local branches. (Snapshots already present locally are
    // skipped by import, so only remote-only commits take the tracking name.)
    for s in &mut pack.snapshots {
        s.branch = format!("remotes/{}/{}", remote_name, s.branch);
    }
    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let (snaps, objs) = bundle::import_pack(&mut conn, &root.join(".velo/objects"), &pack)?;
    for r in &refs {
        remotemod::set_remote_ref(&conn, remote_name, &r.branch, &r.hash)?;
    }

    println!(
        "{} Fetched from '{}' — {} new snapshot(s), {} object(s).",
        style("✔").green().bold(),
        remote_name,
        snaps,
        objs
    );
    for r in &refs {
        println!(
            "  {}/{}  →  {}",
            remote_name,
            style(&r.branch).cyan(),
            style(&r.hash[..12.min(r.hash.len())]).yellow()
        );
    }
    Ok(())
}

// ─── push ─────────────────────────────────────────────────────────────────────

pub fn push(root: &Path, remote_name: &str, branch: Option<&str>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch = branch
        .map(String::from)
        .unwrap_or_else(|| current_branch(root));
    let local_tip = branch_tip(&conn, &branch).ok_or_else(|| {
        VeloError::InvalidInput(format!(
            "Local branch '{}' has no snapshots to push.",
            branch
        ))
    })?;

    let url = remote_url(root, remote_name)?;
    let objects_dir = root.join(".velo/objects");
    let mut remote = transport::open(&url)?;

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
            if snapshot_exists(&conn, &r.hash) {
                peer_has.extend(bundle::reachable_ancestry(&conn, &r.hash));
            }
        }
        let mut snap_set = bundle::reachable_ancestry(&conn, &local_tip);
        for h in &peer_has {
            snap_set.remove(h);
        }
        let mut pack = bundle::build_pack_excluding(&conn, &objects_dir, &snap_set, &peer_has)?;
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

    match remote.push(&branch, &local_tip, &mut build)? {
        PushOutcome::Ok {
            new_snapshots,
            new_objects,
        } => {
            remotemod::set_remote_ref(&conn, remote_name, &branch, &local_tip)?;
            if new_snapshots == 0 && new_objects == 0 {
                println!(
                    "{} '{}' is already up to date on '{}'.",
                    style("✔").green(),
                    branch,
                    remote_name
                );
            } else {
                println!(
                    "{} Pushed '{}' to '{}' — {} snapshot(s), {} object(s).",
                    style("✔").green().bold(),
                    branch,
                    remote_name,
                    new_snapshots,
                    new_objects
                );
                if created_branch.get() {
                    let what = if remote_was_empty.get() {
                        "was empty; it now has its first history"
                    } else {
                        "did not have this branch; it was created there"
                    };
                    println!(
                        "  {} '{}' {}. Others can now {}.",
                        style("note:").dim(),
                        remote_name,
                        what,
                        style(format!("velo clone <url> / velo pull {}", remote_name)).cyan()
                    );
                }
                println!(
                    "  {} the remote's working tree is unchanged; it updates on its next {}.",
                    style("note:").dim(),
                    style("velo pull").cyan()
                );
            }
            Ok(())
        }
        PushOutcome::Rejected(reason) => Err(VeloError::InvalidInput(format!(
            "Push rejected — {}\n  Run 'velo pull {}' and reconcile, then push again.",
            reason, remote_name
        ))),
    }
}

// ─── pull ─────────────────────────────────────────────────────────────────────

pub fn pull(root: &Path, remote_name: &str) -> Result<()> {
    if !get_dirty_files(root).is_empty() {
        return Err(VeloError::InvalidInput(
            "Pull aborted: you have unsaved changes. Save or discard them first.".into(),
        ));
    }
    let branch = current_branch(root);
    let url = remote_url(root, remote_name)?;

    let mut remote = transport::open(&url)?;
    let have = local_snapshots(root)?;
    let (refs, pack) = remote.fetch(&have)?;

    let remote_tip = refs
        .iter()
        .find(|r| r.branch == branch)
        .map(|r| r.hash.clone())
        .ok_or_else(|| {
            VeloError::InvalidInput(format!(
                "Remote '{}' has no branch '{}'.",
                remote_name, branch
            ))
        })?;

    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let local_tip = branch_tip(&conn, &branch);

    if local_tip.as_deref() == Some(remote_tip.as_str()) {
        remotemod::set_remote_ref(&conn, remote_name, &branch, &remote_tip)?;
        println!("{} Already up to date.", style("✔").green());
        return Ok(());
    }

    // Is the pull a fast-forward? i.e. is our tip an ancestor of the remote's?
    // The walk must span (pack ∪ local history): the pack omits commits we
    // already have, and a previous `velo fetch` may have already imported the
    // remote's commits locally (leaving the pack empty).
    let is_ff = match &local_tip {
        None => true,
        Some(lt) => transport::reaches(&conn, &pack, &remote_tip, lt),
    };
    drop(conn);

    if is_ff {
        let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        bundle::import_pack(&mut conn, &root.join(".velo/objects"), &pack)?;
        // A previous `velo fetch` may already have imported these commits under
        // the remote-tracking branch. Since Velo derives branch tips from the
        // `branch` column, the label has to move with the branch — otherwise the
        // fast-forward wouldn't actually advance `main`.
        adopt_tracking_commits(&conn, remote_name, &branch, &remote_tip)?;
        remotemod::set_remote_ref(&conn, remote_name, &branch, &remote_tip)?;
        drop(conn);
        crate::commands::restore::run(root, &remote_tip, false, &[])?;
        println!(
            "{} Fast-forwarded '{}' to {}.",
            style("✔").green().bold(),
            branch,
            style(&remote_tip[..12.min(remote_tip.len())]).yellow()
        );
    } else {
        // Diverged: import under a tracking branch and let the user reconcile.
        let mut pack = pack;
        for s in &mut pack.snapshots {
            s.branch = format!("remotes/{}/{}", remote_name, s.branch);
        }
        let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        bundle::import_pack(&mut conn, &root.join(".velo/objects"), &pack)?;
        remotemod::set_remote_ref(&conn, remote_name, &branch, &remote_tip)?;
        println!(
            "{} '{}' and '{}/{}' have diverged.",
            style("!").yellow().bold(),
            branch,
            remote_name,
            branch
        );
        println!(
            "  Reconcile with {} then {}",
            style(format!("velo merge {}/{}", remote_name, branch)).cyan(),
            style("velo save \"Merge …\"").cyan()
        );
    }
    Ok(())
}

// ─── helpers ────────────────────────────────────────────────────────────────────

fn remote_url(root: &Path, remote_name: &str) -> Result<String> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    remotemod::remote_url(&conn, remote_name)
}

fn snapshot_exists(conn: &rusqlite::Connection, hash: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
        [hash],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

fn local_snapshots(root: &Path) -> Result<HashSet<String>> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
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
