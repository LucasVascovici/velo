//! Compaction, retention, gc, squash, rebase and `save --amend` never drop or
//! re-mint a signed snapshot: FORMAT.md 11.3.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::compact::{self, Protected, Range};
use crate::commands::rebase::{self, Mode};
use crate::commands::retention::{self, Policy};
use crate::commands::{gc, save, squash, switch, undo};
use crate::sign::{self, SigningKey};
use crate::tree::{SaveTree, TreeEntry};
use crate::{Error, Repo, SnapshotId, SnapshotMeta};

const DAY: i64 = 24 * 3_600_000;

fn setup() -> (TempDir, PathBuf) {
    crate::repo::keyframes_only();
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    crate::commands::init::run(&path).unwrap();
    (tmp, path)
}

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    f(&guard)
}

fn save_as(root: &Path, file: &str, msg: &str) -> SnapshotId {
    fs::write(root.join(file), msg).unwrap();
    with_write(root, |g| save::run(g, Some(msg), save::Options::default()))
        .unwrap()
        .into_result()
        .expect("a snapshot")
        .hash
}

fn sign_it(root: &Path, id: &SnapshotId) {
    let key = SigningKey::from_seed([9; 32]);
    with_write(root, |g| sign::sign(g, id, &key)).unwrap();
}

/// Everything a refused rewrite must leave alone: snapshot ids, signature rows
/// and trash.
fn state(root: &Path) -> (Vec<String>, i64, i64) {
    let repo = Repo::open_and_migrate(root).unwrap();
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT hash FROM snapshots ORDER BY hash")
        .unwrap();
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let sigs = conn
        .query_row("SELECT COUNT(*) FROM signatures", [], |r| r.get(0))
        .unwrap();
    let trash = conn
        .query_row("SELECT COUNT(*) FROM trash", [], |r| r.get(0))
        .unwrap();
    (ids, sigs, trash)
}

fn main_branch() -> crate::BranchName {
    "main".parse().unwrap()
}

fn compact_range(
    root: &Path,
    a: &SnapshotId,
    b: &SnapshotId,
) -> crate::Result<compact::Compaction> {
    with_write(root, |g| {
        compact::run(
            g,
            &main_branch(),
            &[Range {
                oldest: a,
                newest: b,
            }],
            Default::default(),
        )
    })
}

fn refused<T: std::fmt::Debug>(r: crate::Result<T>) -> String {
    match r {
        Err(e @ Error::InvalidInput { .. }) => e.to_string(),
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

fn four(root: &Path) -> Vec<SnapshotId> {
    (1..=4)
        .map(|i| save_as(root, "f.txt", &format!("v{i}")))
        .collect()
}

#[test]
fn compact_refuses_a_signed_snapshot_in_the_range() {
    let (_t, root) = setup();
    let ids = four(&root);
    sign_it(&root, &ids[1]);
    let before = state(&root);
    let msg = refused(compact_range(&root, &ids[0], &ids[2]));
    assert!(msg.contains("signed"), "{msg}");
    assert_eq!(state(&root), before);
}

#[test]
fn compact_refuses_to_re_mint_a_signed_descendant() {
    let (_t, root) = setup();
    let ids = four(&root);
    sign_it(&root, &ids[3]);
    let before = state(&root);
    let msg = refused(compact_range(&root, &ids[0], &ids[1]));
    assert!(msg.contains("re-minted"), "{msg}");
    assert_eq!(state(&root), before);
}

#[test]
fn compact_allows_a_signed_snapshot_older_than_the_range() {
    let (_t, root) = setup();
    let ids = four(&root);
    sign_it(&root, &ids[0]);
    compact_range(&root, &ids[1], &ids[2]).unwrap();
    assert!(state(&root).0.contains(&ids[0].to_string()));
}

#[test]
fn retention_leaves_signed_snapshots_and_everything_older() {
    let (_t, root) = setup();
    let now = 100 * DAY;
    let ids: Vec<SnapshotId> = {
        let repo = Repo::open_and_migrate(&root).unwrap();
        let mut ids: Vec<SnapshotId> = Vec::new();
        for i in 0..10 {
            let id = repo
                .write()
                .unwrap()
                .save_tree(SaveTree {
                    branch: &main_branch(),
                    parent: ids.last(),
                    merge_parent: None,
                    message: &format!("step {i}"),
                    entries: vec![TreeEntry::file("f.txt", format!("body {i}").into_bytes())],
                    meta: SnapshotMeta::new(),
                    author: None,
                    timestamp_ms: Some(now - 50 * DAY + i * 1000),
                    renames: &[],
                })
                .unwrap();
            ids.push(id);
        }
        ids
    };
    sign_it(&root, &ids[4]);
    let (_, sigs, _) = state(&root);

    let report = with_write(&root, |g| {
        retention::apply(
            g,
            &main_branch(),
            &Policy::default(),
            retention::Options {
                now_ms: Some(now),
                ..Default::default()
            },
        )
    })
    .unwrap();

    assert!(report.removed > 0, "the newer snapshots should be squashed");
    let (after, after_sigs, _) = state(&root);
    for id in &ids[..=4] {
        assert!(after.contains(&id.to_string()), "{id} must survive");
    }
    assert_eq!(after_sigs, sigs);
    assert!(report
        .protected
        .iter()
        .any(|(id, why)| id == &ids[4] && *why == Protected::Signed));
}

fn gc_now(root: &Path) -> gc::Collected {
    with_write(root, |g| {
        gc::run(
            g,
            gc::Options {
                keep_days: 0,
                ..Default::default()
            },
        )
    })
    .unwrap()
}

#[test]
fn gc_keeps_an_expired_signed_trash_entry_and_signatures() {
    let (_t, root) = setup();
    save_as(&root, "f.txt", "one");
    let two = save_as(&root, "f.txt", "two");
    sign_it(&root, &two);
    with_write(&root, undo::run).unwrap();
    let before = state(&root);
    assert_eq!(before.2, 1, "the undone snapshot is in the trash");

    assert_eq!(gc_now(&root).expired_trash, 0);
    assert_eq!(state(&root), before);
}

#[test]
fn gc_still_expires_an_unsigned_trash_entry() {
    let (_t, root) = setup();
    save_as(&root, "f.txt", "one");
    save_as(&root, "f.txt", "two");
    with_write(&root, undo::run).unwrap();
    assert_eq!(gc_now(&root).expired_trash, 1);
}

#[test]
fn squash_refuses_a_signed_snapshot_in_the_range() {
    let (_t, root) = setup();
    let ids: Vec<_> = (1..=3)
        .map(|i| save_as(&root, "f.txt", &format!("v{i}")))
        .collect();
    sign_it(&root, &ids[1]);
    let before = state(&root);
    let msg = refused(with_write(&root, |g| squash::run(g, 2, "x")));
    assert!(msg.contains("signed"), "{msg}");
    assert_eq!(state(&root), before);
}

#[test]
fn squash_allows_a_signed_snapshot_below_the_range() {
    let (_t, root) = setup();
    let ids: Vec<_> = (1..=3)
        .map(|i| save_as(&root, "f.txt", &format!("v{i}")))
        .collect();
    sign_it(&root, &ids[0]);
    with_write(&root, |g| squash::run(g, 2, "x")).unwrap();
    assert!(state(&root).0.contains(&ids[0].to_string()));
}

/// main: base, m2. topic: base, t1, t2 (t1 and t2 would be replayed onto m2).
fn diverged() -> (TempDir, PathBuf, SnapshotId, SnapshotId, SnapshotId) {
    let (t, root) = setup();
    save_as(&root, "base.txt", "base");
    with_write(&root, |g| switch::run(g, "topic", true)).unwrap();
    let t1 = save_as(&root, "t1.txt", "t1");
    let t2 = save_as(&root, "t2.txt", "t2");
    with_write(&root, |g| switch::run(g, "main", false)).unwrap();
    let m2 = save_as(&root, "m2.txt", "m2");
    with_write(&root, |g| switch::run(g, "topic", false)).unwrap();
    (t, root, t1, t2, m2)
}

fn rebase_onto(root: &Path, onto: &SnapshotId) -> crate::Result<rebase::Outcome> {
    with_write(root, |g| rebase::run(g, Mode::Start { onto }, None))
}

#[test]
fn rebase_refuses_to_replay_a_signed_snapshot() {
    for pick in [0, 1] {
        let (_t, root, t1, t2, m2) = diverged();
        sign_it(&root, if pick == 0 { &t1 } else { &t2 });
        let before = state(&root);
        let msg = refused(rebase_onto(&root, &m2));
        assert!(msg.contains("signed"), "{msg}");
        assert_eq!(state(&root), before);
        assert!(!root.join(".velo/REBASE_STATE").exists());
    }
}

#[test]
fn rebase_still_works_without_signatures() {
    let (_t, root, _t1, _t2, m2) = diverged();
    rebase_onto(&root, &m2).unwrap();
}

fn amend(root: &Path) -> crate::Result<save::Outcome> {
    fs::write(root.join("f.txt"), "three").unwrap();
    with_write(root, |g| {
        save::run(
            g,
            Some("again"),
            save::Options {
                amend: true,
                ..Default::default()
            },
        )
    })
}

#[test]
fn amend_refuses_a_signed_tip() {
    let (_t, root) = setup();
    save_as(&root, "f.txt", "one");
    let two = save_as(&root, "f.txt", "two");
    sign_it(&root, &two);
    let before = state(&root);
    let msg = refused(amend(&root));
    assert!(msg.contains("signed"), "{msg}");
    assert_eq!(state(&root), before);
}

#[test]
fn amend_allows_an_unsigned_tip_above_a_signed_snapshot() {
    let (_t, root) = setup();
    let one = save_as(&root, "f.txt", "one");
    save_as(&root, "f.txt", "two");
    sign_it(&root, &one);
    amend(&root).unwrap();
    assert!(state(&root).0.contains(&one.to_string()));
}
