//! switch, whole-tree restore and stash pop refuse while a merge or rebase is
//! unfinished, and `--force` and path-limited restore remain the way through.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::{merge, rebase, resolve_snapshot_id, restore, save, stash, switch};
use crate::error::{InProgress, VelodyError};
use crate::Repo;

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn commit(repo: &Repo, files: &[(&str, &str)], msg: &str) {
    for (p, body) in files {
        fs::write(repo.root().join(p), body).unwrap();
    }
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

fn go(repo: &Repo, branch: &str) -> crate::Result<switch::Outcome> {
    switch::run(&repo.write().unwrap(), branch, false)
}

/// `main` and `feat` both edited `f.txt`; a branch `other` exists at the base.
/// Returns with `main` checked out and `feat` not yet merged.
fn diverged() -> (TempDir, Repo) {
    let (tmp, repo) = setup();
    commit(&repo, &[("f.txt", "base\n")], "base");
    go(&repo, "other").unwrap();
    go(&repo, "main").unwrap();
    go(&repo, "feat").unwrap();
    commit(&repo, &[("f.txt", "feat\n")], "feat edit");
    go(&repo, "main").unwrap();
    commit(&repo, &[("f.txt", "main\n")], "main edit");
    (tmp, repo)
}

fn bring_feat(repo: &Repo) -> merge::Outcome {
    merge::run(
        &repo.write().unwrap(),
        merge::Mode::Bring { source: "feat" },
    )
    .unwrap()
}

fn conflicted_merge() -> (TempDir, Repo) {
    let (tmp, repo) = diverged();
    assert!(
        matches!(bring_feat(&repo), merge::Outcome::Merged(t) if !t.is_clean()),
        "expected a conflict"
    );
    (tmp, repo)
}

fn state(root: &Path) -> (String, String, Vec<u8>) {
    let r = |p: &str| fs::read_to_string(root.join(p)).unwrap_or_default();
    (
        r(".velo/HEAD"),
        r(".velo/PARENT"),
        fs::read(root.join("f.txt")).unwrap(),
    )
}

fn is_merge(e: &VelodyError) -> bool {
    matches!(
        e,
        VelodyError::OperationInProgress {
            what: InProgress::Merge
        }
    )
}

#[test]
fn switch_mid_merge_refuses_and_changes_nothing() {
    let (_t, repo) = conflicted_merge();
    let before = state(repo.root());
    let err = go(&repo, "other").unwrap_err();
    assert!(is_merge(&err), "{err:?}");
    assert_eq!(state(repo.root()), before);
    assert!(repo.root().join(".velo/MERGE_HEAD").exists());
}

#[test]
fn switch_to_the_same_branch_mid_merge_refuses_too() {
    let (_t, repo) = conflicted_merge();
    // A stale position: the position no longer equals the branch tip.
    fs::write(repo.root().join(".velo/PARENT"), "").unwrap();
    let err = go(&repo, "main").unwrap_err();
    assert!(is_merge(&err), "{err:?}");
}

#[test]
fn switch_force_mid_merge_proceeds_as_before() {
    let (_t, repo) = conflicted_merge();
    let out = switch::run(&repo.write().unwrap(), "other", true);
    assert!(out.is_ok(), "{out:?}");
    let head = fs::read_to_string(repo.root().join(".velo/HEAD")).unwrap();
    assert_eq!(head.trim(), "other");
}

#[test]
fn whole_tree_restore_refuses_but_a_path_restore_is_allowed() {
    let (_t, repo) = conflicted_merge();
    let base = resolve_snapshot_id(&repo, "feat").unwrap();
    let err = restore::run(&repo.write().unwrap(), &base, restore::Options::default()).unwrap_err();
    assert!(is_merge(&err), "{err:?}");

    let paths = [Path::new("f.txt")];
    restore::run(
        &repo.write().unwrap(),
        &base,
        restore::Options {
            force: true,
            paths: &paths,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(repo.root().join("f.txt")).unwrap(),
        "feat
"
    );

    // Without force the conflicted file is dirty and may be refused for that,
    // but never by the in-progress guard.
    let res = restore::run(
        &repo.write().unwrap(),
        &base,
        restore::Options {
            paths: &paths,
            ..Default::default()
        },
    );
    if let Err(e) = res {
        assert!(!is_merge(&e), "path restore hit the guard: {e:?}");
    }
}

#[test]
fn stash_pop_refuses_mid_merge() {
    let (_t, repo) = diverged();
    fs::write(repo.root().join("scratch.txt"), "wip\n").unwrap();
    stash::push(&repo.write().unwrap(), None).unwrap();
    assert!(matches!(bring_feat(&repo), merge::Outcome::Merged(t) if !t.is_clean()));
    let err = stash::pop(&repo.write().unwrap(), None).unwrap_err();
    assert!(is_merge(&err), "{err:?}");
    assert!(!repo.root().join("scratch.txt").exists());
}

#[test]
fn switch_mid_rebase_names_the_rebase() {
    let (_t, repo) = diverged();
    go(&repo, "feat").unwrap();
    let onto = resolve_snapshot_id(&repo, "main").unwrap();
    let out = rebase::run(
        &repo.write().unwrap(),
        rebase::Mode::Start { onto: &onto },
        None,
    )
    .unwrap();
    assert!(matches!(out, rebase::Outcome::Paused { .. }), "{out:?}");
    let err = go(&repo, "other").unwrap_err();
    assert!(
        matches!(
            err,
            VelodyError::OperationInProgress {
                what: InProgress::Rebase
            }
        ),
        "{err:?}"
    );
}

#[test]
fn merge_abort_after_a_refused_switch_returns_to_the_start() {
    let (_t, repo) = diverged();
    let start = fs::read_to_string(repo.root().join(".velo/PARENT")).unwrap();
    bring_feat(&repo);
    go(&repo, "other").unwrap_err();
    merge::run(&repo.write().unwrap(), merge::Mode::Abort).unwrap();
    let head = fs::read_to_string(repo.root().join(".velo/HEAD")).unwrap();
    assert_eq!(head.trim(), "main");
    let parent = fs::read_to_string(repo.root().join(".velo/PARENT")).unwrap();
    assert_eq!(parent, start);
    assert_eq!(
        fs::read_to_string(repo.root().join("f.txt")).unwrap(),
        "main\n"
    );
}
