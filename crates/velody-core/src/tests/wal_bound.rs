//! The WAL of a long-lived handle stays bounded (23.5).

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::{self, fsck};
use crate::db::{WAL_SIZE_LIMIT_BYTES, WAL_TRUNCATE_BYTES};
use crate::tree::{Change, SaveChanges, TreeEntry};
use crate::{BranchName, Repo, SnapshotId};

/// What one save may add on top of the truncation threshold: its own frames.
const SLACK: u64 = 1 << 20;

fn setup() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let path = tmp.path().to_path_buf();
    (tmp, path)
}

fn wal_len(root: &Path) -> u64 {
    std::fs::metadata(root.join(".velo/velo.db-wal"))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Incompressible bytes, so every save writes real pages.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

fn save_one(repo: &Repo, parent: Option<&SnapshotId>, i: u64) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    let g = repo.write().unwrap();
    let mut spec = SaveChanges::new(&branch, parent, "m");
    spec.timestamp_ms = Some(1_000 + i as i64);
    spec.changes = vec![Change::Put(TreeEntry::file(
        format!("f{}.bin", i % 7),
        noise(i, 48 * 1024),
    ))];
    g.save_changes(spec).unwrap().snapshot
}

fn snapshots(root: &Path) -> i64 {
    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    conn.query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn the_pragma_is_set_on_every_connection() {
    let (_t, root) = setup();
    let repo = Repo::open(&root).unwrap();
    let limit: i64 = repo
        .conn()
        .query_row("PRAGMA journal_size_limit", [], |r| r.get(0))
        .unwrap();
    assert_eq!(limit, WAL_SIZE_LIMIT_BYTES);
}

#[test]
fn three_hundred_saves_keep_the_wal_bounded_and_the_repo_sound() {
    let (_t, root) = setup();
    let mut ids = Vec::new();
    {
        let repo = Repo::open(&root).unwrap();
        let mut parent: Option<SnapshotId> = None;
        for i in 0..300 {
            let id = save_one(&repo, parent.as_ref(), i);
            let wal = wal_len(&root);
            assert!(
                wal <= WAL_TRUNCATE_BYTES + SLACK,
                "save {i}: wal is {wal} bytes"
            );
            ids.push(id.clone());
            parent = Some(id);
        }
    }
    assert_eq!(snapshots(&root), 300);
    let repo = Repo::open(&root).unwrap();
    assert!(fsck::check(&repo).unwrap().is_healthy());
    for id in &ids {
        let n: i64 = repo
            .conn()
            .query_row(
                "SELECT count(*) FROM snapshots WHERE hash = ?",
                [id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}

#[test]
fn a_reader_holding_a_transaction_never_makes_a_save_fail() {
    let (_t, root) = setup();
    let repo = Repo::open(&root).unwrap();
    let first = save_one(&repo, None, 0);
    let reader = crate::db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let seen: i64 = reader
        .query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
        .unwrap();
    let mut parent = first;
    for i in 1..120 {
        parent = save_one(&repo, Some(&parent), i);
    }
    let still: i64 = reader
        .query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(seen, still);
    reader.execute_batch("COMMIT").unwrap();
    // Once the reader lets go, the next save bounds the log again.
    save_one(&repo, Some(&parent), 999);
    assert!(wal_len(&root) <= WAL_TRUNCATE_BYTES + SLACK);
    assert_eq!(snapshots(&root), 121);
}

#[test]
fn a_working_tree_save_bounds_the_wal() {
    let (_t, root) = setup();
    let repo = Repo::open(&root).unwrap();
    for i in 0..120u64 {
        std::fs::write(root.join(format!("w{}.bin", i % 5)), noise(i, 48 * 1024)).unwrap();
        let g = repo.write().unwrap();
        commands::save::run(&g, Some("m"), commands::save::Options::default()).unwrap();
        drop(g);
        let wal = wal_len(&root);
        assert!(wal <= WAL_TRUNCATE_BYTES + SLACK, "save {i}: wal {wal}");
    }
    assert_eq!(snapshots(&root), 120);
}
