//! `commands::repack`: keyframes become deltas without changing a tree or an id.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tempfile::TempDir;

use crate::commands::fsck;
use crate::commands::repack::{self, Options};
use crate::progress::{Cancel, Observer, Phase};
use crate::tree::{SaveTree, TreeEntry};
use crate::tree_store::{self, StoredTree};
use crate::{BranchName, Error, Repo, SnapshotId, SnapshotMeta};

/// A repository whose snapshots are written at keyframe interval `k`.
fn setup_k(k: u32) -> (TempDir, Repo) {
    crate::repo::set_test_keyframe_interval(k);
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn put(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    merge_parent: Option<&SnapshotId>,
    body: &str,
    ts: i64,
) -> SnapshotId {
    let branch: BranchName = branch.parse().unwrap();
    // Mostly stable files, so a delta is much smaller than a keyframe.
    let mut entries: Vec<TreeEntry> = (0..8)
        .map(|i| TreeEntry::file(format!("stable{i}.txt"), format!("same {i}\n").into_bytes()))
        .collect();
    entries.push(TreeEntry::file("f.txt", body.as_bytes().to_vec()));
    entries.push(TreeEntry::file(format!("only{ts}.txt"), b"x\n".to_vec()));
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent,
            message: &format!("step {ts}"),
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

fn chain(repo: &Repo, n: i64) -> Vec<SnapshotId> {
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..n {
        let id = put(
            repo,
            "main",
            ids.last(),
            None,
            &format!("body {i}\n"),
            1_000 * (i + 1),
        );
        ids.push(id);
    }
    ids
}

fn hashes(repo: &Repo) -> Vec<String> {
    let mut stmt = repo
        .conn()
        .prepare("SELECT hash FROM snapshots ORDER BY hash")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

fn trees(repo: &Repo) -> BTreeMap<String, StoredTree> {
    hashes(repo)
        .into_iter()
        .map(|h| {
            let t = tree_store::tree_of(repo.conn(), &h).unwrap();
            (h, t)
        })
        .collect()
}

fn max_depth(repo: &Repo) -> usize {
    let links: HashMap<String, String> = tree_store::delta_links(repo.conn())
        .unwrap()
        .into_iter()
        .collect();
    links
        .keys()
        .map(|start| {
            let mut d = 0;
            let mut cur = start;
            while let Some(b) = links.get(cur) {
                d += 1;
                cur = b;
            }
            d
        })
        .max()
        .unwrap_or(0)
}

fn scalar(repo: &Repo, sql: &str) -> i64 {
    repo.conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn healthy(repo: &Repo) {
    let report = fsck::check(repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

fn dump(repo: &Repo) -> Vec<String> {
    let mut out = Vec::new();
    for sql in [
        "SELECT snapshot_hash || path || hash FROM file_map ORDER BY 1",
        "SELECT snapshot_hash || base_hash FROM tree_deltas ORDER BY 1",
        "SELECT snapshot_hash || path || COALESCE(hash, '-') FROM tree_delta_rows ORDER BY 1",
    ] {
        let mut stmt = repo.conn().prepare(sql).unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        out.extend(rows.map(|r| r.unwrap()));
    }
    out
}

#[test]
fn a_k1_history_repacked_at_k4_keeps_ids_and_trees() {
    let (_tmp, mut repo) = setup_k(1);
    chain(&repo, 10);
    assert_eq!(scalar(&repo, "SELECT COUNT(*) FROM tree_deltas"), 0);
    let ids = hashes(&repo);
    let before = trees(&repo);

    repo.set_keyframe_interval(4);
    let out = repack::run(
        &repo,
        Options {
            chunk: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.snapshots_converted, 7);
    assert_eq!(out.keyframes_kept, 3);
    assert_eq!(out.chains_shortened, 0);
    assert!(out.rows_after < out.rows_before, "{out:?}");
    assert!(!out.dry_run);

    assert_eq!(hashes(&repo), ids);
    assert_eq!(trees(&repo), before);
    assert_eq!(max_depth(&repo), 3);
    healthy(&repo);

    let again = repack::run(&repo, Options::default()).unwrap();
    assert_eq!(again.snapshots_converted, 0);
    assert_eq!(again.chains_shortened, 0);
    assert_eq!(again.rows_after, out.rows_after);
}

#[test]
fn a_dry_run_writes_nothing_and_predicts_a_real_run() {
    let (_tmp, mut repo) = setup_k(1);
    chain(&repo, 9);
    repo.set_keyframe_interval(4);
    let rows = dump(&repo);

    let dry = repack::run(
        &repo,
        Options {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(dry.dry_run);
    assert_eq!(dump(&repo), rows, "a dry run changed the database");
    assert!(dry.snapshots_converted > 0 && dry.rows_after < dry.rows_before);
    assert!(dry.bytes_after < dry.bytes_before, "{dry:?}");

    let real = repack::run(&repo, Options::default()).unwrap();
    assert_eq!(real.snapshots_converted, dry.snapshots_converted);
    assert_eq!(real.keyframes_kept, dry.keyframes_kept);
    assert_eq!(real.chains_shortened, dry.chains_shortened);
    assert_eq!(real.rows_before, dry.rows_before);
    assert_eq!(real.rows_after, dry.rows_after);
}

/// Cancels after the first chunk has been reported.
struct CancelAfterChunk(Cancel);

impl Observer for CancelAfterChunk {
    fn advance(&self, phase: Phase, _by: u64) {
        if phase == Phase::Repacking {
            self.0.cancel();
        }
    }
}

#[test]
fn cancelling_between_chunks_leaves_a_valid_repository_a_later_run_finishes() {
    let (_tmp, mut repo) = setup_k(1);
    chain(&repo, 10);
    repo.set_keyframe_interval(4);
    let before = trees(&repo);

    let cancel = Cancel::new();
    let observer = CancelAfterChunk(cancel.clone());
    let err = repack::run(
        &repo,
        Options {
            chunk: 2,
            observer: Some(&observer),
            cancel: Some(&cancel),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    let done = scalar(&repo, "SELECT COUNT(*) FROM tree_deltas");
    assert!(done > 0 && done < 7, "partly repacked, got {done}");
    assert_eq!(trees(&repo), before);
    healthy(&repo);

    let rest = repack::run(&repo, Options::default()).unwrap();
    assert_eq!(rest.snapshots_converted as i64 + done, 7);
    assert_eq!(trees(&repo), before);
    assert_eq!(max_depth(&repo), 3);
    healthy(&repo);
}

/// Saves through a second handle each time a chunk is reported.
struct SaveBetween {
    root: PathBuf,
    parent: SnapshotId,
    saved: AtomicUsize,
}

impl Observer for SaveBetween {
    fn advance(&self, phase: Phase, _by: u64) {
        if phase != Phase::Repacking {
            return;
        }
        let n = self.saved.fetch_add(1, Ordering::SeqCst) as i64;
        let other = Repo::open(&self.root).unwrap();
        put(
            &other,
            "side",
            Some(&self.parent),
            None,
            "side\n",
            90_000 + n,
        );
    }
}

#[test]
fn the_write_lock_is_free_between_chunks() {
    let (tmp, mut repo) = setup_k(1);
    let ids = chain(&repo, 8);
    repo.set_keyframe_interval(4);
    let observer = Arc::new(SaveBetween {
        root: tmp.path().to_path_buf(),
        parent: ids[0].clone(),
        saved: AtomicUsize::new(0),
    });
    repack::run(
        &repo,
        Options {
            chunk: 2,
            observer: Some(&*observer),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(observer.saved.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        scalar(&repo, "SELECT COUNT(*) FROM snapshots"),
        8 + observer.saved.load(Ordering::SeqCst) as i64
    );
    healthy(&repo);
}

#[test]
fn an_overlong_chain_is_cut_to_the_interval() {
    let (_tmp, mut repo) = setup_k(8);
    chain(&repo, 8);
    assert_eq!(max_depth(&repo), 7);
    let before = trees(&repo);

    repo.set_keyframe_interval(3);
    let out = repack::run(&repo, Options::default()).unwrap();
    assert!(out.chains_shortened > 0, "{out:?}");
    assert!(max_depth(&repo) <= 2);
    assert_eq!(trees(&repo), before);
    healthy(&repo);
    let again = repack::run(&repo, Options::default()).unwrap();
    assert_eq!((again.snapshots_converted, again.chains_shortened), (0, 0));
}

#[test]
fn a_merge_snapshot_is_based_on_its_first_parent_only() {
    let (_tmp, mut repo) = setup_k(1);
    let a = put(&repo, "main", None, None, "a\n", 1_000);
    let b = put(&repo, "main", Some(&a), None, "b\n", 2_000);
    let c = put(&repo, "side", Some(&a), None, "c\n", 3_000);
    let m = put(&repo, "main", Some(&b), Some(&c), "m\n", 4_000);
    let before = trees(&repo);

    repo.set_keyframe_interval(4);
    repack::run(&repo, Options::default()).unwrap();
    let base = tree_store::base_of(repo.conn(), m.as_str()).unwrap();
    assert_eq!(base.as_deref(), Some(b.as_str()));
    assert_eq!(trees(&repo), before);
    healthy(&repo);
}

#[test]
fn a_root_and_a_k1_repository_are_left_alone() {
    let (_tmp, mut repo) = setup_k(1);
    chain(&repo, 4);
    let rows = dump(&repo);
    // K = 1 stores whole trees only.
    let out = repack::run(&repo, Options::default()).unwrap();
    assert_eq!((out.snapshots_converted, out.chains_shortened), (0, 0));
    assert_eq!(dump(&repo), rows);

    // A lone root stays a keyframe at any K.
    let (_tmp2, mut lone) = setup_k(1);
    chain(&lone, 1);
    lone.set_keyframe_interval(4);
    let out = repack::run(&lone, Options::default()).unwrap();
    assert_eq!(out.snapshots_converted, 0);
    assert_eq!(out.keyframes_kept, 1);
    repo.set_keyframe_interval(1);
}
