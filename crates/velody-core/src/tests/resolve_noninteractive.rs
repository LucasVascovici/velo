//! Non-interactive resolution: `--take both`, `mark` and `merge_source`.

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

/// `a.txt` and `b.txt` conflict: main has "main", feature "feature".
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
fn take_both_matches_the_tui_decision() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    assert_eq!(
        TakeOption::Both.decision(),
        velody_merge::Decision::BothOursFirst
    );
    assert_eq!(TakeOption::Both.as_str(), "both");

    let g = repo.write().unwrap();
    let t =
        resolve::take_side(&g, &file(&repo, "a.txt"), TakeOption::Both, OnEdits::Refuse).unwrap();
    assert_eq!(t.dropped_lines, 0);

    let mut s = resolve::open_session(&repo, file(&repo, "b.txt")).unwrap();
    for h in &mut s.hunks {
        h.decision = Some(velody_merge::Decision::BothOursFirst);
        resolve::record_decision(&g, "b.txt", h.id, &velody_merge::Decision::BothOursFirst)
            .unwrap();
    }
    resolve::finalise(&g, &s).unwrap();

    let a = fs::read_to_string(tmp.path().join("a.txt")).unwrap();
    let b = fs::read_to_string(tmp.path().join("b.txt")).unwrap();
    assert_eq!(a, "main\nfeature\n");
    assert_eq!(a, b);
}

#[test]
fn mark_refuses_untouched_missing_and_marked_files() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let g = repo.write().unwrap();
    let f = file(&repo, "a.txt");

    let err = resolve::mark(&g, &f).unwrap_err();
    assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");

    write(tmp.path(), "a.txt", "feature\n");
    let err = resolve::mark(&g, &f).unwrap_err();
    assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");

    write(tmp.path(), "a.txt", "x\n<<<<<<< ours\nmain\n");
    let err = resolve::mark(&g, &f).unwrap_err();
    assert!(err.to_string().contains("conflict markers"), "{err}");

    fs::remove_file(tmp.path().join("a.txt")).unwrap();
    let err = resolve::mark(&g, &f).unwrap_err();
    assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");
    drop(g);
    assert_eq!(resolve::conflict_count(&repo), 2);
}

#[test]
fn mark_accepts_a_hand_merge_without_writing() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    write(tmp.path(), "a.txt", "main and feature\nextra\n");
    let g = repo.write().unwrap();
    let m = resolve::mark(&g, &file(&repo, "a.txt")).unwrap();
    drop(g);
    assert_eq!(m.path, "a.txt");
    assert_eq!(m.lines, 2);
    assert!(m.differs_from_ours > 0 && m.differs_from_theirs > 0);
    assert_eq!(resolve::conflict_count(&repo), 1);
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "main and feature\nextra\n"
    );
    assert!(tmp.path().join(".velo/MERGE_HEAD").exists());
}

#[test]
fn merge_source_names_the_branch_only_mid_merge() {
    let (tmp, repo) = setup();
    assert_eq!(resolve::merge_source(&repo), None);
    conflicted(tmp.path(), &repo);
    assert_eq!(resolve::merge_source(&repo), Some("feature".to_string()));
}
