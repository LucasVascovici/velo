//! The tree a handle last committed is reused by the next save on top of it (22.3).
//!
//! The cache is keyed by snapshot id, and a snapshot's tree never changes, so
//! these tests pin the cases where it could go wrong anyway: a rewrite of the
//! stored rows (compact, repack) and a second handle writing in between.

use crate::commands::compact::{self, Range};
use crate::commands::{fsck, init, repack};
use crate::tree::{FileKind, SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};
use tempfile::TempDir;

fn setup() -> (TempDir, Repo) {
    crate::repo::set_test_keyframe_interval(3);
    let tmp = TempDir::new().unwrap();
    init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// One save on top of `parent`: every file the parent has is passed as
/// `Stored` (so the save reads the parent's tree), and `edit` is written
/// as new bytes.
fn save_on(repo: &Repo, parent: Option<&SnapshotId>, edit: (&str, &str), n: i64) -> SnapshotId {
    let mut entries: Vec<TreeEntry> = match parent {
        Some(p) => repo
            .tree_at(p)
            .unwrap()
            .into_iter()
            .filter(|f| f.path != edit.0)
            .map(|f| TreeEntry::stored(f.path, f.object, FileKind::Regular))
            .collect(),
        None => Vec::new(),
    };
    entries.push(TreeEntry::file(edit.0, edit.1.as_bytes().to_vec()));
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: "m",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(n),
            renames: &[],
        })
        .unwrap()
}

fn contents(repo: &Repo, id: &SnapshotId) -> Vec<(String, Vec<u8>)> {
    repo.tree_at(id)
        .unwrap()
        .into_iter()
        .map(|f| {
            let bytes = repo.read_object(&f.object).unwrap();
            (f.path, bytes)
        })
        .collect()
}

fn healthy(repo: &Repo) {
    let report = fsck::check(repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

const EDITS: [(&str, &str); 6] = [
    ("a.txt", "one\n"),
    ("b.txt", "two\n"),
    ("c/d.txt", "three\n"),
    ("a.txt", "four\n"),
    ("e.txt", "five\n"),
    ("c/d.txt", "six\n"),
];

#[test]
fn a_handle_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Repo>();
}

#[test]
fn consecutive_saves_give_the_same_ids_with_and_without_the_cache() {
    let (_a, cached) = setup();
    let (_b, cleared) = setup();
    let (mut pc, mut pu): (Option<SnapshotId>, Option<SnapshotId>) = (None, None);
    for (n, edit) in EDITS.iter().enumerate() {
        cleared.clear_last_tree();
        let c = save_on(&cached, pc.as_ref(), *edit, 10 + n as i64);
        let u = save_on(&cleared, pu.as_ref(), *edit, 10 + n as i64);
        assert_eq!(c, u, "save {n}");
        assert_eq!(contents(&cached, &c), contents(&cleared, &u));
        pc = Some(c);
        pu = Some(u);
    }
    healthy(&cached);
}

#[test]
fn a_save_on_an_older_parent_does_not_use_the_newer_cached_tree() {
    let (_t, repo) = setup();
    let first = save_on(&repo, None, ("a.txt", "one\n"), 1);
    let _second = save_on(&repo, Some(&first), ("b.txt", "two\n"), 2);
    let again = save_on(&repo, Some(&first), ("c.txt", "three\n"), 3);
    let paths: Vec<String> = contents(&repo, &again).into_iter().map(|f| f.0).collect();
    assert_eq!(paths, ["a.txt", "c.txt"]);
}

#[test]
fn a_save_after_compact_and_repack_on_the_same_handle_is_correct() {
    let (_t, repo) = setup();
    let mut ids: Vec<SnapshotId> = Vec::new();
    for (n, edit) in EDITS.iter().enumerate() {
        ids.push(save_on(&repo, ids.last(), *edit, 10 + n as i64));
    }
    let before = contents(&repo, ids.last().unwrap());

    let done = {
        let guard = repo.write().unwrap();
        let branch: BranchName = "main".parse().unwrap();
        compact::run(
            &guard,
            &branch,
            &[Range {
                oldest: &ids[1],
                newest: &ids[3],
            }],
            Default::default(),
        )
        .unwrap()
    };
    let newest = done
        .rewritten
        .iter()
        .find(|(old, _)| old == ids.last().unwrap())
        .map(|(_, new)| new.clone())
        .unwrap_or_else(|| ids.last().unwrap().clone());
    assert_eq!(contents(&repo, &newest), before);
    let next = save_on(&repo, Some(&newest), ("z.txt", "after compact\n"), 100);
    let mut expect = before.clone();
    expect.push(("z.txt".into(), b"after compact\n".to_vec()));
    assert_eq!(contents(&repo, &next), expect);

    repack::run(&repo, repack::Options::default()).unwrap();
    let last = save_on(&repo, Some(&next), ("y.txt", "after repack\n"), 101);
    expect.push((
        "y.txt".into(),
        b"after repack
"
        .to_vec(),
    ));
    expect.sort();
    assert_eq!(contents(&repo, &last), expect);
    healthy(&repo);
}

#[test]
fn a_second_handle_writing_in_between_is_not_overlooked() {
    let (tmp, first) = setup();
    let second = Repo::open(tmp.path()).unwrap();
    let s1 = save_on(&first, None, ("a.txt", "one\n"), 1);
    // The other handle extends s1; the first handle's cache still holds s1.
    let s2 = save_on(&second, Some(&s1), ("b.txt", "from the other handle\n"), 2);
    let s3 = save_on(&first, Some(&s2), ("c.txt", "three\n"), 3);
    let paths: Vec<String> = contents(&first, &s3).into_iter().map(|f| f.0).collect();
    assert_eq!(paths, ["a.txt", "b.txt", "c.txt"]);
    healthy(&first);
}
