//! Retention at agent-checkpointing scale: what the default policy does to a
//! long, dense history. Phase 18.10-c.
//!
//! The measurement is `#[ignore]`d (it saves 2,000 snapshots of a 10,000-file
//! tree) and runs on demand, in release mode:
//!
//! ```text
//! cargo test -p velo-core --release --features timing retention_scale -- --ignored --nocapture
//! ```
//!
//! A small non-ignored test drives the same code so the harness cannot rot.

use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::commands::gc::Vacuum;
use crate::commands::{gc, retention};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId};

const DAY_MS: i64 = 86_400_000;
const BASE_MS: i64 = 1_700_000_000_000;

fn setup() -> (TempDir, Repo) {
    crate::repo::keyframes_only();
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// What the repository holds at one moment.
struct Shape {
    snapshots: i64,
    rows: i64,
    /// `page_count * page_size`.
    db_bytes: i64,
    free_bytes: i64,
    /// `velo.db` on disk, after a WAL checkpoint so the log does not hide it.
    file_bytes: u64,
}

fn shape(repo: &Repo, root: &Path) -> Shape {
    let conn = repo.conn();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    Shape {
        snapshots: one("SELECT COUNT(*) FROM snapshots"),
        rows: one("SELECT COUNT(*) FROM file_map"),
        db_bytes: one("SELECT (SELECT page_count FROM pragma_page_count)
                            * (SELECT page_size FROM pragma_page_size)"),
        free_bytes: one("SELECT (SELECT freelist_count FROM pragma_freelist_count)
                              * (SELECT page_size FROM pragma_page_size)"),
        file_bytes: std::fs::metadata(root.join(".velo/velo.db")).unwrap().len(),
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

/// Run the measurement. Returns the shape before retention and the shape after
/// retention, gc and an incremental vacuum.
#[allow(clippy::print_stderr)]
fn run(files: usize, snapshots: usize, days: i64) -> (Shape, Shape) {
    let (tmp, repo) = setup();
    let built = build(&repo, files, snapshots, days);
    let before = shape(&repo, tmp.path());
    let branch: BranchName = "main".parse().unwrap();
    let now_ms = BASE_MS + days * DAY_MS;

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
    let laps = crate::timing::drain_laps();
    let after_retention = shape(&repo, tmp.path());

    let started = Instant::now();
    gc::run(&repo.write().unwrap(), gc::Options::default()).unwrap();
    let collected = started.elapsed();
    #[cfg(feature = "timing")]
    let gc_laps = crate::timing::drain_laps();
    let after_gc = shape(&repo, tmp.path());

    let started = Instant::now();
    let vacuumed = gc::vacuum(&repo.write().unwrap(), Vacuum::Incremental).unwrap();
    let vacuum_time = started.elapsed();
    let after = shape(&repo, tmp.path());

    eprintln!("--- retention at scale: {files} files x {snapshots} snapshots over {days} days ---");
    eprintln!("build: {built:?}");
    eprintln!(
        "retention::apply (default policy): {retained:?}, examined {}, removed {}",
        report.examined, report.removed
    );
    eprintln!("gc: {collected:?}");
    eprintln!(
        "gc::vacuum(Incremental): {vacuum_time:?}, auto_vacuum {:?}, {} -> {} bytes",
        vacuumed.auto_vacuum, vacuumed.bytes_before, vacuumed.bytes_after
    );
    for (label, s) in [
        ("before", &before),
        ("after retention", &after_retention),
        ("after gc", &after_gc),
        ("after vacuum", &after),
    ] {
        eprintln!(
            "{label}: {} snapshots, {} file_map rows, velo.db {} bytes on disk,              page_count*page_size {} bytes ({} bytes on the freelist)",
            s.snapshots, s.rows, s.file_bytes, s.db_bytes, s.free_bytes
        );
    }
    #[cfg(feature = "timing")]
    {
        eprintln!("--- stage laps: retention::apply ---");
        for lap in laps
            .iter()
            .filter(|l| l.name.starts_with("retention.") || l.name.starts_with("compact."))
        {
            eprintln!("lap {} {:?}", lap.name, lap.elapsed);
        }
        eprintln!("--- stage laps: gc ---");
        for lap in &gc_laps {
            eprintln!("lap {} {:?}", lap.name, lap.elapsed);
        }
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
    assert!(after.file_bytes < before.file_bytes);
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
    // A new repository vacuums incrementally, so the file is no larger than it
    // was before retention ran (it once freed a single page).
    assert!(after.file_bytes < before.file_bytes);
}

/// The 18.10 shape at a fifth of the width and snapshot count, for comparing
/// the stage laps of one change against the next (19.4-a).
#[test]
#[ignore = "measurement: 400 snapshots of 2,000 files; run with --release --features timing"]
fn default_policy_at_a_reduced_18_10_shape() {
    let (before, after) = run(2_000, 400, 60);
    assert!(after.snapshots < before.snapshots);
}
