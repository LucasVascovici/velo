//! `rebase --abort` after some commits were replayed: the tree goes all the way
//! back, including files the new base added.

use std::fs;
use tempfile::TempDir;

use crate::commands::{rebase, resolve_snapshot_id, save, switch};
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

#[test]
fn abort_removes_new_base_files_after_a_replayed_commit() {
    let (_tmp, repo) = setup();
    commit(&repo, &[("shared.txt", "one\ntwo\nthree\n")], "base");
    switch::run(&repo.write().unwrap(), "feat", false).unwrap();
    commit(&repo, &[("r1.txt", "r1\n")], "feat one");
    commit(
        &repo,
        &[("shared.txt", "one\ntwo feat\nthree\n")],
        "feat two",
    );
    switch::run(&repo.write().unwrap(), "main", false).unwrap();
    commit(
        &repo,
        &[("m.txt", "m\n"), ("shared.txt", "one\ntwo main\nthree\n")],
        "main two",
    );
    switch::run(&repo.write().unwrap(), "feat", false).unwrap();
    let before_shared = fs::read(repo.root().join("shared.txt")).unwrap();
    let onto = resolve_snapshot_id(&repo, "main").unwrap();

    let g = repo.write().unwrap();
    let out = rebase::run(&g, rebase::Mode::Start { onto: &onto }, None).unwrap();
    assert!(matches!(out, rebase::Outcome::Paused { .. }), "{out:?}");
    assert!(repo.root().join("m.txt").exists());

    rebase::run(&g, rebase::Mode::Abort, None).unwrap();
    drop(g);
    assert!(
        !repo.root().join("m.txt").exists(),
        "main's file survived the abort"
    );
    assert!(repo.root().join("r1.txt").exists());
    assert_eq!(
        fs::read(repo.root().join("shared.txt")).unwrap(),
        before_shared
    );
}
