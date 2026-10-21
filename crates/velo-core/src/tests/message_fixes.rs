//! Refusals that used to say the wrong thing now say what is actually wrong.

use std::fs;
use tempfile::TempDir;

use crate::commands::{merge, rebase, resolve, resolve_snapshot_id, save, switch};
use crate::error::VeloError;
use crate::Repo;

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn commit(repo: &Repo, body: &str, msg: &str) {
    fs::write(repo.root().join("f.txt"), body).unwrap();
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

fn go(repo: &Repo, branch: &str) {
    switch::run(&repo.write().unwrap(), branch, false).unwrap();
}

/// `main` and `feat` both edited `f.txt`; `feat` is checked out.
fn diverged() -> (TempDir, Repo) {
    let (tmp, repo) = setup();
    commit(&repo, "base\n", "base");
    go(&repo, "feat");
    commit(&repo, "feat\n", "feat edit");
    go(&repo, "main");
    commit(&repo, "main\n", "main edit");
    go(&repo, "feat");
    (tmp, repo)
}

#[test]
fn resolving_an_already_resolved_path_says_so() {
    let (_t, repo) = diverged();
    go(&repo, "main");
    merge::run(
        &repo.write().unwrap(),
        merge::Mode::Bring { source: "feat" },
    )
    .unwrap();
    resolve::clear_conflict(&repo.write().unwrap(), "f.txt").unwrap();
    let err = resolve::get_conflict(&repo, "f.txt").unwrap_err();
    assert!(
        matches!(&err, VeloError::InvalidInput { .. })
            && err.to_string().contains("not in conflict"),
        "{err:?}"
    );
    let missing = resolve::get_conflict(&repo, "nope.txt").unwrap_err();
    assert!(matches!(missing, VeloError::NotFound { .. }), "{missing:?}");
}

#[test]
fn rebase_continue_before_save_is_not_a_conflict_error() {
    let (_t, repo) = diverged();
    let onto = resolve_snapshot_id(&repo, "main").unwrap();
    let out = rebase::run(
        &repo.write().unwrap(),
        rebase::Mode::Start { onto: &onto },
        None,
    )
    .unwrap();
    assert!(matches!(out, rebase::Outcome::Paused { .. }), "{out:?}");
    resolve::clear_conflict(&repo.write().unwrap(), "f.txt").unwrap();
    let err = rebase::run(&repo.write().unwrap(), rebase::Mode::Continue, None).unwrap_err();
    assert!(!matches!(err, VeloError::Conflicts { .. }), "{err:?}");
    assert!(err.to_string().contains("velo save"), "{err}");
}

#[test]
fn merging_an_unborn_branch_says_it_has_no_commits() {
    let (_t, repo) = setup();
    commit(&repo, "base\n", "base");
    go(&repo, "fresh");
    go(&repo, "main");
    let err = merge::run(
        &repo.write().unwrap(),
        merge::Mode::Bring { source: "fresh" },
    )
    .unwrap_err();
    assert!(matches!(err, VeloError::UnbornBranch { .. }), "{err:?}");
}

#[test]
fn cherry_picking_a_change_already_present_updates_nothing() {
    let (_t, repo) = setup();
    let base = "a
b
c
d
e
f
g
";
    commit(&repo, base, "base");
    go(&repo, "feat");
    commit(
        &repo,
        &base.replacen(
            "a
", "A
", 1,
        ),
        "feat edit",
    );
    let feat = resolve_snapshot_id(&repo, "feat").unwrap();
    go(&repo, "main");
    // main made the same edit and another one elsewhere, so the 3-way merge is
    // clean but its result is byte-for-byte what main already has.
    let ours = base
        .replacen(
            "a
", "A
", 1,
        )
        .replacen(
            "g
", "G
", 1,
        );
    commit(&repo, &ours, "main edit");
    let out = crate::commands::cherry_pick::run(&repo.write().unwrap(), &feat, None).unwrap();
    assert_eq!(out.applied.updated(), 0, "{:?}", out.applied);
    assert!(out.applied.files.is_empty(), "{:?}", out.applied);
    assert!(out.saved_as.is_none());
    assert_eq!(fs::read_to_string(repo.root().join("f.txt")).unwrap(), ours);
}
