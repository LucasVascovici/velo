//! Retention at agent-checkpointing scale: what the default policy does to a
//! long, dense history. Phase 18.10-c.
//!
//! The measurement is `#[ignore]`d (it saves 2,000 snapshots of a 10,000-file
//! tree) and runs on demand, in release mode:
//!
//! ```text
//! cargo test -p velo-core --release retention_scale -- --ignored --nocapture
//! ```
//!
//! A small non-ignored test drives the same code so the harness cannot rot.

use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::commands::{gc, retention};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId};

const DAY_MS: i64 = 86_400_000;
const BASE_MS: i64 = 1_700_000_000_000;

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// What the repository holds at one moment.
struct Shape {
    snapshots: i64,
    rows: i64,
    db_bytes: i64,
    free_bytes: i64,
}

fn shape(repo: &Repo) -> Shape {
    let conn = repo.conn();
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    Shape {
        snapshots: one("SELECT COUNT(*) FROM snapshots"),
        rows: one("SELECT COUNT(*) FROM file_map"),
        db_bytes: one("SELECT (SELECT page_count FROM pragma_page_count)
                            * (SELECT page_size FROM pragma_page_size)"),
        free_bytes: one("SELECT (SELECT freelist_count FROM pragma_freelist_count)
                              * (SELECT page_size FROM pragma_page_size)"),
    }
}

fn body(i: usize, version: usize) -> Vec<u8> {
    format!("file {i} version {version}\n").into_bytes()
}

/// Save `snapshots` snapshots of a `files`-file tree, spread evenly over `days`
/// synthetic days, changing three files per step and carrying the rest forward
/// by object hash.
fn build(repo: &Repo, files: usize, snapshots: usize, days: i64) -> Duration {
    let branch: BranchName = "main".parse().unwrap();
    let started = Instant::now();
    let mut parent: Option<SnapshotId> = None;
    for n in 0..snapshots {
        let mut entries: Vec<TreeEntry> = match &parent {
            None => (0..files)
                .map(|i| TreeEntry::file(format!("dir{}/file{i}.txt", i % 50), body(i, 0)))
                .collect(),
            Some(p) => repo
                .tree_at(p)
                .unwrap()
                .into_iter()
                .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
                .collect(),
        };
        if parent.is_some() {
            for k in 0..3 {
                let i = (n * 3 + k) % files;
                let path = format!("dir{}/file{i}.txt", i % 50);
                let at = entries.iter().position(|e| e.path == path).unwrap();
                entries[at] = TreeEntry::file(path, body(i, n));
            }
        }
        let timestamp_ms = BASE_MS + (n as i64) * (days * DAY_MS) / snapshots as i64;
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: None,
                message: &format!("step {n}"),
                entries,
                meta: Default::default(),
                author: None,
                timestamp_ms: Some(timestamp_ms),
                renames: &[],
            })
            .unwrap();
        parent = Some(id);
    }
    started.elapsed()
}

#[allow(clippy::print_stderr)]
fn run(files: usize, snapshots: usize, days: i64) -> (Shape, Shape) {
    let (_tmp, repo) = setup();
    let built = build(&repo, files, snapshots, days);
    let before = shape(&repo);
    let branch: BranchName = "main".parse().unwrap();
    let now_ms = BASE_MS + days * DAY_MS;

    #[cfg(feature = "timing")]
    let pages_before: i64 = repo
        .conn()
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    #[cfg(feature = "timing")]
    crate::timing::drain_laps();
    let started = Instant::now();
    let report = retention::apply(
        &repo.write().unwrap(),
        &branch,
        &retention::Policy::default(),
        retention::Options {
            now_ms: Some(now_ms),
            ..Default::default()
        },
    )
    .unwrap();
    let retained = started.elapsed();
    #[cfg(feature = "timing")]
    {
        let pages_after: i64 = repo
            .conn()
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap();
        eprintln!("page_count before {pages_before}, after {pages_after}");
        for lap in crate::timing::drain_laps() {
            eprintln!("lap {} {:?}", lap.name, lap.elapsed);
        }
    }

    let started = Instant::now();
    gc::run(&repo.write().unwrap(), gc::Options::default()).unwrap();
    let collected = started.elapsed();
    let after = shape(&repo);

    eprintln!("--- retention at scale: {files} files x {snapshots} snapshots over {days} days ---");
    eprintln!("build: {built:?}");
    eprintln!(
        "retention::apply (default policy): {retained:?}, examined {}, removed {}",
        report.examined, report.removed
    );
    eprintln!("gc: {collected:?}");
    for (label, s) in [("before", &before), ("after", &after)] {
        eprintln!(
            "{label}: {} snapshots, {} file_map rows, database {} bytes ({} free pages' bytes)",
            s.snapshots, s.rows, s.db_bytes, s.free_bytes
        );
    }
    (before, after)
}

#[test]
#[ignore = "measurement: 2,000 snapshots of 10,000 files; run with --release"]
fn default_policy_at_ten_thousand_files_and_two_thousand_snapshots() {
    let (before, after) = run(10_000, 2_000, 60);
    assert_eq!(before.snapshots, 2_000);
    assert!(after.snapshots < before.snapshots);
    assert!(after.rows < before.rows);
}

/// The same code at a size that runs in a blink, so it stays compiled and true.
#[test]
fn default_policy_bounds_rows_at_small_scale() {
    // 100 snapshots over 60 days: about 1.7 per day, so the daily tier keeps
    // roughly one per day outside the last day and everything inside it.
    let (before, after) = run(20, 100, 60);
    assert_eq!(before.snapshots, 100);
    assert!(after.snapshots < before.snapshots);
    assert_eq!(after.rows, after.snapshots * 20);
}

/// The 18.10 shape at a fifth of the width and snapshot count, for comparing
/// the stage laps of one change against the next (19.4-a).
#[test]
#[ignore = "measurement: 400 snapshots of 2,000 files; run with --release --features timing"]
fn default_policy_at_a_reduced_18_10_shape() {
    let (before, after) = run(2_000, 400, 60);
    assert!(after.snapshots < before.snapshots);
}
