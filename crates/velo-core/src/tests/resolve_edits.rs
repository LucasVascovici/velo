//! `resolve --take` never silently replaces edits made by hand.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::resolve::{self, OnEdits, TakeOption};
use crate::commands::{self, merge, save, switch};
use crate::{Error, Repo};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn write(root: &Path, name: &str, body: &str) {
    fs::write(root.join(name), body).unwrap();
}

fn save_as(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

fn go(repo: &Repo, branch: &str, force: bool) {
    let g = repo.write().unwrap();
    switch::run(&g, branch, force).unwrap();
}

/// Two files, `a.txt` and `b.txt`, conflict: main has "main", feature "feature".
fn conflicted(root: &Path, repo: &Repo) {
    write(root, "a.txt", "one\n");
    write(root, "b.txt", "one\n");
    save_as(repo, "base");
    go(repo, "feature", false);
    write(root, "a.txt", "feature\n");
    write(root, "b.txt", "feature\n");
    save_as(repo, "feature edit");
    go(repo, "main", true);
    write(root, "a.txt", "main\n");
    write(root, "b.txt", "main\n");
    save_as(repo, "main edit");
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source: "feature" }).unwrap();
    drop(g);
    assert_eq!(resolve::conflict_count(repo), 2);
}

fn file(repo: &Repo, path: &str) -> resolve::ConflictFile {
    resolve::get_conflict(repo, path).unwrap()
}

#[test]
fn clean_take_reports_dropped_lines() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let g = repo.write().unwrap();
    let t =
        resolve::take_side(&g, &file(&repo, "a.txt"), TakeOption::Ours, OnEdits::Refuse).unwrap();
    assert_eq!(t.dropped_lines, 1);
    assert_eq!(t.replaced_edit_lines, 0);
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "main\n"
    );
}

#[test]
fn an_edited_file_is_refused() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    write(tmp.path(), "a.txt", "main\nmine\n");
    let g = repo.write().unwrap();
    let err = resolve::take_side(
        &g,
        &file(&repo, "a.txt"),
        TakeOption::Theirs,
        OnEdits::Refuse,
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::WouldOverwriteEdits { paths } if paths.len() == 1),
        "{err:?}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "main\nmine\n"
    );
    assert_eq!(resolve::conflict_count(&repo), 2);
}

#[test]
fn replace_overwrites_and_counts_the_lines() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    write(tmp.path(), "a.txt", "main\nmine\nmore\n");
    let g = repo.write().unwrap();
    let t = resolve::take_side(
        &g,
        &file(&repo, "a.txt"),
        TakeOption::Theirs,
        OnEdits::Replace,
    )
    .unwrap();
    assert_eq!(t.replaced_edit_lines, 2);
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "feature\n"
    );
}

#[test]
fn refusal_is_atomic_across_files() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    write(tmp.path(), "b.txt", "main\nmine\n");
    let files = resolve::list_conflicts(&repo).unwrap();
    let g = repo.write().unwrap();
    let err = resolve::take_all(&g, &files, TakeOption::Theirs, OnEdits::Refuse).unwrap_err();
    match err {
        Error::WouldOverwriteEdits { paths } => {
            assert_eq!(paths, vec![std::path::PathBuf::from("b.txt")]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "main\n"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "main\nmine\n"
    );
    assert_eq!(resolve::conflict_count(&repo), 2);
}

#[test]
fn a_working_file_equal_to_theirs_is_not_an_edit() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    write(tmp.path(), "a.txt", "feature\n");
    assert_eq!(resolve::edits(&repo, &file(&repo, "a.txt")).unwrap(), None);
    let g = repo.write().unwrap();
    let t =
        resolve::take_side(&g, &file(&repo, "a.txt"), TakeOption::Ours, OnEdits::Refuse).unwrap();
    assert_eq!(t.replaced_edit_lines, 0);
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "main\n"
    );
}
