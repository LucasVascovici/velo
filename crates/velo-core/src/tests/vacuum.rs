//! `gc::vacuum` and the `auto_vacuum` mode new repositories are created with.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::compact::{self, Range};
use crate::commands::gc::{self, AutoVacuum, Vacuum};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn auto_vacuum_of(repo: &Repo) -> i64 {
    let guard = repo.write().unwrap();
    guard
        .conn()
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap()
}

fn auto_vacuum_at(db: &Path) -> i64 {
    rusqlite::Connection::open(db)
        .unwrap()
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap()
}

/// A chain of `n` snapshots on main, each with a fresh 40 KiB file and 200
/// small ones, so that squashing them frees many database pages.
fn chain(repo: &Repo, n: usize) -> Vec<SnapshotId> {
    let main: BranchName = "main".parse().unwrap();
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..n {
        let body: Vec<u8> = (0..40_000u32)
            .map(|j| ((j.wrapping_mul(2654435761).wrapping_add(i as u32 * 7919)) >> 13) as u8)
            .collect();
        let mut entries = vec![TreeEntry::file(format!("big{i}.bin"), body)];
        for j in 0..200 {
            entries.push(TreeEntry::file(
                format!("dir/file_{j:04}.txt"),
                format!(
                    "snapshot {i} file {j}
"
                )
                .into_bytes(),
            ));
        }
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &main,
                parent: ids.last(),
                message: &format!("step {i}"),
                entries: entries,
                meta: SnapshotMeta::default(),
                author: None,
                merge_parent: None,
                timestamp_ms: Some(1_000 + i as i64),
                renames: &[],
            })
            .unwrap();
        ids.push(id);
    }
    ids
}

fn squash_all(repo: &Repo, ids: &[SnapshotId]) {
    let guard = repo.write().unwrap();
    compact::run(
        &guard,
        &"main".parse().unwrap(),
        &[Range {
            oldest: &ids[0],
            newest: ids.last().unwrap(),
        }],
        Default::default(),
    )
    .unwrap();
    // Squashing only unlinks the old snapshots; gc drops what they alone held.
    gc::run(&guard, gc::Options::default()).unwrap();
}

fn corpus_copy() -> (TempDir, PathBuf) {
    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dest = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dest);
            } else {
                fs::copy(e.path(), &dest).unwrap();
            }
        }
    }
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("repo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/repos/v5.1.2");
    copy_dir(&src, &root);
    (dir, root)
}

#[test]
fn new_repositories_are_created_incremental() {
    let (_t, repo) = setup();
    assert_eq!(auto_vacuum_of(&repo), 2);

    let tmp = TempDir::new().unwrap();
    let file = Repo::create_file(&tmp.path().join("a.velo")).unwrap();
    assert_eq!(auto_vacuum_of(&file), 2);
}

#[test]
fn opening_and_migrating_a_corpus_repository_leaves_auto_vacuum_alone() {
    let (_t, root) = corpus_copy();
    let db = root.join(".velo/velo.db");
    let before = auto_vacuum_at(&db);
    let repo = Repo::open_and_migrate(&root).unwrap();
    drop(repo);
    assert_eq!(auto_vacuum_at(&db), before);
    assert_eq!(before, 0);
}

#[test]
fn incremental_vacuum_shrinks_a_new_repository_after_compaction() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 6);
    squash_all(&repo, &ids);
    let guard = repo.write().unwrap();
    let done = gc::vacuum(&guard, Vacuum::Incremental).unwrap();
    assert_eq!(done.mode, Vacuum::Incremental);
    assert_eq!(done.auto_vacuum, AutoVacuum::Incremental);
    assert!(
        done.bytes_after < done.bytes_before,
        "{} -> {}",
        done.bytes_before,
        done.bytes_after
    );
}

#[test]
fn vacuum_with_no_free_pages_changes_nothing() {
    let (_t, repo) = setup();
    chain(&repo, 2);
    let guard = repo.write().unwrap();
    gc::vacuum(&guard, Vacuum::Incremental).unwrap();
    let done = gc::vacuum(&guard, Vacuum::Incremental).unwrap();
    assert_eq!(done.bytes_before, done.bytes_after);
}

#[test]
fn old_repository_is_a_noop_until_a_full_vacuum_converts_it() {
    let (_t, root) = corpus_copy();
    let repo = Repo::open_and_migrate(&root).unwrap();
    let guard = repo.write().unwrap();
    // Make free pages: write and then remove a large blob table row set.
    guard
        .conn()
        .execute_batch(
            "CREATE TABLE scratch(b BLOB);
             INSERT INTO scratch SELECT zeroblob(4000) FROM (
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 200)
                 SELECT i FROM n);
             DROP TABLE scratch;",
        )
        .unwrap();
    let noop = gc::vacuum(&guard, Vacuum::Incremental).unwrap();
    assert_eq!(noop.auto_vacuum, AutoVacuum::None);
    assert_eq!(noop.bytes_before, noop.bytes_after);

    let full = gc::vacuum(&guard, Vacuum::Full).unwrap();
    assert_eq!(full.mode, Vacuum::Full);
    assert_eq!(full.auto_vacuum, AutoVacuum::Incremental);
    assert!(full.bytes_after < full.bytes_before);
}

#[test]
fn single_file_repository_vacuums_after_compaction() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::create_file(&tmp.path().join("a.velo")).unwrap();
    let ids = chain(&repo, 6);
    squash_all(&repo, &ids);
    let guard = repo.write().unwrap();
    let done = gc::vacuum(&guard, Vacuum::Incremental).unwrap();
    assert_eq!(done.auto_vacuum, AutoVacuum::Incremental);
    assert!(done.bytes_after < done.bytes_before);
}
