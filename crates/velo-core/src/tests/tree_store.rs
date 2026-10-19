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
