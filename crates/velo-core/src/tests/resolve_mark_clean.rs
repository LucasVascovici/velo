//! `resolve::mark` refuses a hand edit that drops the other side's clean changes.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::resolve::{self, OnCleanChanges, TakeOption};
use crate::commands::{self, merge, save, switch};
use crate::{Error, Repo};

fn write(root: &Path, body: &str) {
    fs::write(root.join("r.txt"), body).unwrap();
}

fn save_as(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

/// One conflict hunk after `stock`, plus a clean `carol` from `other`.
fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let root = tmp.path();
    write(root, "H\nstock\nbulk\nwh\n");
    save_as(&repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "other", false).unwrap();
    }
    write(root, "H\ncarol\nstock\nheidi\ncat\nbulk\nwh\n");
    save_as(&repo, "other edit");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "main", true).unwrap();
    }
    write(root, "H\nstock\ncancel\nbulk\nwh\n");
    save_as(&repo, "main edit");
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source: "other" }).unwrap();
    drop(g);
    assert_eq!(resolve::conflict_count(&repo), 1);
    (tmp, repo)
}

fn file(repo: &Repo) -> resolve::ConflictFile {
    resolve::get_conflict(repo, "r.txt").unwrap()
}

const WITHOUT_CAROL: &str = "H\nstock\ncancel\nheidi\ncat\nbulk\nwh\n";
const WITH_CAROL: &str = "H\ncarol\nstock\ncancel\nheidi\ncat\nbulk\nwh\n";

#[test]
fn refuses_an_edit_without_carol() {
    let (tmp, repo) = setup();
    write(tmp.path(), WITHOUT_CAROL);
    let g = repo.write().unwrap();
    let err = resolve::mark(&g, &file(&repo)).unwrap_err();
    drop(g);
    match err {
        Error::MissingCleanChanges {
            missing, sample, ..
        } => {
            assert!(missing >= 1);
            assert!(sample.iter().any(|l| l == "carol"));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(resolve::conflict_count(&repo), 1);
    assert_eq!(
        fs::read_to_string(tmp.path().join("r.txt")).unwrap(),
        WITHOUT_CAROL
    );
}

#[test]
fn accepts_once_carol_is_back() {
    let (tmp, repo) = setup();
    write(tmp.path(), WITH_CAROL);
    let g = repo.write().unwrap();
    let m = resolve::mark(&g, &file(&repo)).unwrap();
    assert!(m.dropped_clean_changes.is_empty());
    drop(g);
    assert_eq!(resolve::conflict_count(&repo), 0);
}

#[test]
fn drop_override_reports_the_lines() {
    let (tmp, repo) = setup();
    write(tmp.path(), WITHOUT_CAROL);
    let g = repo.write().unwrap();
    let m = resolve::mark_with(&g, &file(&repo), OnCleanChanges::Drop).unwrap();
    drop(g);
    assert_eq!(m.dropped_clean_changes, vec!["carol".to_string()]);
    assert_eq!(resolve::conflict_count(&repo), 0);
}

#[test]
fn a_moved_line_is_accepted() {
    let (tmp, repo) = setup();
    write(
        tmp.path(),
        "H\nstock\ncancel\nheidi\ncat\nbulk\nwh\ncarol\n",
    );
    let g = repo.write().unwrap();
    resolve::mark(&g, &file(&repo)).unwrap();
}

#[test]
fn untouched_file_lists_the_clean_lines() {
    let (_tmp, repo) = setup();
    let missing = resolve::missing_clean_changes(&repo, &file(&repo)).unwrap();
    assert!(missing.iter().any(|l| l == "carol"));
}

#[test]
fn take_paths_keep_carol() {
    for side in [TakeOption::Ours, TakeOption::Theirs, TakeOption::Both] {
        let (tmp, repo) = setup();
        let g = repo.write().unwrap();
        resolve::take_side(&g, &file(&repo), side, resolve::OnEdits::Refuse).unwrap();
        drop(g);
        let text = fs::read_to_string(tmp.path().join("r.txt")).unwrap();
        assert!(text.contains("carol"), "{side:?}");
    }
}
