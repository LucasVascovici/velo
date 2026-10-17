//! The tree accessor (`treestore`): whole-tree loads and point lookups over a
//! small history of a root, an edit, a delete, a merge and a trashed snapshot.

use tempfile::TempDir;

use crate::tree::{SaveTree, TreeEntry};
use crate::treestore::{self, Entry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn put(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    merge_parent: Option<&SnapshotId>,
    files: &[(&str, &str)],
    ts: i64,
) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent,
            message: &format!("step {ts}"),
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

fn paths(rows: &[(String, String, i64)]) -> Vec<&str> {
    rows.iter().map(|r| r.0.as_str()).collect()
}

/// root {a, b/c} -> edit {a changed, b/c} -> delete {a} dropped, b/c kept.
struct History {
    root: SnapshotId,
    edit: SnapshotId,
    delete: SnapshotId,
}

fn history(repo: &Repo) -> History {
    let root = put(repo, None, None, &[("a.txt", "one"), ("b/c.txt", "two")], 1);
    let edit = put(
        repo,
        Some(&root),
        None,
        &[("a.txt", "one!"), ("b/c.txt", "two")],
        2,
    );
    let delete = put(repo, Some(&edit), None, &[("b/c.txt", "two")], 3);
    History { root, edit, delete }
}

#[test]
fn tree_of_is_sorted_and_empty_for_the_empty_id() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    let rows = treestore::tree_of(conn, h.root.as_str()).unwrap();
    assert_eq!(paths(&rows), ["a.txt", "b/c.txt"]);
    assert!(treestore::tree_of(conn, "").unwrap().is_empty());
    assert!(treestore::tree_of(conn, "nonexistent").unwrap().is_empty());
    let rows = treestore::tree_of(conn, h.delete.as_str()).unwrap();
    assert_eq!(paths(&rows), ["b/c.txt"]);
}

#[test]
fn file_at_and_files_at_find_present_paths_and_omit_absent_ones() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    let root_a = treestore::file_at(conn, h.root.as_str(), "a.txt")
        .unwrap()
        .unwrap();
    let edit_a = treestore::file_at(conn, h.edit.as_str(), "a.txt")
        .unwrap()
        .unwrap();
    assert_ne!(root_a.hash, edit_a.hash);
    assert_eq!(
        treestore::file_at(conn, h.delete.as_str(), "a.txt").unwrap(),
        None
    );
    assert_eq!(
        treestore::file_at(conn, h.root.as_str(), "nope").unwrap(),
        None
    );

    let got = treestore::files_at(conn, h.root.as_str(), &["a.txt", "nope", "b/c.txt"]).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got["a.txt"], root_a);
    assert!(!got.contains_key("nope"));
}

#[test]
fn files_at_batches_past_one_statement() {
    let (_tmp, repo) = setup();
    let names: Vec<String> = (0..1100).map(|i| format!("f{i:04}.txt")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "x")).collect();
    let snap = put(&repo, None, None, &files, 1);
    let asked: Vec<&str> = names.iter().map(String::as_str).collect();
    let got = treestore::files_at(repo.conn(), snap.as_str(), &asked).unwrap();
    assert_eq!(got.len(), 1100);
}

#[test]
fn holds_path_or_under_counts_directories_but_not_prefixes() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    let hold =
        |s: &SnapshotId, p: &str| treestore::holds_path_or_under(conn, s.as_str(), p).unwrap();
    assert!(hold(&h.root, "a.txt"));
    assert!(hold(&h.root, "b"));
    assert!(hold(&h.root, "b/c.txt"));
    assert!(!hold(&h.root, "b/c"));
    assert!(!hold(&h.delete, "a.txt"));
    assert!(!hold(&h.root, "zzz"));
}

#[test]
fn changes_of_a_root_is_every_path() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let changes = treestore::changes_of(repo.conn(), h.root.as_str()).unwrap();
    let names: Vec<&str> = changes.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, ["a.txt", "b/c.txt"]);
    assert!(changes.iter().all(|c| c.1.is_some()));
}

#[test]
fn changes_of_an_edit_and_a_delete() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    let edit = treestore::changes_of(conn, h.edit.as_str()).unwrap();
    assert_eq!(edit.len(), 1);
    assert_eq!(edit[0].0, "a.txt");
    let new_a: Entry = treestore::file_at(conn, h.edit.as_str(), "a.txt")
        .unwrap()
        .unwrap();
    assert_eq!(edit[0].1, Some(new_a));

    let del = treestore::changes_of(conn, h.delete.as_str()).unwrap();
    assert_eq!(del, vec![("a.txt".to_string(), None)]);
}

#[test]
fn changes_of_a_merge_is_against_the_first_parent() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let side = put(
        &repo,
        Some(&h.root),
        None,
        &[("a.txt", "one"), ("b/c.txt", "two"), ("s.txt", "side")],
        4,
    );
    let merge = put(
        &repo,
        Some(&h.delete),
        Some(&side),
        &[("b/c.txt", "two"), ("s.txt", "side")],
        5,
    );
    let changes = treestore::changes_of(repo.conn(), merge.as_str()).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, "s.txt");
    assert!(changes[0].1.is_some());
}

#[test]
fn changes_of_a_trashed_snapshot_reads_its_parent_from_trash() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    conn.execute(
        "INSERT INTO trash (hash, message, branch, parent_hash, merge_parent, created_at_ms, deleted_at_ms)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms, 9
         FROM snapshots WHERE hash = ?",
        [h.delete.as_str()],
    )
    .unwrap();
    conn.execute("DELETE FROM snapshots WHERE hash = ?", [h.delete.as_str()])
        .unwrap();
    let del = treestore::changes_of(conn, h.delete.as_str()).unwrap();
    assert_eq!(del, vec![("a.txt".to_string(), None)]);
}

#[test]
fn path_in_all_covers_live_and_trashed_snapshots() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    conn.execute(
        "INSERT INTO trash (hash, message, branch, parent_hash, merge_parent, created_at_ms, deleted_at_ms)
         SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms, 9
         FROM snapshots WHERE hash = ?",
        [h.delete.as_str()],
    )
    .unwrap();
    conn.execute("DELETE FROM snapshots WHERE hash = ?", [h.delete.as_str()])
        .unwrap();

    let a = treestore::path_in_all(conn, "a.txt").unwrap();
    let mut keys: Vec<&str> = a.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut want = [h.root.as_str(), h.edit.as_str()];
    want.sort_unstable();
    assert_eq!(keys, want);
    assert_ne!(a[h.root.as_str()], a[h.edit.as_str()]);

    // The trashed snapshot still holds b/c.txt.
    let c = treestore::path_in_all(conn, "b/c.txt").unwrap();
    assert_eq!(c.len(), 3);
    assert!(c.contains_key(h.delete.as_str()));
    assert!(treestore::path_in_all(conn, "nope").unwrap().is_empty());
}

#[test]
fn any_path_under_and_changed_under() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    assert!(treestore::any_path_under(conn, "b").unwrap());
    assert!(!treestore::any_path_under(conn, "b/c.txt").unwrap());
    assert!(!treestore::any_path_under(conn, "a.txt").unwrap());
    let (r, e, d) = (h.root.as_str(), h.edit.as_str(), h.delete.as_str());
    assert!(treestore::changed_under(conn, r, "", "b").unwrap());
    assert!(!treestore::changed_under(conn, e, r, "b").unwrap());
    assert!(treestore::changed_under(conn, e, r, "a.txt").unwrap());
    assert!(treestore::changed_under(conn, d, e, "a.txt").unwrap());
}

#[test]
fn for_each_tree_visits_each_stored_tree_once_and_referenced_objects_is_their_union() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    let mut seen: Vec<(String, Vec<treestore::Row>)> = Vec::new();
    treestore::for_each_tree(conn, |s, rows| {
        seen.push((s.to_string(), rows.to_vec()));
        Ok(())
    })
    .unwrap();
    let mut ids: Vec<&str> = seen.iter().map(|s| s.0.as_str()).collect();
    ids.sort_unstable();
    let mut want = [h.root.as_str(), h.edit.as_str(), h.delete.as_str()];
    want.sort_unstable();
    assert_eq!(ids, want);
    for (s, rows) in &seen {
        assert_eq!(rows, &treestore::tree_of(conn, s).unwrap());
    }

    let union: std::collections::HashSet<String> = seen
        .iter()
        .flat_map(|(_, rows)| rows.iter().map(|r| r.1.clone()))
        .filter(|h| !h.is_empty())
        .collect();
    assert_eq!(treestore::referenced_objects(conn).unwrap(), union);
}

#[test]
fn duplicate_rows_counts_exact_repeats() {
    let (_tmp, repo) = setup();
    let h = history(&repo);
    let conn = repo.conn();
    assert_eq!(treestore::duplicate_rows(conn).unwrap(), 0);
    conn.execute(
        "INSERT INTO file_map (snapshot_hash, path, hash, mode)
         SELECT snapshot_hash, path, hash, mode FROM file_map WHERE snapshot_hash = ?",
        [h.root.as_str()],
    )
    .unwrap();
    assert_eq!(treestore::duplicate_rows(conn).unwrap(), 2);
}
