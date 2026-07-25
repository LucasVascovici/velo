//! `velo clone` / `fetch` / `push` / `pull` — filesystem-remote sync.
//!
//! A remote is another Velo repository reachable by path. All four verbs reuse
//! the Phase-1 pack machinery (`bundle::build_pack` / `import_pack`): sync is
//! "build a pack of what's missing from one repo, import it into the other".
//!
//! Model (deliberately Git-like):
//! - `clone` — copy all history, local branches = remote branches, checkout.
//! - `fetch` — import remote-only history under `remotes/<remote>/<branch>`
//!   tracking branches; update `remote_refs`. Never touches local branches or
//!   the working tree.
//! - `push` — fast-forward-only: refuse if the remote branch has commits you
//!   don't; otherwise send your commits into the remote.
//! - `pull` — fetch the current branch, then fast-forward if you're behind, or
//!   (if diverged) leave `remotes/<remote>/<branch>` for you to
//!   `velo merge <remote>/<branch>`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use console::style;

use crate::commands::{
    all_branch_tips, branch_tip, bundle, get_dirty_files, remote as remotemod,
};
use crate::db;
use crate::error::{Result, VeloError};
use crate::{lock, storage};

// ─── clone ────────────────────────────────────────────────────────────────────

pub fn clone(url: &str, dir: Option<&str>) -> Result<()> {
    let remote_root = remotemod::resolve_remote_root(url)?;
    let target = PathBuf::from(dir.map(String::from).unwrap_or_else(|| default_dir(url)));

    if target.join(".velo").exists() {
        return Err(VeloError::InvalidInput(format!(
            "'{}' already contains a Velo repository.",
            target.display()
        )));
    }
    std::fs::create_dir_all(&target)?;
    crate::commands::init::run(&target)?;

    println!("Cloning {} → {}…", style(url).cyan(), style(target.display()).cyan());

    let remote_conn = db::get_conn_at_path(&remote_root.join(".velo/velo.db"))?;
    let remote_objects = remote_root.join(".velo/objects");
    let remote_tips = all_branch_tips(&remote_conn);
    if remote_tips.is_empty() {
        return Err(VeloError::InvalidInput("Remote has no branches to clone.".into()));
    }

    // Pack all history reachable from every remote branch, import as-is so the
    // remote branches become local branches.
    let mut snap_set: HashSet<String> = HashSet::new();
    for (_b, tip) in &remote_tips {
        snap_set.extend(bundle::reachable_ancestry(&remote_conn, tip));
    }
    let pack = bundle::build_pack(&remote_conn, &remote_objects, &snap_set)?;

    let mut conn = db::get_conn_at_path(&target.join(".velo/velo.db"))?;
    let (snaps, objs) = bundle::import_pack(&mut conn, &target.join(".velo/objects"), &pack)?;

    // Record the remote and its tracking refs.
    conn.execute(
        "INSERT OR REPLACE INTO remotes (name, url) VALUES ('origin', ?)",
        [url],
    )?;
    for (b, tip) in &remote_tips {
        remotemod::set_remote_ref(&conn, "origin", b, tip)?;
    }

    // Checkout the remote's default branch.
    let default_branch = std::fs::read_to_string(remote_root.join(".velo/HEAD"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "main".into());
    let default_branch = if default_branch.is_empty() { "main".into() } else { default_branch };

    if let Some(tip) = branch_tip(&conn, &default_branch) {
        storage::write_atomic(&target.join(".velo/HEAD"), default_branch.as_bytes())?;
        crate::commands::restore::run(&target, &tip, true, &[])?;
    }

    println!(
        "{} Cloned {} snapshot(s), {} object(s), {} branch(es) into {}",
        style("✔").green().bold(),
        snaps,
        objs,
        remote_tips.len(),
        style(target.display()).cyan()
    );
    Ok(())
}

// ─── fetch ────────────────────────────────────────────────────────────────────

pub fn fetch(root: &Path, remote_name: &str) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let url = remotemod::remote_url(&conn, remote_name)?;
    let remote_root = remotemod::resolve_remote_root(&url)?;
    let remote_conn = db::get_conn_at_path(&remote_root.join(".velo/velo.db"))?;
    let remote_objects = remote_root.join(".velo/objects");

    let remote_tips = all_branch_tips(&remote_conn);
    let mut snap_set: HashSet<String> = HashSet::new();
    for (_b, tip) in &remote_tips {
        snap_set.extend(bundle::reachable_ancestry(&remote_conn, tip));
    }

    // Namespace imported history under remotes/<remote>/<branch> so it never
    // collides with local branches. (Shared snapshots already present locally
    // are skipped by import, so only remote-only commits get the tracking name.)
    let mut pack = bundle::build_pack(&remote_conn, &remote_objects, &snap_set)?;
    for s in &mut pack.snapshots {
        s.branch = format!("remotes/{}/{}", remote_name, s.branch);
    }

    let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let (snaps, objs) = bundle::import_pack(&mut conn, &root.join(".velo/objects"), &pack)?;
    for (b, tip) in &remote_tips {
        remotemod::set_remote_ref(&conn, remote_name, b, tip)?;
    }

    println!(
        "{} Fetched from '{}' — {} new snapshot(s), {} object(s).",
        style("✔").green().bold(),
        remote_name,
        snaps,
        objs
    );
    for (b, tip) in &remote_tips {
        println!("  {}/{}  →  {}", remote_name, style(b).cyan(), style(&tip[..12.min(tip.len())]).yellow());
    }
    Ok(())
}

// ─── push ─────────────────────────────────────────────────────────────────────

pub fn push(root: &Path, remote_name: &str, branch: Option<&str>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let branch = match branch {
        Some(b) => b.to_string(),
        None => current_branch(root),
    };
    let url = remotemod::remote_url(&conn, remote_name)?;
    let remote_root = remotemod::resolve_remote_root(&url)?;

    let local_tip = branch_tip(&conn, &branch).ok_or_else(|| {
        VeloError::InvalidInput(format!("Local branch '{}' has no snapshots to push.", branch))
    })?;

    // Hold the remote's lock for the whole read-check-then-write.
    let _remote_lock = lock::RepoLock::acquire(&remote_root)?;
    let mut remote_conn = db::get_conn_at_path(&remote_root.join(".velo/velo.db"))?;
    let remote_objects = remote_root.join(".velo/objects");
    let remote_tip = branch_tip(&remote_conn, &branch);

    if remote_tip.as_deref() == Some(local_tip.as_str()) {
        println!("{} '{}' is already up to date on '{}'.", style("✔").green(), branch, remote_name);
        return Ok(());
    }

    // Fast-forward-only: the remote's tip (if any) must be in our ancestry.
    if let Some(rt) = &remote_tip {
        let local_reach = bundle::reachable_ancestry(&conn, &local_tip);
        if !local_reach.contains(rt) {
            return Err(VeloError::InvalidInput(format!(
                "Push rejected — '{}' on '{}' has commits you don't have (non-fast-forward).\n  \
                 Run 'velo pull {}' and reconcile, then push again.",
                branch, remote_name, remote_name
            )));
        }
    }

    let snap_set = bundle::reachable_ancestry(&conn, &local_tip);
    let pack = bundle::build_pack(&conn, &root.join(".velo/objects"), &snap_set)?;
    let (snaps, objs) = bundle::import_pack(&mut remote_conn, &remote_objects, &pack)?;

    // We now know the remote is at our tip.
    remotemod::set_remote_ref(&conn, remote_name, &branch, &local_tip)?;

    println!(
        "{} Pushed '{}' to '{}' — {} snapshot(s), {} object(s).",
        style("✔").green().bold(),
        branch,
        remote_name,
        snaps,
        objs
    );
    println!(
        "  {} the remote's working tree is unchanged; it updates on its next {} or {}.",
        style("note:").dim(),
        style("velo pull").cyan(),
        style("velo restore").cyan()
    );
    Ok(())
}

// ─── pull ─────────────────────────────────────────────────────────────────────

pub fn pull(root: &Path, remote_name: &str) -> Result<()> {
    if !get_dirty_files(root).is_empty() {
        return Err(VeloError::InvalidInput(
            "Pull aborted: you have unsaved changes. Save or discard them first.".into(),
        ));
    }
    let branch = current_branch(root);
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let url = remotemod::remote_url(&conn, remote_name)?;
    let remote_root = remotemod::resolve_remote_root(&url)?;
    let remote_conn = db::get_conn_at_path(&remote_root.join(".velo/velo.db"))?;
    let remote_objects = remote_root.join(".velo/objects");

    let remote_tip = branch_tip(&remote_conn, &branch).ok_or_else(|| {
        VeloError::InvalidInput(format!("Remote '{}' has no branch '{}'.", remote_name, branch))
    })?;
    let local_tip = branch_tip(&conn, &branch);

    if local_tip.as_deref() == Some(remote_tip.as_str()) {
        remotemod::set_remote_ref(&conn, remote_name, &branch, &remote_tip)?;
        println!("{} Already up to date.", style("✔").green());
        return Ok(());
    }

    let remote_reach = bundle::reachable_ancestry(&remote_conn, &remote_tip);
    let is_ff = match &local_tip {
        None => true,
        Some(lt) => remote_reach.contains(lt),
    };

    if is_ff {
        // Import the remote history onto the local branch and move to its tip.
        let pack = bundle::build_pack(&remote_conn, &remote_objects, &remote_reach)?;
        let mut conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        bundle::import_pack(&mut conn, &root.join(".velo/objects"), &pack)?;
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
        // Diverged: bring the remote history in as a tracking branch and let the
        // user reconcile with the existing merge engine.
        let mut pack = bundle::build_pack(&remote_conn, &remote_objects, &remote_reach)?;
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
        println!(
            "  or replay your work with {}",
            style(format!("velo rebase {}/{}", remote_name, branch)).cyan()
        );
    }
    Ok(())
}

// ─── helpers ────────────────────────────────────────────────────────────────────

fn current_branch(root: &Path) -> String {
    std::fs::read_to_string(root.join(".velo/HEAD"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "main".into())
}

fn default_dir(url: &str) -> String {
    let trimmed = url.trim_end_matches(['/', '\\']);
    Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or("velo-clone")
        .to_string()
}
