//! `gc --dry-run`: reports what would be collected and removes nothing.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::commands::gc::{self, Collected};
use crate::commands::{save, undo};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotMeta};

fn setup() -> TempDir {
    crate::repo::keyframes_only();
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    tmp
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn save_file(root: &Path, content: &str, msg: &str, amend: bool) {
    fs::write(root.join("f.txt"), content).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    save::run(
        &guard,
        Some(msg),
        save::Options {
            amend,
            ..save::Options::default()
        },
    )
    .unwrap();
}

fn rows(root: &Path, table: &str) -> i64 {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn listing(dir: PathBuf) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn run_gc(root: &Path, dry_run: bool) -> Collected {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    gc::run(
        &guard,
        gc::Options {
            keep_days: 0,
            dry_run,
            ..Default::default()
        },
    )
    .unwrap()
}

/// A repository with every kind of garbage: an expired trash entry, an amended
/// snapshot's old object, a stale cache row and an unreachable chunked object.
fn garbage_repo() -> TempDir {
    let tmp = setup();
    let root = tmp.path();
    let repo = Repo::open_and_migrate(root).unwrap();
    let big = noise(2 * 1024 * 1024, 7);
    let hex = blake3::hash(&big).to_hex().to_string();
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "big",
            entries: vec![TreeEntry::file("big.bin", big)],
            meta: SnapshotMeta::new(),
            timestamp_ms: Some(1),
            author: None,
            renames: &[],
        })
        .unwrap();
    // Make the chunked object unreachable, as an expired undo would.
    repo.conn()
        .execute(
            "DELETE FROM snapshots WHERE hash IN
             (SELECT snapshot_hash FROM file_map WHERE hash = ?1)",
            [&hex],
        )
        .unwrap();
    repo.conn()
        .execute("DELETE FROM file_map WHERE hash = ?", [&hex])
        .unwrap();
    drop(repo);

    save_file(root, "v1", "one", false);
    save_file(root, "v2", "two", false);
    {
        let repo = Repo::open_and_migrate(root).unwrap();
        undo::run(&repo.write().unwrap()).unwrap();
    }
    save_file(root, "v1 amended", "one again", true);
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .execute(
            "INSERT INTO index_cache (path, mtime_ns, size, hash) VALUES ('ghost', 1, 1, 'x')",
            [],
        )
        .unwrap();
    drop(repo);
    tmp
}

#[test]
fn a_dry_run_removes_nothing_and_a_real_run_then_agrees() {
    let tmp = garbage_repo();
    let root = tmp.path();
    let objects = || listing(root.join(".velo/objects"));
    let chunks = || listing(root.join(".velo/chunks"));
    let tables = || {
        [
            "trash",
            "file_map",
            "index_cache",
            "hunk_decisions",
            "trash_tags",
        ]
        .map(|t| rows(root, t))
    };
    let (o, c, t) = (objects(), chunks(), tables());
    assert!(!c.is_empty());

    let dry = run_gc(root, true);
    assert!(dry.dry_run);
    assert!(dry.objects >= 2 && dry.chunks > 0 && dry.stale_cache >= 1);
    assert!(dry.bytes_freed > 1024 * 1024);
    assert_eq!(objects(), o);
    assert_eq!(chunks(), c);
    assert_eq!(tables(), t);

    let real = run_gc(root, false);
    assert!(!real.dry_run);
    assert_eq!(
        Collected {
            dry_run: false,
            ..dry
        },
        real
    );
    assert!(chunks().is_empty());
    assert_ne!(objects(), o);

    // Nothing left: a second dry run is empty.
    assert!(run_gc(root, true).is_empty());
}

#[test]
fn a_dry_run_on_a_clean_repo_is_empty() {
    let tmp = setup();
    save_file(tmp.path(), "v1", "one", false);
    let dry = run_gc(tmp.path(), true);
    assert!(dry.is_empty());
    assert!(dry.dry_run);
}
