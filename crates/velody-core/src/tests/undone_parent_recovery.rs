//! Repositories damaged by the 5.1.0 `undo` bug: a live snapshot whose parent
//! sits in the trash. `fsck` names `redo` as the way back, and the trash row
//! survives `save` and `gc` until then.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::commands::fsck::{self, Problem};
use crate::commands::gc;
use crate::commands::history;
use crate::commands::{redo, save, switch};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn setup() -> TempDir {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    tmp
}

fn put(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    merge_parent: Option<&SnapshotId>,
    body: &str,
    ts: i64,
) -> SnapshotId {
    let branch: BranchName = branch.parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent,
            message: &format!("step {ts}"),
            entries: vec![TreeEntry::file("f.txt", body.as_bytes().to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

/// Shelve `hash` in the trash and drop it from `snapshots`, the way 5.1.0's
/// `undo` did even though another snapshot built on it.
fn damage(repo: &Repo, hash: &SnapshotId, deleted_at_ms: i64) {
    let guard = repo.write().unwrap();
    let conn = guard.conn();
    conn.execute(
        "INSERT INTO trash (hash, message, branch, parent_hash, merge_parent, created_at_ms, deleted_at_ms)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms, ?
         FROM snapshots WHERE hash = ?",
        rusqlite::params![deleted_at_ms, hash.as_str()],
    )
    .unwrap();
    conn.execute("DELETE FROM snapshots WHERE hash = ?", [hash.as_str()])
        .unwrap();
}

fn trash_has(root: &Path, hash: &SnapshotId) -> bool {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row(
            "SELECT COUNT(*) FROM trash WHERE hash = ?",
            [hash.as_str()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        > 0
}

fn run_gc(root: &Path) -> gc::Collected {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    gc::run(
        &guard,
        gc::Options {
            keep_days: 0,
            ..Default::default()
        },
    )
    .unwrap()
}

/// wip holds W0 then A; main holds B built on A. A is then undone the 5.1.0 way.
fn damaged() -> (TempDir, SnapshotId, SnapshotId) {
    let tmp = setup();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let w0 = put(&repo, "wip", None, None, "w0", 1);
    let a = put(&repo, "wip", Some(&w0), None, "a", 2);
    let b = put(&repo, "main", Some(&a), None, "b", 3);
    damage(&repo, &a, 1);
    // The working position sits on a live snapshot, as it would after the undo.
    fs::write(tmp.path().join(".velo/PARENT"), b.as_str()).unwrap();
    (tmp, a, b)
}

#[test]
fn fsck_points_a_trashed_parent_at_redo() {
    let (tmp, a, _b) = damaged();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let report = fsck::check(&repo).unwrap();
    assert!(!report.is_healthy());
    assert!(report.problems.iter().any(|p| matches!(
        p,
        Problem::UndoneParent { parent, branch, .. }
            if parent == a.as_str() && branch == "wip"
    )));
    assert!(!report
        .problems
        .iter()
        .any(|p| matches!(p, Problem::MissingParent { .. })));
    let text = report.problems[0].to_string();
    assert!(text.contains("velody redo"), "{text}");
}

#[test]
fn fsck_points_a_trashed_merge_parent_at_redo() {
    let tmp = setup();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let root = put(&repo, "main", None, None, "r", 1);
    let a = put(&repo, "wip", Some(&root), None, "a", 2);
    let m = put(&repo, "main", Some(&root), Some(&a), "m", 3);
    damage(&repo, &a, 1);
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.iter().any(|p| matches!(
        p,
        Problem::UndoneMergeParent { snapshot, merge_parent, branch }
            if snapshot == m.as_str() && merge_parent == a.as_str() && branch == "wip"
    )));
    assert!(!report
        .problems
        .iter()
        .any(|p| matches!(p, Problem::MissingMergeParent { .. })));
}

#[test]
fn a_parent_in_neither_table_is_still_missing() {
    let (tmp, a, _b) = damaged();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    {
        let guard = repo.write().unwrap();
        guard
            .conn()
            .execute("DELETE FROM trash WHERE hash = ?", [a.as_str()])
            .unwrap();
    }
    let report = fsck::check(&repo).unwrap();
    assert!(report
        .problems
        .iter()
        .any(|p| matches!(p, Problem::MissingParent { parent, .. } if parent == a.as_str())));
    assert!(!report
        .problems
        .iter()
        .any(|p| matches!(p, Problem::UndoneParent { .. })));
}

#[test]
fn redo_restores_a_damaged_repository() {
    let (tmp, a, b) = damaged();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    switch::run(&repo.write().unwrap(), "wip", true).unwrap();
    // `init` leaves a `.veloignore` no snapshot holds, which redo reads as dirty.
    let _ = fs::remove_file(tmp.path().join(".veloignore"));
    let out = redo::run(&repo.write().unwrap()).unwrap();
    assert_eq!(out.snapshot, a.as_str());
    assert!(fsck::check(&repo).unwrap().is_healthy());
    let wip: BranchName = "wip".parse().unwrap();
    let on_wip = history::run(
        &repo,
        history::Options {
            branch: Some(&wip),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(on_wip.entries.iter().any(|e| e.hash == a));
    let from_main = history::run(
        &repo,
        history::Options {
            from: Some(&b),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(from_main.entries.iter().any(|e| e.hash == a));
}

#[test]
fn save_and_gc_keep_the_trash_row_a_snapshot_needs() {
    let (tmp, a, _b) = damaged();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    switch::run(&repo.write().unwrap(), "wip", true).unwrap();
    fs::write(tmp.path().join("g.txt"), "new").unwrap();
    save::run(
        &repo.write().unwrap(),
        Some("again"),
        save::Options::default(),
    )
    .unwrap();
    assert!(trash_has(tmp.path(), &a), "save purged the needed row");
    run_gc(tmp.path());
    assert!(trash_has(tmp.path(), &a), "gc expired the needed row");

    // gc also deletes object files; redo must still find everything A needs.
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let out = redo::run(&repo.write().unwrap()).unwrap();
    assert_eq!(out.snapshot, a.as_str());
    let in_snapshots: i64 = repo
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM snapshots WHERE hash = ?",
            [a.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_snapshots, 1, "redo did not bring A back");
    assert_eq!(fs::read_to_string(tmp.path().join("f.txt")).unwrap(), "a");
}

#[test]
fn unrelated_trash_rows_still_expire() {
    let tmp = setup();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let a = put(&repo, "wip", None, None, "a", 1);
    put(&repo, "main", Some(&a), None, "b", 2);
    // Leaves nothing builds on, shelved long ago.
    let leaf = put(&repo, "wip", Some(&a), None, "leaf", 5);
    let other = put(&repo, "main", None, None, "other", 6);
    damage(&repo, &a, 1);
    damage(&repo, &leaf, 2);
    damage(&repo, &other, 2);
    let collected = run_gc(tmp.path());
    assert_eq!(collected.expired_trash, 2);
    assert!(!trash_has(tmp.path(), &leaf));
    assert!(!trash_has(tmp.path(), &other));
    assert!(trash_has(tmp.path(), &a));
}
