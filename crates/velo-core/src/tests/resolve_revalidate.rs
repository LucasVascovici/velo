//! Interactive resolve takes the lock per decision, so core re-checks the
//! conflict under each short lock and refuses writes after an abort.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::resolve;
use crate::commands::{self, merge, save, switch};
use crate::merge::Decision;
use crate::{Error, Repo};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn write(root: &Path, body: &str) {
    fs::write(root.join("a.txt"), body).unwrap();
}

fn save_as(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

fn go(repo: &Repo, branch: &str, force: bool) {
    let g = repo.write().unwrap();
    switch::run(&g, branch, force).unwrap();
}

fn bring_feature(repo: &Repo) {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source: "feature" }).unwrap();
}

fn abort(repo: &Repo) {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Abort).unwrap();
}

/// `a.txt` conflicts: main says "main", feature says "feature".
fn conflicted(root: &Path, repo: &Repo) {
    write(root, "one\n");
    save_as(repo, "base");
    go(repo, "feature", false);
    write(root, "feature\n");
    save_as(repo, "feature edit");
    go(repo, "main", true);
    write(root, "main\n");
    save_as(repo, "main edit");
    bring_feature(repo);
    assert_eq!(resolve::conflict_count(repo), 1);
}

fn count(repo: &Repo, table: &str) -> i64 {
    repo.conn()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn open(repo: &Repo) -> resolve::ConflictSession {
    let cf = resolve::get_conflict(repo, "a.txt").unwrap();
    resolve::open_session(repo, cf).unwrap()
}

#[test]
fn finalise_after_abort_writes_nothing() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let mut session = open(&repo);
    session.hunks[0].decision = Some(Decision::Theirs);
    abort(&repo);
    let before = fs::read(tmp.path().join("a.txt")).unwrap();
    assert_eq!(before, b"main\n");

    let g = repo.write().unwrap();
    let err = resolve::finalise(&g, &session).unwrap_err();
    drop(g);
    assert!(
        matches!(err, Error::NoOperationInProgress { .. }),
        "{err:?}"
    );
    assert_eq!(fs::read(tmp.path().join("a.txt")).unwrap(), before);
    assert_eq!(count(&repo, "conflict_files"), 0);
    assert_eq!(count(&repo, "hunk_decisions"), 0);
}

#[test]
fn record_and_clear_after_abort_are_not_found() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    abort(&repo);
    let g = repo.write().unwrap();
    let e = resolve::record_decision(&g, "a.txt", 0, &Decision::Theirs).unwrap_err();
    assert!(matches!(e, Error::NotFound { .. }), "{e:?}");
    let e = resolve::clear_decision(&g, "a.txt", 0).unwrap_err();
    assert!(matches!(e, Error::NotFound { .. }), "{e:?}");
    drop(g);
    assert_eq!(count(&repo, "hunk_decisions"), 0);
}

#[test]
fn finalise_refuses_a_conflict_that_changed() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let session = open(&repo);
    abort(&repo);
    go(&repo, "feature", false);
    write(tmp.path(), "feature again\n");
    save_as(&repo, "feature edit 2");
    go(&repo, "main", true);
    bring_feature(&repo);
    assert_eq!(resolve::conflict_count(&repo), 1);
    let before = fs::read(tmp.path().join("a.txt")).unwrap();

    let g = repo.write().unwrap();
    let err = resolve::finalise(&g, &session).unwrap_err();
    drop(g);
    assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");
    assert_eq!(fs::read(tmp.path().join("a.txt")).unwrap(), before);
    assert_eq!(resolve::conflict_count(&repo), 1);
}

#[test]
fn decisions_through_separate_short_guards_resolve_the_file() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let mut session = open(&repo);
    {
        let g = repo.write().unwrap();
        resolve::record_decision(&g, "a.txt", 0, &Decision::Theirs).unwrap();
    }
    session.hunks[0].decision = Some(Decision::Theirs);
    {
        let g = repo.write().unwrap();
        resolve::finalise(&g, &session).unwrap();
    }
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "feature\n"
    );
    assert_eq!(count(&repo, "conflict_files"), 0);
    assert_eq!(count(&repo, "hunk_decisions"), 0);
}
