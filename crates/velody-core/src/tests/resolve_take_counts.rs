//! `resolve::take_side` reports conflicts and kept clean lines; `mark_with`
//! counts ignore carriage returns.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::resolve::{self, OnCleanChanges, OnEdits, TakeOption};
use crate::commands::{self, merge, save, switch};
use crate::Repo;

fn write(root: &Path, body: &str) {
    fs::write(root.join("r.txt"), body).unwrap();
}

fn save_as(repo: &Repo, msg: &str) {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
}

/// Conflicted merge of `main` (ours) with `other` (theirs) over `base`.
fn setup(base: &str, ours: &str, theirs: &str) -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let root = tmp.path();
    write(root, base);
    save_as(&repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "other", false).unwrap();
    }
    write(root, theirs);
    save_as(&repo, "other edit");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "main", true).unwrap();
    }
    write(root, ours);
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

#[test]
fn take_ours_leaves_ours_and_keeps_nothing() {
    let ours = "a\nOURS\nc\n";
    let (tmp, repo) = setup("a\nb\nc\n", ours, "a\nTHEIRS\nc\n");
    let g = repo.write().unwrap();
    let t = resolve::take_side(&g, &file(&repo), TakeOption::Ours, OnEdits::Refuse).unwrap();
    drop(g);
    assert_eq!(t.conflicts, 1);
    assert_eq!(t.kept_clean_lines, 0);
    assert_eq!(t.dropped_lines, 1);
    assert_eq!(fs::read_to_string(tmp.path().join("r.txt")).unwrap(), ours);
}

#[test]
fn take_ours_counts_the_clean_theirs_lines_it_keeps() {
    let (tmp, repo) = setup(
        "H\nstock\nbulk\nwh\n",
        "H\nstock\ncancel\nbulk\nwh\n",
        "H\ncarol\nstock\nheidi\nbulk\nwh\n",
    );
    let g = repo.write().unwrap();
    let t = resolve::take_side(&g, &file(&repo), TakeOption::Ours, OnEdits::Refuse).unwrap();
    drop(g);
    assert_eq!(t.conflicts, 1);
    assert_eq!(t.kept_clean_lines, 1, "{t:?}");
    let body = fs::read_to_string(tmp.path().join("r.txt")).unwrap();
    assert!(body.contains("carol"), "{body}");
}

#[test]
fn mark_counts_ignore_carriage_returns() {
    let edit = "a\nMINE\nc\n";
    let (tmp, repo) = setup("a\nb\nc\n", "a\nOURS\nc\n", "a\nTHEIRS\nc\n");
    write(tmp.path(), edit);
    let g = repo.write().unwrap();
    let lf = resolve::mark_with(&g, &file(&repo), OnCleanChanges::Refuse).unwrap();
    drop(g);

    let (tmp, repo) = setup("a\nb\nc\n", "a\nOURS\nc\n", "a\nTHEIRS\nc\n");
    write(tmp.path(), &edit.replace('\n', "\r\n"));
    let g = repo.write().unwrap();
    let crlf = resolve::mark_with(&g, &file(&repo), OnCleanChanges::Refuse).unwrap();
    drop(g);
    assert_eq!(lf.differs_from_ours, 2);
    assert_eq!(crlf.differs_from_ours, lf.differs_from_ours);
    assert_eq!(crlf.differs_from_theirs, lf.differs_from_theirs);
}
