//! Direct tests of the single-snapshot readers in `tree_store`.

use tempfile::TempDir;

use crate::tree::{SaveTree, TreeEntry};
use crate::tree_store;
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn snapshot_of(repo: &Repo, files: &[(String, String)]) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    let entries = files
        .iter()
        .map(|(p, b)| TreeEntry::file(p.clone(), b.as_bytes().to_vec()))
        .collect();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "tree",
            entries,
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap()
}

fn small(repo: &Repo) -> SnapshotId {
    snapshot_of(
        repo,
        &[
            ("a.txt".into(), "one".into()),
            ("dir/b.txt".into(), "two".into()),
        ],
    )
}

#[test]
fn tree_of_returns_every_path_in_order() {
    let (_t, repo) = setup();
    let snap = small(&repo);
    let tree = tree_store::tree_of(repo.conn(), &snap).unwrap();
    let paths: Vec<&str> = tree.keys().map(String::as_str).collect();
    assert_eq!(paths, ["a.txt", "dir/b.txt"]);
    assert!(tree.values().all(|(obj, _)| !obj.is_empty()));
}

#[test]
fn tree_of_an_unknown_or_empty_snapshot_is_empty() {
    let (_t, repo) = setup();
    assert!(tree_store::tree_of(repo.conn(), "").unwrap().is_empty());
    assert!(tree_store::tree_of(repo.conn(), "nope").unwrap().is_empty());
}

#[test]
fn file_at_finds_one_path_and_misses_another() {
    let (_t, repo) = setup();
    let snap = small(&repo);
    let tree = tree_store::tree_of(repo.conn(), &snap).unwrap();
    let got = tree_store::file_at(repo.conn(), &snap, "dir/b.txt").unwrap();
    assert_eq!(got.as_ref(), tree.get("dir/b.txt"));
    assert!(tree_store::file_at(repo.conn(), &snap, "missing")
        .unwrap()
        .is_none());
    assert!(tree_store::file_at(repo.conn(), "other", "a.txt")
        .unwrap()
        .is_none());
}

#[test]
fn files_at_spans_the_batch_boundary() {
    let (_t, repo) = setup();
    let files: Vec<(String, String)> = (0..1100)
        .map(|i| (format!("f{i:04}.txt"), format!("body {i}")))
        .collect();
    let snap = snapshot_of(&repo, &files);
    let tree = tree_store::tree_of(repo.conn(), &snap).unwrap();
    for n in [0usize, 1, 498, 499, 500, 998, 1100] {
        let mut asked: Vec<&str> = files.iter().take(n).map(|(p, _)| p.as_str()).collect();
        asked.push("absent.txt");
        let got = tree_store::files_at(repo.conn(), &snap, &asked).unwrap();
        assert_eq!(got.len(), n, "asking for {n} paths");
        for (p, entry) in &got {
            assert_eq!(Some(entry), tree.get(p));
        }
    }
}

#[test]
fn has_tree_and_path_probe() {
    let (_t, repo) = setup();
    let snap = small(&repo);
    assert!(tree_store::has_tree(repo.conn(), &snap).unwrap());
    assert!(!tree_store::has_tree(repo.conn(), "").unwrap());
    assert!(tree_store::has_path_or_under(repo.conn(), &snap, "dir").unwrap());
    assert!(tree_store::has_path_or_under(repo.conn(), &snap, "a.txt").unwrap());
    assert!(!tree_store::has_path_or_under(repo.conn(), &snap, "di").unwrap());
}

#[test]
fn tree_with_index_cache_joins_both_sides() {
    let (_t, repo) = setup();
    let snap = small(&repo);
    repo.conn()
        .execute(
            "INSERT INTO index_cache (path, mtime_ns, size, hash) VALUES ('a.txt', 5, 3, 'h'), ('only_cached', 6, 1, 'k')",
            [],
        )
        .unwrap();
    let got = tree_store::tree_with_index_cache(repo.conn(), &snap).unwrap();
    let tree = tree_store::tree_of(repo.conn(), &snap).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got["a.txt"].0.as_ref(), Some(&tree["a.txt"].0));
    assert_eq!(got["a.txt"].1, Some((5, 3, "h".to_string())));
    assert_eq!(got["dir/b.txt"].1, None);
    assert_eq!(got["only_cached"].0, None);
}

#[test]
fn write_rekey_and_delete_round_trip() {
    let (_t, repo) = setup();
    let conn = repo.conn();
    let rows = [("a.txt", "obj1", 0o100644), ("d/b.txt", "obj2", 0o100755)];
    tree_store::write_tree(conn, "snapX", Some("p"), &rows).unwrap();
    assert!(tree_store::has_tree(conn, "snapX").unwrap());
    let tree = tree_store::tree_of(conn, "snapX").unwrap();
    assert_eq!(tree["d/b.txt"], ("obj2".to_string(), 0o100755));

    tree_store::rekey_tree(conn, "snapX", "snapY").unwrap();
    assert!(!tree_store::has_tree(conn, "snapX").unwrap());
    let mut exported = tree_store::tree_rows_for_export(conn, "snapY").unwrap();
    exported.sort();
    assert_eq!(exported.len(), 2);
    assert_eq!(
        exported[0],
        ("a.txt".to_string(), "obj1".to_string(), 0o100644)
    );

    tree_store::write_tree(conn, "snapZ", None, &rows[..1]).unwrap();
    tree_store::delete_trees(conn, &["snapY", "snapZ", "absent"]).unwrap();
    assert!(!tree_store::has_tree(conn, "snapY").unwrap());
    assert!(!tree_store::has_tree(conn, "snapZ").unwrap());
    tree_store::delete_trees(conn, &[]).unwrap();
}

fn child_of(repo: &Repo, parent: &SnapshotId, files: &[(&str, &str)]) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    let entries = files
        .iter()
        .map(|(p, b)| TreeEntry::file(p.to_string(), b.as_bytes().to_vec()))
        .collect();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: Some(parent),
            merge_parent: None,
            message: "child",
            entries,
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(2),
            renames: &[],
        })
        .unwrap()
}

#[test]
fn changes_of_reports_add_modify_delete_against_the_first_parent() {
    let (_t, repo) = setup();
    let root = small(&repo);
    // a.txt modified, dir/b.txt deleted, c.txt added.
    let next = child_of(&repo, &root, &[("a.txt", "ONE"), ("c.txt", "three")]);
    let conn = repo.conn();

    let first = tree_store::changes_of(conn, &root).unwrap();
    let paths: Vec<&str> = first.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "dir/b.txt"]);
    assert!(first
        .iter()
        .all(|c| c.before.is_none() && c.after.is_some()));

    let before = tree_store::tree_of(conn, &root).unwrap();
    let after = tree_store::tree_of(conn, &next).unwrap();
    let got = tree_store::changes_of(conn, &next).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].path, "a.txt");
    assert_eq!(got[0].before.as_ref(), before.get("a.txt"));
    assert_eq!(got[0].after.as_ref(), after.get("a.txt"));
    assert_eq!(got[1].path, "c.txt");
    assert!(got[1].before.is_none() && got[1].after.is_some());
    assert_eq!(got[2].path, "dir/b.txt");
    assert!(got[2].before.is_some() && got[2].after.is_none());

    assert!(tree_store::changes_of(conn, "unknown").unwrap().is_empty());
}

#[test]
fn path_versions_distinguish_a_file_from_a_directory_prefix() {
    let (_t, repo) = setup();
    let root = small(&repo);
    let next = child_of(&repo, &root, &[("a.txt", "ONE"), ("dir/b.txt", "two")]);
    let conn = repo.conn();

    let versions = tree_store::PathVersions::load(conn, "a.txt").unwrap();
    assert!(!versions.is_empty());
    assert_eq!(
        versions.at(&root),
        tree_store::file_at(conn, &root, "a.txt").unwrap()
    );
    assert_ne!(versions.at(&root), versions.at(&next));
    assert_eq!(versions.at("nope"), None);
    assert!(!tree_store::PathVersions::is_directory(conn, "a.txt").unwrap());

    assert!(tree_store::PathVersions::is_directory(conn, "dir").unwrap());
    assert!(tree_store::PathVersions::load(conn, "dir")
        .unwrap()
        .is_empty());
    assert!(!tree_store::PathVersions::is_directory(conn, "di").unwrap());

    assert!(tree_store::changed_under(conn, &next, &root, "a.txt").unwrap());
    assert!(!tree_store::changed_under(conn, &next, &root, "dir").unwrap());
}

#[test]
fn referenced_objects_is_every_object_of_every_tree() {
    let (_t, repo) = setup();
    let root = small(&repo);
    let next = child_of(&repo, &root, &[("a.txt", "ONE"), ("dir/b.txt", "two")]);
    let conn = repo.conn();
    let got = tree_store::referenced_objects(conn).unwrap();
    let mut want = std::collections::HashSet::new();
    for snap in [&root, &next] {
        for (obj, _) in tree_store::tree_of(conn, snap).unwrap().values() {
            want.insert(obj.clone());
        }
    }
    assert_eq!(got, want);
    assert_eq!(got.len(), 3);
}

#[test]
fn for_each_tree_visits_every_tree_once_and_duplicates_are_counted() {
    let (_t, repo) = setup();
    let root = small(&repo);
    let next = child_of(&repo, &root, &[("a.txt", "ONE")]);
    let conn = repo.conn();
    let mut seen = Vec::new();
    tree_store::for_each_tree_in_order(conn, |snap, rows| {
        seen.push((snap, rows.len()));
        Ok(())
    })
    .unwrap();
    seen.sort();
    let mut want = vec![(root.to_string(), 2), (next.to_string(), 1)];
    want.sort();
    assert_eq!(seen, want);

    assert_eq!(tree_store::duplicate_rows(conn).unwrap(), 0);
    conn.execute(
        "INSERT INTO file_map (snapshot_hash, path, hash, mode)
         SELECT snapshot_hash, path, hash, mode FROM file_map WHERE path = 'a.txt'",
        [],
    )
    .unwrap();
    assert_eq!(tree_store::duplicate_rows(conn).unwrap(), 2);
    assert_eq!(tree_store::remove_duplicate_rows(conn).unwrap(), 2);
    assert_eq!(tree_store::duplicate_rows(conn).unwrap(), 0);
}
