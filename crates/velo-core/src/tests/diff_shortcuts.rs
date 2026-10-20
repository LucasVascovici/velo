//! `diff::between` skips hash-equal paths without reading the object store.

use std::fs;
use tempfile::TempDir;

use crate::commands::{self, diff, save};
use crate::{Repo, SnapshotId};

fn setup() -> (TempDir, Repo) {
    crate::repo::keyframes_only();
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn save_as(repo: &Repo, msg: &str) -> SnapshotId {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default()).unwrap();
    drop(g);
    parent(repo)
}

fn parent(repo: &Repo) -> SnapshotId {
    let p = fs::read_to_string(repo.root().join(".velo/PARENT")).unwrap();
    SnapshotId::from_stored(p.trim())
}

fn file_hash(repo: &Repo, snap: &SnapshotId, path: &str) -> String {
    repo.conn()
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
            [snap.as_str(), path],
            |r| r.get(0),
        )
        .unwrap()
}

fn two_snapshots() -> (TempDir, Repo, SnapshotId, SnapshotId) {
    let (tmp, repo) = setup();
    fs::write(tmp.path().join("same.txt"), "same\n").unwrap();
    fs::write(tmp.path().join("chg.txt"), "one\n").unwrap();
    let a = save_as(&repo, "a");
    fs::write(tmp.path().join("chg.txt"), "two\n").unwrap();
    let b = save_as(&repo, "b");
    (tmp, repo, a, b)
}

fn paths(d: &diff::Diff) -> Vec<String> {
    d.files.iter().map(|f| f.path.clone()).collect()
}

#[test]
fn snapshot_range_does_not_read_unchanged_objects() {
    let (_tmp, repo, a, b) = two_snapshots();
    let before = diff::between(&repo, &a, Some(&b), &[]).unwrap();
    let h = file_hash(&repo, &a, "same.txt");
    repo.objects().remove(&h).unwrap();
    let after = diff::between(&repo, &a, Some(&b), &[]).unwrap();
    assert_eq!(paths(&after), vec!["chg.txt".to_string()]);
    assert_eq!(paths(&before), paths(&after));
}

#[test]
fn worktree_against_parent_does_not_read_clean_objects() {
    let (tmp, repo, _a, b) = two_snapshots();
    fs::write(tmp.path().join("chg.txt"), "three\n").unwrap();
    let h = file_hash(&repo, &b, "same.txt");
    repo.objects().remove(&h).unwrap();
    let d = diff::between(&repo, &b, None, &[]).unwrap();
    assert_eq!(paths(&d), vec!["chg.txt".to_string()]);
}

#[test]
fn worktree_against_older_snapshot_still_reports_differences() {
    let (tmp, repo, a, _b) = two_snapshots();
    fs::write(tmp.path().join("new.txt"), "n\n").unwrap();
    let d = diff::between(&repo, &a, None, &[]).unwrap();
    assert_eq!(
        paths(&d),
        vec!["chg.txt".to_string(), "new.txt".to_string()]
    );
}
