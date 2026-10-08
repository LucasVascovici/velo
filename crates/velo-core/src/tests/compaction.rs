//! Compaction (`commands::compact`): squashing ranges and the record it leaves.

use std::path::PathBuf;
use std::sync::mpsc;
use tempfile::TempDir;

use crate::commands::compact::{self, Range};
use crate::commands::{fsck, resolve_snapshot_id, tag};
use crate::events::{Event, Ref};
use crate::tree::{SaveTree, TreeEntry};
use crate::{Author, BranchName, Error, Repo, SnapshotId, SnapshotMeta, TagName};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

#[allow(clippy::too_many_arguments)]
fn put(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    merge_parent: Option<&SnapshotId>,
    entries: Vec<TreeEntry>,
    meta: SnapshotMeta,
    author: Option<&Author>,
    renames: &[(PathBuf, PathBuf)],
    ts: i64,
) -> SnapshotId {
    let branch: BranchName = branch.parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent,
            message: &format!("step {ts}"),
            entries,
            meta,
            author,
            timestamp_ms: Some(ts),
            renames,
        })
        .unwrap()
}

/// A snapshot on main holding one file.
fn simple(repo: &Repo, parent: Option<&SnapshotId>, file: &str, body: &str, ts: i64) -> SnapshotId {
    put(
        repo,
        "main",
        parent,
        None,
        vec![TreeEntry::file(file, body.as_bytes().to_vec())],
        SnapshotMeta::new(),
        None,
        &[],
        ts,
    )
}

/// `n` snapshots on main, each with its own `f.txt` body, oldest first.
fn chain(repo: &Repo, n: i64) -> Vec<SnapshotId> {
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..n {
        let id = simple(
            repo,
            ids.last(),
            "f.txt",
            &format!("body {i}\n"),
            1_000 * (i + 1),
        );
        ids.push(id);
    }
    ids
}

fn scalar(repo: &Repo, sql: &str) -> i64 {
    repo.conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn count(repo: &Repo) -> i64 {
    scalar(repo, "SELECT COUNT(*) FROM snapshots")
}

fn all_hashes(repo: &Repo) -> Vec<String> {
    let mut stmt = repo
        .conn()
        .prepare("SELECT hash FROM snapshots ORDER BY hash")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn compact_one(
    repo: &Repo,
    oldest: &SnapshotId,
    newest: &SnapshotId,
) -> crate::Result<compact::Compaction> {
    let guard = repo.write()?;
    compact::run(
        &guard,
        &main_branch(),
        &[Range { oldest, newest }],
        Default::default(),
    )
}

fn healthy(repo: &Repo) {
    let report = fsck::check(repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

#[test]
fn a_middle_range_collapses_and_later_snapshots_are_reminted() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 6);
    let done = compact_one(&repo, &ids[1], &ids[3]).unwrap();

    assert_eq!(count(&repo), 4);
    assert_eq!(done.squashed.len(), 1);
    assert_eq!(done.squashed[0].removed, ids[1..=3].to_vec());
    assert_eq!(done.rewritten.len(), 2);
    assert_eq!(done.rewritten[0].0, ids[4]);
    assert_eq!(done.rewritten[1].0, ids[5]);

    let into = &done.squashed[0].into;
    let squashed = repo.snapshot(into).unwrap();
    assert_eq!(squashed.parent.as_ref(), Some(&ids[0]));
    assert_eq!(squashed.message, "step 4000");
    assert_eq!(repo.read_file_at(into, "f.txt").unwrap(), b"body 3\n");

    for (n, (old_i, (_, new))) in [4usize, 5].iter().zip(done.rewritten.iter()).enumerate() {
        let ts = 1_000 * (*old_i as i64 + 1);
        let entry = repo.snapshot(new).unwrap();
        assert_eq!(entry.message, format!("step {ts}"));
        assert_eq!(entry.created_at.timestamp_millis(), ts);
        let expected_parent = if n == 0 { into } else { &done.rewritten[0].1 };
        assert_eq!(entry.parent.as_ref(), Some(expected_parent));
        let body = format!("body {old_i}\n");
        assert_eq!(repo.read_file_at(new, "f.txt").unwrap(), body.as_bytes());
    }
    assert_eq!(
        repo.branch_tip(&main_branch()).unwrap().as_ref(),
        Some(&done.rewritten[1].1)
    );
    healthy(&repo);
}

#[test]
fn two_ranges_in_one_call() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 7);
    let guard = repo.write().unwrap();
    let done = compact::run(
        &guard,
        &main_branch(),
        &[
            Range {
                oldest: &ids[4],
                newest: &ids[5],
            },
            Range {
                oldest: &ids[1],
                newest: &ids[2],
            },
        ],
        Default::default(),
    )
    .unwrap();
    drop(guard);
    // 0, S1, 3', S2, 6'
    assert_eq!(count(&repo), 5);
    assert_eq!(done.squashed.len(), 2);
    assert_eq!(done.rewritten.len(), 2);
    assert_eq!(done.rewritten[0].0, ids[3]);
    assert_eq!(done.rewritten[1].0, ids[6]);
    healthy(&repo);
}

#[test]
fn old_ids_report_where_they_went_across_two_compactions() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 6);
    let first = compact_one(&repo, &ids[1], &ids[2]).unwrap();
    let into1 = first.squashed[0].into.clone();
    // The first compaction left ids[0], into1, 3', 4', 5'. Squash into1..3'.
    let third = first.rewritten[0].1.clone();
    let second = compact_one(&repo, &into1, &third).unwrap();
    let live = second.squashed[0].into.clone();

    for old in [&ids[1], &ids[2], &ids[3], &into1] {
        match resolve_snapshot_id(&repo, old.as_str()) {
            Err(Error::Compacted { into, .. }) => assert_eq!(into, live),
            other => panic!("expected Compacted, got {other:?}"),
        }
        assert!(matches!(
            repo.snapshot(old),
            Err(Error::Compacted { into, .. }) if into == live
        ));
        assert!(matches!(
            repo.tree_at(old),
            Err(Error::Compacted { into, .. }) if into == live
        ));
        assert!(matches!(
            repo.read_file_at(old, "f.txt"),
            Err(Error::Compacted { into, .. }) if into == live
        ));
        assert!(matches!(
            repo.snapshot_meta(old),
            Err(Error::Compacted { into, .. }) if into == live
        ));
    }
    assert!(matches!(
        repo.snapshot(&ids[5]),
        Err(Error::Compacted { .. })
    ));
    let rows = scalar(&repo, "SELECT COUNT(*) FROM compactions");
    assert!(rows >= 8, "{rows}");
    assert!(repo.snapshot(&live).is_ok());
    let msg = Error::Compacted {
        id: "abc".into(),
        into: live.clone(),
    }
    .to_string();
    assert_eq!(msg, format!("snapshot abc was compacted into {live}"));
}

#[test]
fn a_unique_prefix_of_a_compacted_id_is_compacted_too() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 4);
    let done = compact_one(&repo, &ids[1], &ids[2]).unwrap();
    let prefix = &ids[1].as_str()[..20];
    match resolve_snapshot_id(&repo, prefix) {
        Err(Error::Compacted { into, .. }) => assert_eq!(into, done.squashed[0].into),
        other => panic!("expected Compacted, got {other:?}"),
    }
    // Not a prefix of anything: still NotFound.
    assert!(matches!(
        resolve_snapshot_id(&repo, "ffffffffffffffffffff"),
        Err(Error::NotFound { .. })
    ));
}

fn refused(repo: &Repo, oldest: &SnapshotId, newest: &SnapshotId, why: &str) {
    let before = (all_hashes(repo), repo.head_token().unwrap());
    match compact_one(repo, oldest, newest) {
        Err(Error::InvalidInput { detail }) => {
            assert!(detail.contains(why), "{detail:?} should mention {why:?}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(before, (all_hashes(repo), repo.head_token().unwrap()));
    assert_eq!(scalar(repo, "SELECT COUNT(*) FROM compactions"), 0);
}

#[test]
fn a_tagged_snapshot_in_the_range_is_refused() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 5);
    let name: TagName = "v1".parse().unwrap();
    tag::create(&repo.write().unwrap(), &name, Some(&ids[2]), false).unwrap();
    refused(&repo, &ids[1], &ids[3], "tagged");
}

#[test]
fn a_merge_snapshot_in_the_range_is_refused() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 2);
    let side = put(
        &repo,
        "side",
        Some(&ids[0]),
        None,
        vec![TreeEntry::file("s.txt", b"s\n".to_vec())],
        SnapshotMeta::new(),
        None,
        &[],
        1_500,
    );
    let merge = put(
        &repo,
        "main",
        Some(&ids[1]),
        Some(&side),
        vec![TreeEntry::file("f.txt", b"m\n".to_vec())],
        SnapshotMeta::new(),
        None,
        &[],
        3_000,
    );
    let tip = simple(&repo, Some(&merge), "f.txt", "t\n", 4_000);
    refused(&repo, &ids[1], &tip, "merge");
}

#[test]
fn a_published_snapshot_is_refused() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 5);
    repo.conn()
        .execute(
            "INSERT INTO remote_refs (remote, branch, hash) VALUES ('origin', 'main', ?)",
            [&ids[4]],
        )
        .unwrap();
    refused(&repo, &ids[1], &ids[2], "remote-tracking");
}

#[test]
fn a_published_snapshot_blocks_a_rewrite_that_reaches_it() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 5);
    // Only ids[3] is published; the range 0..=1 is its ancestry, and the
    // rewrite also re-mints ids[3] itself.
    repo.conn()
        .execute(
            "INSERT INTO remote_refs (remote, branch, hash) VALUES ('origin', 'main', ?)",
            [&ids[3]],
        )
        .unwrap();
    refused(&repo, &ids[0], &ids[1], "remote-tracking");
}

#[test]
fn a_branch_forked_from_a_member_is_refused() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 5);
    put(
        &repo,
        "side",
        Some(&ids[2]),
        None,
        vec![TreeEntry::file("s.txt", b"s\n".to_vec())],
        SnapshotMeta::new(),
        None,
        &[],
        9_000,
    );
    refused(&repo, &ids[1], &ids[3], "outside the rewrite");
}

#[test]
fn malformed_ranges_are_refused() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 5);
    for (a, b) in [(2usize, 2usize), (3, 1)] {
        assert!(matches!(
            compact_one(&repo, &ids[a], &ids[b]),
            Err(Error::InvalidInput { .. })
        ));
    }
    let guard = repo.write().unwrap();
    let overlapping = compact::run(
        &guard,
        &main_branch(),
        &[
            Range {
                oldest: &ids[0],
                newest: &ids[2],
            },
            Range {
                oldest: &ids[2],
                newest: &ids[3],
            },
        ],
        Default::default(),
    );
    assert!(matches!(overlapping, Err(Error::InvalidInput { .. })));
    assert_eq!(count(&repo), 5);
}

#[test]
fn a_tag_on_a_reminted_descendant_follows_it() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 5);
    let name: TagName = "release".parse().unwrap();
    tag::create(&repo.write().unwrap(), &name, Some(&ids[4]), false).unwrap();
    let done = compact_one(&repo, &ids[1], &ids[2]).unwrap();
    let new = &done.rewritten.last().unwrap().1;
    assert_eq!(&resolve_snapshot_id(&repo, "release").unwrap(), new);
    healthy(&repo);
}

#[test]
fn the_squashed_snapshot_keeps_the_newest_members_metadata_and_author() {
    let (_t, repo) = setup();
    let ada = Author::new("ada").unwrap();
    let bob = Author::new("bob").unwrap();
    let meta = |v: &str| {
        let mut m = SnapshotMeta::new();
        m.set("ci", "run", v).unwrap();
        m
    };
    let entries = |b: &str| vec![TreeEntry::file("f.txt", b.as_bytes().to_vec())];
    let a = put(
        &repo,
        "main",
        None,
        None,
        entries("a"),
        meta("1"),
        Some(&ada),
        &[],
        1_000,
    );
    let b = put(
        &repo,
        "main",
        Some(&a),
        None,
        entries("b"),
        meta("2"),
        Some(&ada),
        &[],
        2_000,
    );
    let c = put(
        &repo,
        "main",
        Some(&b),
        None,
        entries("c"),
        meta("3"),
        Some(&bob),
        &[],
        3_000,
    );
    let done = compact_one(&repo, &a, &c).unwrap();
    let stored = repo.snapshot_meta(&done.squashed[0].into).unwrap();
    assert_eq!(stored.get("ci", "run"), Some("3"));
    assert_eq!(stored.get("velo", "author.name"), Some("bob"));
    healthy(&repo);
}

#[test]
fn rename_edges_compose_across_the_range() {
    let (_t, repo) = setup();
    let file = |p: &str| TreeEntry::file(p, b"same\n".to_vec());
    let s0 = put(
        &repo,
        "main",
        None,
        None,
        vec![file("a")],
        SnapshotMeta::new(),
        None,
        &[],
        1_000,
    );
    let s1 = put(
        &repo,
        "main",
        Some(&s0),
        None,
        vec![file("b")],
        SnapshotMeta::new(),
        None,
        &[(PathBuf::from("a"), PathBuf::from("b"))],
        2_000,
    );
    let s2 = put(
        &repo,
        "main",
        Some(&s1),
        None,
        vec![file("c")],
        SnapshotMeta::new(),
        None,
        &[(PathBuf::from("b"), PathBuf::from("c"))],
        3_000,
    );
    let _s3 = put(
        &repo,
        "main",
        Some(&s2),
        None,
        vec![file("c")],
        SnapshotMeta::new(),
        None,
        &[],
        4_000,
    );
    let done = compact_one(&repo, &s1, &s2).unwrap();
    let into = done.squashed[0].into.clone();
    let edges: Vec<(String, String)> = repo
        .conn()
        .prepare("SELECT from_path, to_path FROM renames WHERE snapshot_hash = ?")
        .unwrap()
        .query_map([&into], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(edges, vec![("a".to_string(), "c".to_string())]);
    healthy(&repo);
}

#[test]
fn dry_run_reports_the_same_result_and_writes_nothing() {
    let (_t, repo) = setup();
    let ids = chain(&repo, 6);
    let before = (all_hashes(&repo), repo.head_token().unwrap());

    let guard = repo.write().unwrap();
    let ranges = [Range {
        oldest: &ids[1],
        newest: &ids[3],
    }];
    let dry = compact::run(
        &guard,
        &main_branch(),
        &ranges,
        compact::Options {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(dry.dry_run);
    assert_eq!(before, (all_hashes(&repo), repo.head_token().unwrap()));
    assert_eq!(scalar(&repo, "SELECT COUNT(*) FROM compactions"), 0);

    let real = compact::run(&guard, &main_branch(), &ranges, Default::default()).unwrap();
    assert!(!real.dry_run);
    assert_eq!(dry.squashed, real.squashed);
    assert_eq!(dry.rewritten, real.rewritten);
}

#[test]
fn the_branch_move_is_announced_as_ref_moved() {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let (tx, rx) = mpsc::channel();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap().listening(tx);
    let ids = chain(&repo, 5);
    let name: TagName = "v".parse().unwrap();
    tag::create(&repo.write().unwrap(), &name, Some(&ids[4]), false).unwrap();
    rx.try_iter().for_each(drop);

    let done = compact_one(&repo, &ids[1], &ids[2]).unwrap();
    let new_tip = done.rewritten.last().unwrap().1.clone();
    let events: Vec<Event> = rx.try_iter().collect();
    assert_eq!(
        events,
        vec![
            Event::RefMoved {
                reference: Ref::Branch(main_branch()),
                from: Some(ids[4].clone()),
                to: Some(new_tip.clone()),
            },
            Event::RefMoved {
                reference: Ref::Tag(name),
                from: Some(ids[4].clone()),
                to: Some(new_tip),
            },
        ]
    );
}

#[test]
fn parent_is_followed_when_it_names_a_reminted_snapshot() {
    let (tmp, repo) = setup();
    let ids = chain(&repo, 4);
    std::fs::write(tmp.path().join(".velo/PARENT"), ids[3].as_str()).unwrap();
    let done = compact_one(&repo, &ids[0], &ids[1]).unwrap();
    let on_disk = std::fs::read_to_string(tmp.path().join(".velo/PARENT")).unwrap();
    assert_eq!(on_disk, done.rewritten[1].1.as_str());

    // Inside a range, the checked-out snapshot may only be the newest.
    let (tmp, repo) = setup();
    let ids = chain(&repo, 4);
    std::fs::write(tmp.path().join(".velo/PARENT"), ids[1].as_str()).unwrap();
    refused(&repo, &ids[0], &ids[2], "checked-out");
}
