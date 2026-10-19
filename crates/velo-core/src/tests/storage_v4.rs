//! Storage format v4: the O(1) migration and the delta-aware single-snapshot
//! readers of `tree_store`. Nothing writes deltas yet, so the deltas here are
//! built by hand with SQL, which is exactly what a hand-damaged or
//! spec-written repository looks like to a reader.

use rusqlite::Connection;
use tempfile::TempDir;

use crate::tree::{SaveTree, TreeEntry};
use crate::tree_store;
use crate::{BranchName, Error, Repo, SnapshotId, SnapshotMeta, FORMAT_VERSION};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn save(repo: &Repo, parent: Option<&SnapshotId>, files: &[(&str, &str)], ts: i64) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: "step",
            entries: files
                .iter()
                .map(|(p, b)| TreeEntry::file(*p, b.as_bytes().to_vec()))
                .collect(),
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

/// Turn `snap` into a delta against `base`: its `file_map` rows go, and the
/// given rows (`None` hash: removed) take their place.
fn make_delta(conn: &Connection, snap: &str, base: &str, rows: &[(&str, Option<&str>)]) {
    conn.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [snap])
        .unwrap();
    conn.execute("INSERT INTO tree_deltas VALUES (?, ?)", [snap, base])
        .unwrap();
    for (path, hash) in rows {
        let mode = hash.map(|_| 0i64);
        conn.execute(
            "INSERT INTO tree_delta_rows VALUES (?, ?, ?, ?)",
            rusqlite::params![snap, path, hash, mode],
        )
        .unwrap();
    }
}

/// A keyframe holding `a`, `dir/b`, `dir/c`, and a delta on it that changes
/// `a`, removes `dir/b`, adds `new/d`. Returns (keyframe, delta).
fn keyframe_and_delta(repo: &Repo) -> (String, String) {
    let k = save(repo, None, &[("a", "1"), ("dir/b", "2"), ("dir/c", "3")], 1);
    let d = save(repo, Some(&k), &[("zz", "x")], 2);
    let (k, d) = (k.to_string(), d.to_string());
    make_delta(
        repo.conn(),
        &d,
        &k,
        &[("a", Some("h-a2")), ("dir/b", None), ("new/d", Some("h-d"))],
    );
    (k, d)
}

#[test]
fn a_fresh_repository_is_v4_with_both_tables_and_indexes() {
    let (_t, repo) = setup();
    assert_eq!(FORMAT_VERSION, 4);
    assert_eq!(repo.format_version().unwrap(), 4);
    for (kind, name) in [
        ("table", "tree_deltas"),
        ("table", "tree_delta_rows"),
        ("index", "idx_tree_deltas_base"),
        ("index", "idx_delta_rows_path"),
    ] {
        let n: i64 = repo
            .conn()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                [kind, name],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{name}");
    }
}

#[test]
fn migrating_a_v3_repository_writes_no_tree_row() {
    let (tmp, repo) = setup();
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..60i64 {
        let body = format!("body {i}");
        let name = format!("f{i}.txt");
        let id = save(
            &repo,
            ids.last(),
            &[("keep.txt", "same"), (name.as_str(), body.as_str())],
            i + 1,
        );
        ids.push(id);
    }
    let before: Vec<_> = ids
        .iter()
        .map(|i| tree_store::tree_of(repo.conn(), i.as_str()).unwrap())
        .collect();
    let rows: i64 = repo
        .conn()
        .query_row("SELECT count(*) FROM file_map", [], |r| r.get(0))
        .unwrap();
    assert!(rows >= 120);
    // Make it v3-shaped.
    repo.conn()
        .execute_batch(
            "DROP TABLE tree_deltas; DROP TABLE tree_delta_rows; PRAGMA user_version = 3;",
        )
        .unwrap();
    drop(repo);

    assert!(matches!(
        Repo::open(tmp.path()),
        Err(Error::MigrationRequired {
            found: 3,
            supported: 4
        })
    ));

    let conn = Connection::open(tmp.path().join(".velo/velo.db")).unwrap();
    let base = conn.total_changes();
    crate::db::migrate(&conn).unwrap();
    assert_eq!(conn.total_changes() - base, 0, "migration wrote rows");
    assert_eq!(crate::db::format_version(&conn).unwrap(), 4);
    let after_rows: i64 = conn
        .query_row("SELECT count(*) FROM file_map", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after_rows, rows);
    drop(conn);

    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    for (id, want) in ids.iter().zip(&before) {
        assert_eq!(
            &tree_store::tree_of(repo.conn(), id.as_str()).unwrap(),
            want
        );
        assert!(repo.snapshot(id).is_ok());
    }
}

#[test]
fn tree_of_applies_changes_removals_and_additions() {
    let (_t, repo) = setup();
    let (_k, d) = keyframe_and_delta(&repo);
    let tree = tree_store::tree_of(repo.conn(), &d).unwrap();
    let paths: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(paths, ["a", "dir/c", "new/d"]);
    assert_eq!(tree["a"].0, "h-a2");
    assert_eq!(tree["new/d"], ("h-d".to_string(), 0));
}

#[test]
fn a_delta_on_a_delta_resolves_newest_first() {
    let (_t, repo) = setup();
    let (_k, d) = keyframe_and_delta(&repo);
    let e = save(&repo, None, &[("q", "q")], 3).to_string();
    make_delta(repo.conn(), &e, &d, &[("a", None), ("dir/b", Some("h-b2"))]);
    let c = repo.conn();
    let tree = tree_store::tree_of(c, &e).unwrap();
    let paths: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(paths, ["dir/b", "dir/c", "new/d"]);
    assert_eq!(tree_store::file_at(c, &e, "a").unwrap(), None);
    assert_eq!(
        tree_store::file_at(c, &e, "dir/b").unwrap().unwrap().0,
        "h-b2"
    );
    assert_eq!(
        tree_store::file_at(c, &e, "new/d").unwrap().unwrap().0,
        "h-d"
    );
}

#[test]
fn file_at_reads_delta_rows_then_the_keyframe() {
    let (_t, repo) = setup();
    let (k, d) = keyframe_and_delta(&repo);
    let c = repo.conn();
    assert_eq!(tree_store::file_at(c, &d, "a").unwrap().unwrap().0, "h-a2");
    assert_eq!(tree_store::file_at(c, &d, "dir/b").unwrap(), None);
    assert_eq!(
        tree_store::file_at(c, &d, "dir/c").unwrap(),
        tree_store::file_at(c, &k, "dir/c").unwrap()
    );
    assert!(tree_store::file_at(c, &d, "dir/c").unwrap().is_some());
    assert_eq!(tree_store::file_at(c, &d, "nope").unwrap(), None);
    assert!(tree_store::file_at(c, &k, "dir/b").unwrap().is_some());
}

#[test]
fn files_at_reads_a_delta_snapshot() {
    let (_t, repo) = setup();
    let (_k, d) = keyframe_and_delta(&repo);
    let got =
        tree_store::files_at(repo.conn(), &d, &["a", "dir/b", "dir/c", "new/d", "x"]).unwrap();
    let mut keys: Vec<&str> = got.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["a", "dir/c", "new/d"]);
    assert_eq!(got["a"].0, "h-a2");
}

#[test]
fn has_tree_is_true_for_a_delta_even_with_no_rows() {
    let (_t, repo) = setup();
    let (_k, d) = keyframe_and_delta(&repo);
    assert!(tree_store::has_tree(repo.conn(), &d).unwrap());
    let e = save(&repo, None, &[("q", "q")], 5).to_string();
    make_delta(repo.conn(), &e, &d, &[]);
    assert!(tree_store::has_tree(repo.conn(), &e).unwrap());
    assert!(!tree_store::has_tree(repo.conn(), "unknown").unwrap());
}

#[test]
fn has_path_or_under_sees_through_a_delta() {
    let (_t, repo) = setup();
    let (_k, d) = keyframe_and_delta(&repo);
    let c = repo.conn();
    assert!(tree_store::has_path_or_under(c, &d, "a").unwrap());
    assert!(tree_store::has_path_or_under(c, &d, "new").unwrap());
    assert!(tree_store::has_path_or_under(c, &d, "dir").unwrap());
    assert!(tree_store::has_path_or_under(c, &d, "dir/c").unwrap());
    assert!(!tree_store::has_path_or_under(c, &d, "dir/b").unwrap());
    assert!(!tree_store::has_path_or_under(c, &d, "ne").unwrap());
}

#[test]
fn tree_with_index_cache_joins_a_delta_in_memory() {
    let (_t, repo) = setup();
    let (_k, d) = keyframe_and_delta(&repo);
    let c = repo.conn();
    c.execute_batch(
        "INSERT INTO index_cache VALUES ('a', 1, 2, 'h-a2');
         INSERT INTO index_cache VALUES ('dir/b', 3, 4, 'old');
         INSERT INTO index_cache VALUES ('untracked', 5, 6, 'u');",
    )
    .unwrap();
    let got = tree_store::tree_with_index_cache(c, &d).unwrap();
    assert_eq!(got.len(), 5);
    assert_eq!(got["a"], (Some("h-a2".into()), Some((1, 2, "h-a2".into()))));
    assert_eq!(got["dir/b"], (None, Some((3, 4, "old".into()))));
    assert_eq!(got["untracked"], (None, Some((5, 6, "u".into()))));
    assert_eq!(got["dir/c"].1, None);
    assert!(got["dir/c"].0.is_some());
    assert_eq!(got["new/d"], (Some("h-d".into()), None));
}

fn assert_corrupt<T: std::fmt::Debug>(r: crate::error::Result<T>, snap: &str) {
    match r {
        Err(Error::Corrupt { detail }) => assert!(detail.contains(snap), "{detail}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn a_cycle_is_corrupt_not_a_hang() {
    let (_t, repo) = setup();
    let a = save(&repo, None, &[("a", "1")], 1).to_string();
    let b = save(&repo, None, &[("b", "1")], 2).to_string();
    let c = repo.conn();
    c.execute("INSERT INTO tree_deltas VALUES (?, ?)", [&a, &b])
        .unwrap();
    c.execute("INSERT INTO tree_deltas VALUES (?, ?)", [&b, &a])
        .unwrap();
    assert_corrupt(tree_store::tree_of(c, &a), &a);
    assert_corrupt(tree_store::file_at(c, &a, "a"), &a);
}

#[test]
fn a_missing_base_is_corrupt() {
    let (_t, repo) = setup();
    let a = save(&repo, None, &[("a", "1")], 1).to_string();
    let c = repo.conn();
    c.execute("INSERT INTO tree_deltas VALUES (?, 'nowhere')", [&a])
        .unwrap();
    assert_corrupt(tree_store::tree_of(c, &a), &a);
    assert_corrupt(tree_store::file_at(c, &a, "a"), &a);
}

#[test]
fn a_base_with_no_rows_is_an_empty_keyframe() {
    let (_t, repo) = setup();
    let c = repo.conn();
    c.execute(
        "INSERT INTO snapshots (hash, message, branch) VALUES ('empty', 'm', 'main')",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO tree_deltas VALUES ('onempty', 'empty')", [])
        .unwrap();
    c.execute(
        "INSERT INTO tree_delta_rows VALUES ('onempty', 'x', 'hx', 0)",
        [],
    )
    .unwrap();
    assert_eq!(tree_store::tree_of(c, "onempty").unwrap().len(), 1);
}

#[test]
fn a_five_thousand_link_chain_is_corrupt() {
    let (_t, repo) = setup();
    let c = repo.conn();
    c.execute_batch("BEGIN").unwrap();
    for i in 0..=5000 {
        c.execute(
            "INSERT INTO snapshots (hash, message, branch) VALUES (?, 'm', 'main')",
            [format!("s{i}")],
        )
        .unwrap();
        if i > 0 {
            c.execute(
                "INSERT INTO tree_deltas VALUES (?, ?)",
                [format!("s{i}"), format!("s{}", i - 1)],
            )
            .unwrap();
        }
    }
    c.execute_batch("COMMIT").unwrap();
    assert_corrupt(tree_store::tree_of(c, "s5000"), "s5000");
    assert_corrupt(tree_store::file_at(c, "s5000", "p"), "s5000");
    // A chain within the bound still reads.
    assert!(tree_store::tree_of(c, "s4000").unwrap().is_empty());
}
