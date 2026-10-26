//! `commands::restore::materialize`: a snapshot's tree written into any
//! directory, with no working tree and no `.velo/PARENT` involved.

use std::path::Path;
use tempfile::TempDir;

use crate::commands::restore::{self, MaterializeOptions};
use crate::progress::Cancel;
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Error, Repo, Scope, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn save(repo: &Repo, entries: Vec<TreeEntry>) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "m",
            entries,
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap()
}

fn sample(repo: &Repo) -> SnapshotId {
    save(
        repo,
        vec![
            TreeEntry::file("a.txt", b"alpha\n".to_vec()),
            TreeEntry::file("src/b.rs", b"fn b() {}\n".to_vec()),
            TreeEntry::file("src/deep/c.rs", b"fn c() {}\n".to_vec()),
            TreeEntry::file("logs/x.log", b"log\n".to_vec()),
        ],
    )
}

fn count(repo: &Repo, table: &str) -> i64 {
    repo.conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// Inject a `file_map` row the public API would refuse.
fn inject(repo: &Repo, snapshot: &SnapshotId, path: &str) {
    let guard = repo.write().unwrap();
    let (hash, mode): (String, i64) = guard
        .conn()
        .query_row(
            "SELECT hash, mode FROM file_map WHERE snapshot_hash = ? LIMIT 1",
            [snapshot.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    guard
        .conn()
        .execute(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
            rusqlite::params![snapshot.as_str(), path, hash, mode],
        )
        .unwrap();
}

fn mat(repo: &Repo, snap: &SnapshotId, dest: &Path) -> crate::Result<restore::Materialized> {
    restore::materialize(repo, snap, dest, MaterializeOptions::new())
}

#[test]
fn materializes_every_file_into_an_empty_directory() {
    let (_t, repo) = setup();
    let snap = sample(&repo);
    let out = TempDir::new().unwrap();
    let dest = out.path().join("not/yet/there");
    let done = mat(&repo, &snap, &dest).unwrap();
    assert_eq!(done.files, 4);
    assert_eq!(done.snapshot, snap);
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(
        std::fs::read(dest.join("src/deep/c.rs")).unwrap(),
        b"fn c() {}\n"
    );
    assert_eq!(std::fs::read(dest.join("logs/x.log")).unwrap(), b"log\n");
}

#[cfg(unix)]
#[test]
fn keeps_the_exec_bit_and_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, repo) = setup();
    let snap = save(
        &repo,
        vec![
            TreeEntry::executable("run.sh", b"#!/bin/sh\n".to_vec()),
            TreeEntry::file("plain", b"p".to_vec()),
            TreeEntry::symlink("link", "plain"),
        ],
    );
    let out = TempDir::new().unwrap();
    mat(&repo, &snap, out.path()).unwrap();
    let mode = std::fs::metadata(out.path().join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
    let plain = std::fs::metadata(out.path().join("plain"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(plain & 0o111, 0);
    assert_eq!(
        std::fs::read_link(out.path().join("link")).unwrap(),
        Path::new("plain")
    );
}

#[test]
fn a_scope_limits_what_is_written() {
    let (_t, repo) = setup();
    let snap = sample(&repo);

    let only = Scope::new().only("src/**").unwrap();
    let out = TempDir::new().unwrap();
    let done = restore::materialize(
        &repo,
        &snap,
        out.path(),
        MaterializeOptions::new().scope(&only),
    )
    .unwrap();
    assert_eq!(done.files, 2);
    assert!(out.path().join("src/b.rs").exists());
    assert!(out.path().join("src/deep/c.rs").exists());
    assert!(!out.path().join("a.txt").exists());
    assert!(!out.path().join("logs").exists());

    let ignore = Scope::new()
        .ignore("logs/")
        .unwrap()
        .ignore("*.rs")
        .unwrap();
    let out = TempDir::new().unwrap();
    let done = restore::materialize(
        &repo,
        &snap,
        out.path(),
        MaterializeOptions::new().scope(&ignore),
    )
    .unwrap();
    assert_eq!(done.files, 1);
    assert!(out.path().join("a.txt").exists());
    assert!(!out.path().join("src").exists());
    assert!(!out.path().join("logs").exists());
}

#[test]
fn a_hostile_tree_writes_nothing() {
    let (_t, repo) = setup();
    let snap = sample(&repo);
    inject(&repo, &snap, "../escape");
    let outer = TempDir::new().unwrap();
    let dest = outer.path().join("dest");
    let err = mat(&repo, &snap, &dest).unwrap_err();
    assert!(matches!(err, Error::UnsafePath { .. }), "{err:?}");
    assert!(!dest.exists(), "nothing may be created before validation");
    assert!(!outer.path().join("escape").exists());

    // A hostile row outside the scope still refuses: the tree is what is checked.
    let only = Scope::new().only("a.txt").unwrap();
    let err = restore::materialize(&repo, &snap, &dest, MaterializeOptions::new().scope(&only))
        .unwrap_err();
    assert!(matches!(err, Error::UnsafePath { .. }), "{err:?}");
    assert!(!dest.exists());
}

#[test]
fn the_working_tree_and_the_velody_directory_are_refused() {
    let (t, repo) = setup();
    let snap = sample(&repo);
    for dest in [
        t.path().to_path_buf(),
        t.path().join("sub").join(".."),
        t.path().join(".velo"),
        t.path().join(".velo/objects/new"),
    ] {
        let err = mat(&repo, &snap, &dest).unwrap_err();
        assert!(
            matches!(err, Error::InvalidInput { .. }),
            "{dest:?}: {err:?}"
        );
    }
    assert!(!t.path().join("a.txt").exists());
    // A directory inside the working tree is fine.
    mat(&repo, &snap, &t.path().join("sub")).unwrap();
    assert!(t.path().join("sub/a.txt").exists());
}

#[test]
fn leaves_the_repository_state_alone() {
    let (t, repo) = setup();
    let snap = sample(&repo);
    let parent_before = std::fs::read(t.path().join(".velo/PARENT")).ok();
    let head_before = std::fs::read(t.path().join(".velo/HEAD")).ok();
    let cache_before = count(&repo, "index_cache");
    let out = TempDir::new().unwrap();
    mat(&repo, &snap, out.path()).unwrap();
    assert_eq!(count(&repo, "index_cache"), cache_before);
    assert_eq!(
        std::fs::read(t.path().join(".velo/PARENT")).ok(),
        parent_before
    );
    assert_eq!(std::fs::read(t.path().join(".velo/HEAD")).ok(), head_before);
}

#[test]
fn overwrites_tree_paths_and_keeps_other_files() {
    let (_t, repo) = setup();
    let snap = sample(&repo);
    let out = TempDir::new().unwrap();
    std::fs::write(out.path().join("a.txt"), b"old").unwrap();
    std::fs::write(out.path().join("keep.txt"), b"mine").unwrap();
    mat(&repo, &snap, out.path()).unwrap();
    assert_eq!(std::fs::read(out.path().join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(std::fs::read(out.path().join("keep.txt")).unwrap(), b"mine");
}

#[test]
fn works_on_a_repository_with_no_working_tree() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::create_file(&tmp.path().join("r.velo")).unwrap();
    let snap = sample(&repo);
    // Even the directory holding the database file is a valid destination.
    let done = mat(&repo, &snap, tmp.path()).unwrap();
    assert_eq!(done.files, 4);
    assert_eq!(
        std::fs::read(tmp.path().join("src/b.rs")).unwrap(),
        b"fn b() {}\n"
    );
}

#[test]
fn unknown_and_cancelled_are_typed() {
    let (_t, repo) = setup();
    let snap = sample(&repo);
    let out = TempDir::new().unwrap();
    let missing = SnapshotId::from_stored("0123456789abcdef");
    let err = mat(&repo, &missing, out.path()).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "{err:?}");

    let cancel = Cancel::new();
    cancel.cancel();
    let err = restore::materialize(
        &repo,
        &snap,
        out.path(),
        MaterializeOptions::new().cancel(&cancel),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
}
