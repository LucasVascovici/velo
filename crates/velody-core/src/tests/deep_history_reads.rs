//! `history --file` answers from one lookup per path instead of a tree-wide
//! comparison per snapshot; the filtered result must be the same either way.

use std::path::Path;
use tempfile::TempDir;

use crate::commands::history;
use crate::tree::{SaveTree, TreeEntry};
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
    message: &str,
    ts: i64,
) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent,
            message,
            entries: files
                .iter()
                .map(|(p, b)| TreeEntry::file(*p, b.as_bytes().to_vec()))
                .collect(),
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

fn messages(repo: &Repo, from: &SnapshotId, path: &str) -> Vec<String> {
    history::run(
        repo,
        history::Options {
            from: Some(from),
            paths: &[Path::new(path)],
            ..Default::default()
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|e| e.message)
    .collect()
}

#[test]
fn file_filter_matches_edits_deletions_merges_and_directories() {
    let (_tmp, repo) = setup();
    let s1 = put(
        &repo,
        None,
        None,
        &[("a.txt", "1"), ("d/x.txt", "x")],
        "s1",
        1000,
    );
    let s2 = put(
        &repo,
        Some(&s1),
        None,
        &[("a.txt", "1"), ("d/x.txt", "x")],
        "s2",
        2000,
    );
    let s3 = put(
        &repo,
        Some(&s2),
        None,
        &[("a.txt", "2"), ("d/x.txt", "x")],
        "s3",
        3000,
    );
    // A side line that edits d/x.txt, then merged back keeping a.txt as it was.
    let s4 = put(
        &repo,
        Some(&s1),
        None,
        &[("a.txt", "1"), ("d/x.txt", "y")],
        "s4",
        3500,
    );
    let s5 = put(
        &repo,
        Some(&s3),
        Some(&s4),
        &[("a.txt", "2"), ("d/x.txt", "y")],
        "s5",
        4000,
    );
    let s6 = put(
        &repo,
        Some(&s5),
        None,
        &[("d/x.txt", "y")],
        "s6 drops a",
        5000,
    );
    let s7 = put(
        &repo,
        Some(&s6),
        None,
        &[("a.txt", "2"), ("d/x.txt", "y")],
        "s7 re-adds a",
        6000,
    );

    assert_eq!(
        messages(&repo, &s7, "a.txt"),
        ["s7 re-adds a", "s6 drops a", "s3", "s1"]
    );
    // The merge changed d/x.txt against its first parent, so it counts.
    assert_eq!(messages(&repo, &s7, "d/x.txt"), ["s5", "s4", "s1"],);
    assert_eq!(messages(&repo, &s7, "d"), ["s5", "s4", "s1"],);
    assert!(messages(&repo, &s7, "missing.txt").is_empty());
}
