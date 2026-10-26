//! `merge --abort` gives back the exact pre-merge bytes of CR-ending files.

use std::fs;
use tempfile::TempDir;

use crate::commands::resolve::{self, OnEdits, TakeOption};
use crate::commands::{self, diff, merge, save, status, switch};
use crate::Repo;

fn save_as(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

fn merge_in(repo: &Repo) {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source: "feat" }).unwrap();
}

fn abort(repo: &Repo) {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Abort).unwrap();
}

/// `main` and `feat` edit line 2 of `F.md`; `main` is checked out and saved.
fn setup(base: &[u8], ours: &[u8], theirs: &[u8]) -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let f = tmp.path().join("F.md");
    fs::write(&f, base).unwrap();
    save_as(&repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "feat", false).unwrap();
    }
    fs::write(&f, theirs).unwrap();
    save_as(&repo, "feat edit");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "main", true).unwrap();
    }
    fs::write(&f, ours).unwrap();
    save_as(&repo, "main edit");
    (tmp, repo)
}

fn take_both(repo: &Repo) {
    let row = resolve::get_conflict(repo, "F.md").unwrap();
    let g = repo.write().unwrap();
    resolve::take_side(&g, &row, TakeOption::Both, OnEdits::Refuse).unwrap();
}

fn conflict_round_trip(base: &[u8], ours: &[u8], theirs: &[u8], with_resolve: bool) {
    let (tmp, repo) = setup(base, ours, theirs);
    let f = tmp.path().join("F.md");
    merge_in(&repo);
    assert_eq!(
        tmp.path().join(".velo/MERGE_ORIG").exists(),
        ours.contains(&b"\r".as_slice()[0])
    );
    if with_resolve {
        take_both(&repo);
    }
    abort(&repo);
    assert_eq!(fs::read(&f).unwrap(), ours);
    assert!(!tmp.path().join(".velo/MERGE_ORIG").exists());
    assert!(get_status_clean(&repo));
    assert!(diff_empty(&repo));
}

fn get_status_clean(repo: &Repo) -> bool {
    status::run(repo, &[])
        .map(|s| s.is_clean())
        .unwrap_or(false)
}

fn diff_empty(repo: &Repo) -> bool {
    diff::run(repo, &None)
        .map(|d| d.is_empty())
        .unwrap_or(false)
}

#[test]
fn crlf_file_survives_resolve_and_abort() {
    conflict_round_trip(
        b"a\r\nb\r\nc\r\n",
        b"a\r\nb-main\r\nc\r\n",
        b"a\r\nb-feat\r\nc\r\n",
        true,
    );
}

#[test]
fn crlf_file_survives_abort_without_resolve() {
    conflict_round_trip(
        b"a\r\nb\r\nc\r\n",
        b"a\r\nb-main\r\nc\r\n",
        b"a\r\nb-feat\r\nc\r\n",
        false,
    );
}

#[test]
fn lf_file_survives_resolve_and_abort() {
    conflict_round_trip(b"a\nb\nc\n", b"a\nb-main\nc\n", b"a\nb-feat\nc\n", true);
}

#[test]
fn mixed_endings_survive_resolve_and_abort() {
    conflict_round_trip(
        b"a\r\nb\nc\r\n",
        b"a\r\nb-main\nc\r\n",
        b"a\r\nb-feat\nc\r\n",
        true,
    );
}

#[test]
fn cleanly_merged_crlf_file_is_restored() {
    // Only theirs changes the file, so the merge rewrites it without conflict.
    let (tmp, repo) = setup(b"a\r\nb\r\nc\r\n", b"a\r\nb\r\nc\r\n", b"a\r\nB\r\nc\r\n");
    // `main edit` saved nothing new; make main diverge on another file.
    fs::write(tmp.path().join("other.txt"), "x\n").unwrap();
    save_as(&repo, "main other");
    merge_in(&repo);
    let f = tmp.path().join("F.md");
    abort(&repo);
    assert_eq!(fs::read(&f).unwrap(), b"a\r\nb\r\nc\r\n");
    assert!(get_status_clean(&repo));
    assert!(!tmp.path().join(".velo/MERGE_ORIG").exists());
}

#[test]
fn save_finalising_a_merge_removes_merge_orig() {
    let (tmp, repo) = setup(
        b"a\r\nb\r\nc\r\n",
        b"a\r\nb-main\r\nc\r\n",
        b"a\r\nb-feat\r\nc\r\n",
    );
    merge_in(&repo);
    take_both(&repo);
    assert!(tmp.path().join(".velo/MERGE_ORIG").exists());
    save_as(&repo, "merge");
    assert!(!tmp.path().join(".velo/MERGE_ORIG").exists());
}
