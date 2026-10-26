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
    crate::repo::keyframes_only();
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

// ─── 20.3-c: scans and probes over deltas ────────────────────────────────────

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::commands::{blame, gc, history};

/// Everything `for_each_tree_in_order` hands out, keyed by snapshot, plus the
/// order it came in.
fn streamed(conn: &Connection) -> (BTreeMap<String, tree_store::StoredTree>, Vec<String>) {
    let mut out: BTreeMap<String, tree_store::StoredTree> = BTreeMap::new();
    let mut order = Vec::new();
    let unreached = tree_store::for_each_tree_in_order(conn, |snap, rows| {
        order.push(snap.clone());
        out.insert(
            snap,
            rows.into_iter().map(|(p, h, m)| (p, (h, m))).collect(),
        );
        Ok(())
    })
    .unwrap();
    assert!(unreached.is_empty(), "{unreached:?}");
    (out, order)
}

#[test]
fn referenced_objects_and_gc_see_an_object_only_a_delta_names() {
    let (_t, repo) = setup();
    let k = save(&repo, None, &[("a", "1")], 1);
    let d = save(&repo, Some(&k), &[("a", "1"), ("b", "only-in-delta")], 2);
    let conn = repo.conn();
    let (k, d) = (k.to_string(), d.to_string());
    let obj: String = conn
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = 'b'",
            [&d],
            |r| r.get(0),
        )
        .unwrap();
    make_delta(conn, &d, &k, &[("b", Some(obj.as_str()))]);
    let in_file_map: i64 = conn
        .query_row(
            "SELECT count(*) FROM file_map WHERE hash = ?",
            [&obj],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_file_map, 0);
    assert!(tree_store::referenced_objects(conn).unwrap().contains(&obj));

    let guard = repo.write().unwrap();
    let got = gc::run(
        &guard,
        gc::Options {
            keep_days: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(got.objects, 0);
    drop(guard);
    assert!(
        repo.objects().get(&obj).is_ok(),
        "gc deleted a delta's object"
    );
    assert_eq!(tree_store::tree_of(repo.conn(), &d).unwrap()["b"].0, obj);
}

/// keyframe `k`, deltas `d` (on k) and `e` (on d), a sibling delta `g` on k,
/// and a second keyframe `z`.
fn forest(repo: &Repo) -> [String; 5] {
    let (k, d) = keyframe_and_delta(repo);
    let e = save(repo, None, &[("q", "q")], 3).to_string();
    make_delta(repo.conn(), &e, &d, &[("a", None), ("dir/b", Some("h-b2"))]);
    let g = save(repo, None, &[("q", "g")], 4).to_string();
    make_delta(repo.conn(), &g, &k, &[("dir/c", None)]);
    let z = save(repo, None, &[("z", "z")], 5).to_string();
    [k, d, e, g, z]
}

#[test]
fn for_each_tree_streams_every_tree_once_equal_to_tree_of_even_with_a_trashed_base() {
    let (_t, repo) = setup();
    let ids = forest(&repo);
    let conn = repo.conn();
    // Trash the keyframe everything hangs on.
    conn.execute(
        "INSERT INTO trash (hash, message, branch)
         SELECT hash, message, branch FROM snapshots WHERE hash = ?",
        [&ids[0]],
    )
    .unwrap();
    conn.execute("DELETE FROM snapshots WHERE hash = ?", [&ids[0]])
        .unwrap();

    let (trees, order) = streamed(conn);
    assert_eq!(order.len(), 5, "{order:?}");
    assert_eq!(order.iter().collect::<HashSet<_>>().len(), 5);
    for id in &ids {
        assert_eq!(&trees[id], &tree_store::tree_of(conn, id).unwrap(), "{id}");
    }
    // Chain order: a base comes before the deltas on it.
    let pos = |s: &String| order.iter().position(|o| o == s).unwrap();
    assert!(pos(&ids[0]) < pos(&ids[1]) && pos(&ids[1]) < pos(&ids[2]));
    assert!(pos(&ids[0]) < pos(&ids[3]));
}

#[test]
fn for_each_tree_reports_unreachable_deltas_and_descends_from_an_empty_base() {
    let (_t, repo) = setup();
    let c = repo.conn();
    c.execute(
        "INSERT INTO snapshots (hash, message, branch) VALUES ('empty', 'm', 'main')",
        [],
    )
    .unwrap();
    c.execute_batch(
        "INSERT INTO tree_deltas VALUES ('onempty', 'empty');
         INSERT INTO tree_delta_rows VALUES ('onempty', 'x', 'hx', 0);
         INSERT INTO tree_deltas VALUES ('c1', 'c2');
         INSERT INTO tree_deltas VALUES ('c2', 'c1');
         INSERT INTO tree_deltas VALUES ('lost', 'nowhere');",
    )
    .unwrap();
    let mut seen = Vec::new();
    let unreached = tree_store::for_each_tree_in_order(c, |s, rows| {
        seen.push((s, rows.len()));
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, [("onempty".to_string(), 1)]);
    assert_eq!(unreached, ["c1", "c2", "lost"]);
}

#[test]
fn delete_orphan_trees_folds_what_a_remaining_delta_is_based_on() {
    let (_t, repo) = setup();
    let [k, d, e, g, _z] = forest(&repo);
    let c = repo.conn();
    // Strays: delta rows with no record, and a whole orphan chain.
    c.execute(
        "INSERT INTO tree_delta_rows VALUES ('ghost', 'p', 'h', 0)",
        [],
    )
    .unwrap();
    c.execute_batch(
        "INSERT INTO tree_deltas VALUES ('o2', 'o1');
         INSERT INTO tree_delta_rows VALUES ('o2', 'p', 'h', 0);
         INSERT INTO tree_deltas VALUES ('o1', 'o0');
         INSERT INTO tree_delta_rows VALUES ('o1', 'p', 'h', 0);
         INSERT INTO file_map VALUES ('o0', 'p', 'h', 0);",
    )
    .unwrap();
    // `k` and `d` leave the live set, but `e` (on `d`) and `g` (on `k`) stay.
    c.execute("DELETE FROM snapshots WHERE hash IN (?, ?)", [&k, &d])
        .unwrap();
    let n = tree_store::delete_orphan_trees(c, 128).unwrap();
    assert!(n >= 5, "{n}");
    let count = |sql: &str, h: &str| -> i64 { c.query_row(sql, [h], |r| r.get(0)).unwrap() };
    for h in ["o0", "o1", "o2", "ghost"] {
        for table in ["file_map", "tree_deltas", "tree_delta_rows"] {
            let sql = format!("SELECT count(*) FROM {table} WHERE snapshot_hash = ?");
            assert_eq!(count(&sql, h), 0, "{table} {h}");
        }
    }
    // `e` and `g` were based on the orphans `d` and `k`: gc folds those away, so
    // the two live snapshots are keyframes now and no orphan is left.
    for h in [&e, &g] {
        let sql = "SELECT count(*) FROM tree_deltas WHERE snapshot_hash = ?";
        assert_eq!(count(sql, h), 0);
        let sql = "SELECT count(*) FROM file_map WHERE snapshot_hash = ?";
        assert!(count(sql, h) > 0);
    }
    for h in [&k, &d] {
        let sql = "SELECT count(*) FROM file_map WHERE snapshot_hash = ?";
        assert_eq!(count(sql, h), 0);
    }
    assert_eq!(tree_store::orphan_delta_snapshots(c).unwrap(), 0);
}

#[test]
fn prune_stale_index_cache_keeps_a_path_a_delta_names() {
    let (_t, repo) = setup();
    let (_k, _d) = keyframe_and_delta(&repo);
    let c = repo.conn();
    c.execute_batch(
        "INSERT INTO index_cache VALUES ('new/d', 1, 1, 'h-d');
         INSERT INTO index_cache VALUES ('stale', 1, 1, 'x');",
    )
    .unwrap();
    assert_eq!(tree_store::prune_stale_index_cache(c).unwrap(), 1);
    let left: i64 = c
        .query_row(
            "SELECT count(*) FROM index_cache WHERE path = 'new/d'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 1);
}

#[test]
fn path_versions_and_probes_resolve_through_deltas() {
    let (_t, repo) = setup();
    let [k, d, e, g, _z] = forest(&repo);
    let c = repo.conn();
    let v = tree_store::PathVersions::load(c, "a").unwrap();
    assert_eq!(
        v.at(&k).unwrap().unwrap().0,
        tree_store::tree_of(c, &k).unwrap()["a"].0
    );
    assert_eq!(v.at(&d).unwrap().unwrap().0, "h-a2");
    assert_eq!(v.at(&e).unwrap(), None);
    assert_eq!(v.at(&g).unwrap(), v.at(&k).unwrap());
    let v = tree_store::PathVersions::load(c, "dir/b").unwrap();
    assert!(v.at(&k).unwrap().is_some());
    assert_eq!(v.at(&d).unwrap(), None);
    assert_eq!(v.at(&e).unwrap().unwrap().0, "h-b2");
    // Only a delta holds this path.
    let v = tree_store::PathVersions::load(c, "new/d").unwrap();
    assert!(!v.is_empty());
    assert_eq!(v.at(&e).unwrap().unwrap().0, "h-d");
    assert!(tree_store::PathVersions::is_directory(c, "new").unwrap());
    assert!(!tree_store::PathVersions::is_directory(c, "ne").unwrap());

    // changed_under on a delta against its base reads the delta rows.
    assert!(tree_store::changed_under(c, &d, &k, "a").unwrap());
    assert!(tree_store::changed_under(c, &d, &k, "dir").unwrap());
    assert!(tree_store::changed_under(c, &d, &k, "new").unwrap());
    assert!(!tree_store::changed_under(c, &g, &k, "a").unwrap());
    assert!(tree_store::changed_under(c, &g, &k, "dir/c").unwrap());
    // A pairing that is not delta-on-base falls back to the trees.
    assert!(tree_store::changed_under(c, &e, &k, "a").unwrap());
    assert!(!tree_store::changed_under(c, &e, &d, "dir/c").unwrap());
    assert!(tree_store::changed_under(c, &d, "", "new").unwrap());

    let changes = tree_store::changes_of(c, &d).unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].path, "a");
    assert_eq!(changes[1].path, "dir/b");
    assert!(changes[1].after.is_none() && changes[1].before.is_some());
    assert!(changes[2].before.is_none());
}

/// Rewrite every snapshot of `repo` after the first as a delta against its
/// first parent, using the trees `repo` had as keyframes.
fn deltaize(repo: &Repo, ids: &[SnapshotId]) {
    let trees: Vec<_> = ids
        .iter()
        .map(|i| tree_store::tree_of(repo.conn(), i.as_str()).unwrap())
        .collect();
    for i in 1..ids.len() {
        let (before, after) = (&trees[i - 1], &trees[i]);
        let mut rows: Vec<(&str, Option<&str>)> = Vec::new();
        for (p, (h, _)) in after {
            if before.get(p) != after.get(p) {
                rows.push((p, Some(h)));
            }
        }
        for p in before.keys() {
            if !after.contains_key(p) {
                rows.push((p, None));
            }
        }
        make_delta(repo.conn(), ids[i].as_str(), ids[i - 1].as_str(), &rows);
    }
}

#[test]
fn history_file_and_blame_match_an_all_keyframe_repository() {
    let steps: &[&[(&str, &str)]] = &[
        &[("a.txt", "one\ntwo\n"), ("other", "x")],
        &[("a.txt", "one\ntwo\nthree\n"), ("other", "x")],
        &[("a.txt", "one\ntwo\nthree\n"), ("other", "y")],
        &[("a.txt", "ONE\ntwo\nthree\n"), ("other", "y")],
        &[("other", "y"), ("b.txt", "b")],
        &[("a.txt", "ONE\ntwo\nthree\nfour\n"), ("other", "y")],
    ];
    let build = || {
        let (t, repo) = setup();
        let mut ids: Vec<SnapshotId> = Vec::new();
        for (i, files) in steps.iter().enumerate() {
            let id = save(&repo, ids.last(), files, i as i64 + 1);
            ids.push(id);
        }
        (t, repo, ids)
    };
    let (_ta, plain, ids_a) = build();
    let (_tb, delta, ids_b) = build();
    assert_eq!(ids_a, ids_b);
    deltaize(&delta, &ids_b);
    let n: i64 = delta
        .conn()
        .query_row("SELECT count(*) FROM tree_deltas", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, steps.len() as i64 - 1);

    for file in ["a.txt", "other", "b.txt", "nope"] {
        let paths = [Path::new(file)];
        let list = |repo: &Repo| -> Vec<String> {
            history::run(
                repo,
                history::Options {
                    paths: &paths,
                    ..Default::default()
                },
            )
            .unwrap()
            .entries
            .iter()
            .map(|e| e.hash.to_string())
            .collect()
        };
        assert_eq!(list(&plain), list(&delta), "history of {file}");
    }
    let blame_of = |repo: &Repo, tip: &SnapshotId| -> Vec<(usize, String, Option<String>)> {
        blame::run(
            repo,
            Path::new("a.txt"),
            blame::Options {
                at: Some(tip),
                ..Default::default()
            },
        )
        .unwrap()
        .lines
        .iter()
        .map(|l| {
            (
                l.line_no,
                l.text.clone(),
                l.origin.as_ref().map(|o| o.hash.to_string()),
            )
        })
        .collect()
    };
    for tip in [&ids_a[3], &ids_a[5]] {
        let want = blame_of(&plain, tip);
        assert!(!want.is_empty());
        assert_eq!(blame_of(&delta, tip), want);
    }
}

// ─── 20.4-a: the write path ──────────────────────────────────────────────────

fn setup_k(k: u32) -> (TempDir, Repo) {
    let (tmp, mut repo) = setup();
    repo.set_keyframe_interval(k);
    (tmp, repo)
}

fn base_of(repo: &Repo, snap: &SnapshotId) -> Option<String> {
    repo.conn()
        .query_row(
            "SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?",
            [snap.as_str()],
            |r| r.get(0),
        )
        .ok()
}

fn delta_rows(repo: &Repo, snap: &SnapshotId) -> Vec<(String, Option<String>)> {
    let mut stmt = repo
        .conn()
        .prepare("SELECT path, hash FROM tree_delta_rows WHERE snapshot_hash = ? ORDER BY path")
        .unwrap();
    let rows = stmt
        .query_map([snap.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

fn keyframe_rows_of(repo: &Repo, snap: &SnapshotId) -> i64 {
    repo.conn()
        .query_row(
            "SELECT count(*) FROM file_map WHERE snapshot_hash = ?",
            [snap.as_str()],
            |r| r.get(0),
        )
        .unwrap()
}

fn write_wt(root: &std::path::Path, name: &str, body: &str) {
    std::fs::write(root.join(name), body).unwrap();
}

fn save_wt(repo: &Repo, msg: &str, amend: bool) -> Result<(), Error> {
    crate::commands::save::run(
        &repo.write().unwrap(),
        Some(msg),
        crate::commands::save::Options {
            amend,
            ..Default::default()
        },
    )
    .map(drop)
}

fn head(repo: &Repo) -> SnapshotId {
    let h: String = repo
        .conn()
        .query_row(
            "SELECT hash FROM snapshots WHERE branch = 'main' \
             ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    SnapshotId::from_stored(h)
}

#[test]
fn k4_stores_keyframes_at_depth_zero_and_four_and_canonical_deltas() {
    let (_t, repo) = setup_k(4);
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..10usize {
        let mut files: Vec<(String, String)> = vec![("keep".into(), "same".into())];
        files.push(("mod".into(), format!("v{i}")));
        // `gone` exists only at step 1, so step 2 deletes it.
        if i == 1 {
            files.push(("gone".into(), "x".into()));
        }
        files.push((format!("new{i}"), "n".into()));
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let id = save(&repo, ids.last(), &refs, i as i64 + 1);
        ids.push(id);
    }
    for (i, id) in ids.iter().enumerate() {
        let keyframe = i % 4 == 0;
        assert_eq!(base_of(&repo, id).is_none(), keyframe, "step {i}");
        assert_eq!(keyframe_rows_of(&repo, id) > 0, keyframe, "step {i}");
    }
    // Step 2 against step 1: modifies `mod`, adds `new2`, deletes `gone` and
    // `new1` (neither is in step 2's tree).
    assert_eq!(base_of(&repo, &ids[2]).as_deref(), Some(ids[1].as_str()));
    let rows = delta_rows(&repo, &ids[2]);
    let null: Vec<&str> = rows
        .iter()
        .filter(|r| r.1.is_none())
        .map(|r| r.0.as_str())
        .collect();
    let set: Vec<&str> = rows
        .iter()
        .filter(|r| r.1.is_some())
        .map(|r| r.0.as_str())
        .collect();
    assert_eq!(null, ["gone", "new1"]);
    assert_eq!(set, ["mod", "new2"]);
    // Every tree reads back whole.
    for (i, id) in ids.iter().enumerate() {
        let t = tree_store::tree_of(repo.conn(), id.as_str()).unwrap();
        assert!(
            t.contains_key("keep") && t.contains_key(&format!("new{i}")),
            "step {i}"
        );
        assert_eq!(t.len(), if i == 1 { 4 } else { 3 }, "step {i}");
    }
}

#[test]
fn k1_stores_every_tree_whole() {
    let (_t, repo) = setup_k(1);
    let a = save(&repo, None, &[("a", "1")], 1);
    let b = save(&repo, Some(&a), &[("a", "2")], 2);
    assert!(base_of(&repo, &b).is_none());
    assert_eq!(keyframe_rows_of(&repo, &b), 1);
}

#[test]
fn a_merge_snapshot_is_a_delta_on_its_first_parent() {
    let (_t, repo) = setup_k(8);
    let root = save(&repo, None, &[("a", "1")], 1);
    let first = save(&repo, Some(&root), &[("a", "1"), ("b", "2")], 2);
    let other = save(&repo, Some(&root), &[("a", "1"), ("c", "3")], 3);
    let branch: BranchName = "main".parse().unwrap();
    let merged = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: Some(&first),
            merge_parent: Some(&other),
            message: "merge",
            entries: ["a", "b", "c"]
                .iter()
                .map(|p| TreeEntry::file(*p, p.as_bytes().to_vec()))
                .collect(),
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(4),
            renames: &[],
        })
        .unwrap();
    assert_eq!(base_of(&repo, &merged).as_deref(), Some(first.as_str()));
    assert_eq!(
        tree_store::tree_of(repo.conn(), merged.as_str())
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn amend_deltas_against_the_replaced_snapshots_parent() {
    let (tmp, repo) = setup_k(8);
    write_wt(tmp.path(), "f.txt", "one");
    save_wt(&repo, "one", false).unwrap();
    let first = head(&repo);
    write_wt(tmp.path(), "f.txt", "two");
    write_wt(tmp.path(), "g.txt", "g");
    save_wt(&repo, "two", false).unwrap();
    let replaced = head(&repo);
    write_wt(tmp.path(), "g.txt", "g2");
    save_wt(&repo, "two again", true).unwrap();
    let amended = head(&repo);
    assert_ne!(amended, replaced);
    assert_eq!(base_of(&repo, &amended).as_deref(), Some(first.as_str()));
    let t = tree_store::tree_of(repo.conn(), amended.as_str()).unwrap();
    assert!(
        t.contains_key("g.txt") && t.contains_key("f.txt"),
        "the file the replaced snapshot introduced is kept"
    );
    assert!(!tree_store::has_tree(repo.conn(), replaced.as_str()).unwrap());
}

#[test]
fn amend_folds_a_stash_that_is_based_on_the_snapshot() {
    let (tmp, repo) = setup_k(8);
    write_wt(tmp.path(), "f.txt", "one");
    save_wt(&repo, "one", false).unwrap();
    let tip = head(&repo);
    write_wt(tmp.path(), "f.txt", "dirty");
    crate::commands::stash::push(&repo.write().unwrap(), None).unwrap();
    // The stash is a delta on the tip.
    let shelf: String = repo
        .conn()
        .query_row("SELECT snapshot_hash FROM stash", [], |r| r.get(0))
        .unwrap();
    let base: String = repo
        .conn()
        .query_row(
            "SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?",
            [&shelf],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(base, tip.as_str());
    let before = tree_store::tree_of(repo.conn(), &shelf).unwrap();
    write_wt(tmp.path(), "f.txt", "amended");
    // The amended snapshot's tree goes; the stash keeps its own, now stored
    // whole (the fold of 20.6 rewrites a dependant rather than refusing).
    save_wt(&repo, "one", true).unwrap();
    assert!(!tree_store::has_tree(repo.conn(), tip.as_str()).unwrap());
    assert_eq!(tree_store::tree_of(repo.conn(), &shelf).unwrap(), before);
}

#[test]
fn squashing_a_tip_range_leaves_a_delta_on_the_range_base() {
    let (tmp, repo) = setup_k(8);
    for (i, body) in ["a", "b", "c"].iter().enumerate() {
        write_wt(tmp.path(), "f.txt", body);
        write_wt(tmp.path(), &format!("n{i}.txt"), "n");
        save_wt(&repo, "step", false).unwrap();
    }
    let all: Vec<String> = repo
        .conn()
        .prepare("SELECT hash FROM snapshots ORDER BY created_at_ms, rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let want = tree_store::tree_of(repo.conn(), &all[2]).unwrap();
    crate::commands::squash::run(&repo.write().unwrap(), 2, "squashed").unwrap();
    let new = head(&repo);
    assert_eq!(base_of(&repo, &new).as_deref(), Some(all[0].as_str()));
    assert_eq!(
        tree_store::tree_of(repo.conn(), new.as_str()).unwrap(),
        want
    );
    for gone in &all[1..] {
        assert!(!tree_store::has_tree(repo.conn(), gone).unwrap());
    }
}

#[test]
fn delete_trees_refuses_while_an_outside_delta_depends_on_a_member() {
    let (_t, repo) = setup_k(8);
    let a = save(&repo, None, &[("a", "1")], 1);
    let b = save(&repo, Some(&a), &[("a", "2")], 2);
    let c = save(&repo, Some(&b), &[("a", "3")], 3);
    assert!(base_of(&repo, &c).is_some());
    let err = tree_store::delete_trees(repo.conn(), &[a.as_str(), b.as_str()]).unwrap_err();
    match err {
        Error::Corrupt { detail } => {
            assert!(
                detail.contains("is the base of 1 delta snapshot(s)"),
                "{detail}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(tree_store::has_tree(repo.conn(), a.as_str()).unwrap());
    // The whole chain goes together, deltas and keyframe alike.
    tree_store::delete_trees(repo.conn(), &[a.as_str(), b.as_str(), c.as_str()]).unwrap();
    for s in [&a, &b, &c] {
        assert!(!tree_store::has_tree(repo.conn(), s.as_str()).unwrap());
    }
    let left: i64 = repo
        .conn()
        .query_row(
            "SELECT (SELECT count(*) FROM tree_deltas) + (SELECT count(*) FROM tree_delta_rows)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn rekey_moves_a_delta_its_rows_and_its_children() {
    let (_t, repo) = setup_k(8);
    let a = save(&repo, None, &[("a", "1")], 1);
    let b = save(&repo, Some(&a), &[("a", "2")], 2);
    let c = save(&repo, Some(&b), &[("a", "3")], 3);
    tree_store::rekey_tree(repo.conn(), b.as_str(), "renamed-b").unwrap();
    let base: String = repo
        .conn()
        .query_row(
            "SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?",
            [c.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(base, "renamed-b");
    assert_eq!(
        tree_store::tree_of(repo.conn(), "renamed-b").unwrap().len(),
        1
    );
    let rows: i64 = repo
        .conn()
        .query_row(
            "SELECT count(*) FROM tree_delta_rows WHERE snapshot_hash = ?",
            [b.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn saving_nothing_new_and_the_same_tree_twice_still_behave() {
    let (tmp, repo) = setup_k(8);
    write_wt(tmp.path(), "f.txt", "one");
    save_wt(&repo, "one", false).unwrap();
    let before = head(&repo);
    // Nothing changed: no new snapshot.
    let _ = save_wt(&repo, "again", false);
    assert_eq!(head(&repo), before);
    // The same tree reached twice is one snapshot, with one stored tree.
    let a = save(&repo, None, &[("x", "1")], 10);
    let b = save(&repo, None, &[("x", "1")], 10);
    assert_eq!(a, b);
    assert_eq!(keyframe_rows_of(&repo, &a), 1);
}

#[test]
fn materializing_dependants_lets_a_prefix_be_deleted_without_changing_any_tree() {
    let (_t, repo) = setup_k(8);
    let a = save(&repo, None, &[("a", "1")], 1);
    let b = save(&repo, Some(&a), &[("a", "2"), ("b", "x")], 2);
    let c = save(&repo, Some(&b), &[("a", "3"), ("b", "x")], 3);
    let want = tree_store::tree_of(repo.conn(), c.as_str()).unwrap();
    tree_store::materialize_dependants(repo.conn(), &[a.as_str(), b.as_str()]).unwrap();
    tree_store::delete_trees(repo.conn(), &[a.as_str(), b.as_str()]).unwrap();
    assert!(base_of(&repo, &c).is_none());
    assert_eq!(tree_store::tree_of(repo.conn(), c.as_str()).unwrap(), want);
}
