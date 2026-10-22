//! Blame skips a visit whose file object equals a parent's, and must not change
//! an answer by doing so: renames, merges and reverts pin exact origins.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::blame::{self, Blame};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// One whole-tree snapshot holding a single file, at a fixed millisecond.
fn save(
    repo: &Repo,
    parents: (Option<&SnapshotId>, Option<&SnapshotId>),
    path: &str,
    body: &str,
    message: &str,
    at_ms: i64,
    renames: &[(PathBuf, PathBuf)],
) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: parents.0,
            merge_parent: parents.1,
            message,
            entries: vec![TreeEntry::file(path, body.as_bytes().to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(at_ms),
            renames,
        })
        .unwrap()
}

fn blamed(
    repo: &Repo,
    file: &str,
    at: &SnapshotId,
    lines: Option<std::ops::Range<usize>>,
) -> Blame {
    blame::run(
        repo,
        Path::new(file),
        blame::Options {
            at: Some(at),
            lines,
            ..Default::default()
        },
    )
    .unwrap()
}

fn owners(b: &Blame) -> Vec<SnapshotId> {
    b.lines
        .iter()
        .map(|l| l.origin.as_ref().expect("attributed").hash.clone())
        .collect()
}

#[test]
fn a_pure_rename_is_skipped_and_earlier_lines_keep_their_origin() {
    let (_tmp, repo) = setup();
    let s1 = save(
        &repo,
        (None, None),
        "old.txt",
        "a\nb\nc\n",
        "write",
        1_000,
        &[],
    );
    let s2 = save(
        &repo,
        (Some(&s1), None),
        "new.txt",
        "a\nb\nc\n",
        "move",
        2_000,
        &[(PathBuf::from("old.txt"), PathBuf::from("new.txt"))],
    );
    let s3 = save(
        &repo,
        (Some(&s2), None),
        "new.txt",
        "a\nB\nc\n",
        "edit",
        3_000,
        &[],
    );
    let s4 = save(
        &repo,
        (Some(&s3), None),
        "new.txt",
        "a\nB\nc\n",
        "same",
        4_000,
        &[],
    );

    let b = blamed(&repo, "new.txt", &s4, None);
    assert_eq!(owners(&b), vec![s1.clone(), s3, s1]);
    assert!(b.crossed_a_rename());
}

/// Base, two branches each editing one end, and a merge that resolves with a
/// line of its own, followed by a snapshot that changes nothing.
fn merged() -> (TempDir, Repo, [SnapshotId; 4], SnapshotId) {
    let (tmp, repo) = setup();
    let base = save(
        &repo,
        (None, None),
        "f.txt",
        "1\n2\n3\n4\n5\n6\n",
        "base",
        1_000,
        &[],
    );
    let a = save(
        &repo,
        (Some(&base), None),
        "f.txt",
        "A1\n2\n3\n4\n5\n6\n",
        "A",
        2_000,
        &[],
    );
    let b = save(
        &repo,
        (Some(&base), None),
        "f.txt",
        "1\n2\n3\n4\n5\nB6\n",
        "B",
        3_000,
        &[],
    );
    let text = "A1\n2\n3\n4\n5\nB6\nresolved\n";
    let m = save(
        &repo,
        (Some(&a), Some(&b)),
        "f.txt",
        text,
        "merge",
        4_000,
        &[],
    );
    let tip = save(&repo, (Some(&m), None), "f.txt", text, "touch", 5_000, &[]);
    (tmp, repo, [base, a, b, m], tip)
}

#[test]
fn a_merge_owns_only_its_resolution_line() {
    let (_tmp, repo, [base, a, b, m], tip) = merged();
    let bl = blamed(&repo, "f.txt", &tip, None);
    let want = vec![a, base.clone(), base.clone(), base.clone(), base, b, m];
    assert_eq!(owners(&bl), want);
}

#[test]
fn a_windowed_blame_over_a_merge_agrees_with_the_whole_file() {
    let (_tmp, repo, [_, _, b, m], tip) = merged();
    let bl = blamed(&repo, "f.txt", &tip, Some(6..8));
    assert_eq!(owners(&bl), vec![b, m]);
}

#[test]
fn a_revert_credits_the_snapshot_that_restored_the_lines() {
    let (_tmp, repo) = setup();
    let r1 = save(&repo, (None, None), "f.txt", "x\ny\n", "one", 1_000, &[]);
    let r2 = save(
        &repo,
        (Some(&r1), None),
        "f.txt",
        "x\nz\n",
        "two",
        2_000,
        &[],
    );
    // Byte-identical to r1, a grandparent rather than the parent: its object
    // differs from its parent's, so this visit is still read and diffed.
    let r3 = save(
        &repo,
        (Some(&r2), None),
        "f.txt",
        "x\ny\n",
        "revert",
        3_000,
        &[],
    );
    let r4 = save(
        &repo,
        (Some(&r3), None),
        "f.txt",
        "x\ny\n",
        "touch",
        4_000,
        &[],
    );

    let bl = blamed(&repo, "f.txt", &r4, None);
    assert_eq!(owners(&bl), vec![r1, r3]);
}
