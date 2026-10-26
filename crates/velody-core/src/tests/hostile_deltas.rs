//! Hostile `tree_deltas` and `tree_delta_rows` tables are typed errors: a
//! cycle, a missing base, a chain of a million links, a row whose hash and mode
//! disagree, a path past the size ceiling, a path that escapes the root. None
//! of them hangs, panics or allocates without bound.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use crate::commands::restore;
use crate::limits::Limit;
use crate::tree::{SaveTree, TreeEntry};
use crate::tree_store::{self, PathVersions};
use crate::{BranchName, Error, Limits, Repo, SnapshotId, SnapshotMeta};

fn nested() -> (TempDir, PathBuf, Repo) {
    crate::repo::keyframes_only();
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    crate::commands::init::run(&root).unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    (outer, root, repo)
}

fn save(repo: &Repo, parent: Option<&SnapshotId>, file: &str, body: &str) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: "m",
            entries: vec![TreeEntry::file(file, body.as_bytes().to_vec())],
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap()
}

/// Two keyframes `a` and `b`; `b` then becomes a delta on `a` with no rows.
fn pair(repo: &Repo) -> (String, String) {
    let a = save(repo, None, "a.txt", "one");
    let b = save(repo, Some(&a), "a.txt", "two");
    let (a, b) = (a.to_string(), b.to_string());
    let c = repo.conn();
    c.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [&b])
        .unwrap();
    c.execute("INSERT INTO tree_deltas VALUES (?, ?)", [&b, &a])
        .unwrap();
    (a, b)
}

fn object_of(repo: &Repo, snapshot: &str) -> String {
    repo.conn()
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? LIMIT 1",
            [snapshot],
            |r| r.get(0),
        )
        .unwrap()
}

fn row(repo: &Repo, snapshot: &str, path: &str, hash: Option<&str>, mode: Option<i64>) {
    repo.conn()
        .execute(
            "INSERT OR REPLACE INTO tree_delta_rows VALUES (?, ?, ?, ?)",
            rusqlite::params![snapshot, path, hash, mode],
        )
        .unwrap();
}

fn assert_corrupt<T: std::fmt::Debug>(result: crate::Result<T>, what: &str) -> String {
    match result {
        Err(Error::Corrupt { detail }) => detail,
        other => panic!("{what}: expected Corrupt, got {other:?}"),
    }
}

/// Every single-snapshot reader refuses `snapshot` as corrupt.
fn all_readers_refuse(repo: &Repo, snapshot: &str) {
    let c = repo.conn();
    assert_corrupt(tree_store::tree_of(c, snapshot), "tree_of");
    assert_corrupt(tree_store::file_at(c, snapshot, "a.txt"), "file_at");
    assert_corrupt(tree_store::files_at(c, snapshot, &["a.txt"]), "files_at");
}

#[test]
fn a_two_node_cycle_is_corrupt() {
    let (_o, _r, repo) = nested();
    let (a, b) = pair(&repo);
    repo.conn()
        .execute("INSERT INTO tree_deltas VALUES (?, ?)", [&a, &b])
        .unwrap();
    all_readers_refuse(&repo, &a);
    all_readers_refuse(&repo, &b);
    let v = PathVersions::load(repo.conn(), "a.txt").unwrap();
    assert_corrupt(v.at(&a), "PathVersions::at");
}

#[test]
fn a_self_cycle_is_corrupt() {
    let (_o, _r, repo) = nested();
    let (a, _b) = pair(&repo);
    repo.conn()
        .execute("INSERT INTO tree_deltas VALUES (?, ?)", [&a, &a])
        .unwrap();
    all_readers_refuse(&repo, &a);
}

#[test]
fn a_base_in_neither_table_is_corrupt() {
    let (_o, _r, repo) = nested();
    let (_a, b) = pair(&repo);
    let ghost = "e".repeat(64);
    repo.conn()
        .execute(
            "UPDATE tree_deltas SET base_hash = ? WHERE snapshot_hash = ?",
            [&ghost, &b],
        )
        .unwrap();
    let detail = assert_corrupt(tree_store::tree_of(repo.conn(), &b), "tree_of");
    assert!(detail.contains("does not exist"), "{detail}");
    all_readers_refuse(&repo, &b);
}

#[test]
fn a_chain_of_a_million_links_errors_within_the_bound() {
    let (_o, _r, repo) = nested();
    let (_a, b) = pair(&repo);
    let c = repo.conn();
    // `b` leads into a synthetic chain of 1,000,000 links, every one of them a
    // known (trashed) snapshot so that only the length bound can stop the walk.
    c.execute("DELETE FROM tree_deltas WHERE snapshot_hash = ?", [&b])
        .unwrap();
    c.execute(
        "INSERT INTO tree_deltas VALUES (?, printf('%064d', 1))",
        [&b],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000000)
         INSERT INTO tree_deltas SELECT printf('%064d', i), printf('%064d', i + 1) FROM n",
        [],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i <= 1000000)
         INSERT INTO trash (hash, message, branch) SELECT printf('%064d', i), 'x', 'main' FROM n",
        [],
    )
    .unwrap();

    let start = Instant::now();
    let detail = assert_corrupt(tree_store::tree_of(c, &b), "tree_of");
    assert!(detail.contains("longer than"), "{detail}");
    all_readers_refuse(&repo, &b);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "walked too far: {:?}",
        start.elapsed()
    );
}

#[test]
fn a_hash_without_a_mode_or_a_mode_without_a_hash_is_corrupt() {
    for (with_hash, mode) in [(true, None), (false, Some(0o644_i64))] {
        let (_o, _r, repo) = nested();
        let (a, b) = pair(&repo);
        let obj = object_of(&repo, &a);
        row(&repo, &b, "a.txt", with_hash.then_some(obj.as_str()), mode);
        let c = repo.conn();
        assert_corrupt(tree_store::tree_of(c, &b), "tree_of");
        assert_corrupt(tree_store::file_at(c, &b, "a.txt"), "file_at");
        assert_corrupt(
            PathVersions::load(c, "a.txt").map(|_| ()),
            "PathVersions::load",
        );
        assert_corrupt(tree_store::changes_of(c, &b), "changes_of");
        assert_corrupt(
            tree_store::changed_under(c, &b, &a, "a.txt"),
            "changed_under",
        );
        let walked = tree_store::for_each_tree_in_order(c, |_, _| Ok(()));
        assert_corrupt(walked, "for_each_tree_in_order");
    }
}

#[test]
fn a_reconstructed_path_over_the_string_ceiling_is_a_limit_error() {
    let (_o, _r, repo) = nested();
    let (a, b) = pair(&repo);
    let obj = object_of(&repo, &a);
    row(&repo, &b, &"x".repeat(65), Some(&obj), Some(0o644));
    let id: SnapshotId = b.parse().unwrap();
    let repo = repo.limited(Limits::default().with_max_string_bytes(64));
    match repo.tree_at(&id) {
        Err(Error::LimitExceeded { limit, max, found }) => {
            assert_eq!(limit, Limit::StringBytes);
            assert_eq!(max, 64);
            assert_eq!(found, Some(65));
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
    // The same row under the default ceiling reads fine.
    let repo = repo.limited(Limits::default());
    assert!(repo.tree_at(&id).is_ok());
}

fn restore_refuses_row(path: &str) {
    let (outer, root, repo) = nested();
    let (a, b) = pair(&repo);
    let obj = object_of(&repo, &a);
    row(&repo, &b, path, Some(&obj), Some(0o644));
    let id: SnapshotId = b.parse().unwrap();
    let guard = repo.write().unwrap();
    let err = restore::run(
        &guard,
        &id,
        restore::Options {
            force: true,
            ..Default::default()
        },
    );
    match err {
        Err(Error::UnsafePath { path: p, .. }) => assert!(p.contains(path), "{p}"),
        other => panic!("expected UnsafePath for {path}, got {other:?}"),
    }
    drop(guard);
    assert!(!outer.path().join("x").exists());
    assert!(!root.join("x").exists());
}

#[test]
fn restore_refuses_a_delta_row_that_escapes_the_root() {
    restore_refuses_row("../x");
}

#[test]
fn restore_refuses_a_delta_row_with_an_absolute_path() {
    restore_refuses_row("/x");
}

#[test]
fn committed_seeds_replay_through_the_fuzz_entry() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/delta_table");
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let data = std::fs::read(entry.unwrap().path()).unwrap();
        crate::fuzzing::delta_table(&data);
        // Every prefix too: a truncated record must not panic either.
        for len in 0..data.len() {
            crate::fuzzing::delta_table(&data[..len]);
        }
        n += 1;
    }
    assert!(n >= 3, "seeds are missing");
}
