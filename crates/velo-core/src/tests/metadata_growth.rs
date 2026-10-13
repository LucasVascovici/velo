//! Measurements for Phase 15.9: how `file_map` grows, and who needs
//! `idx_filemap_path`. Nothing here changes storage; the timing tests are
//! `#[ignore]`d and run on demand:
//!
//! ```text
//! cargo test -p velo-core metadata_growth -- --ignored --nocapture
//! ```
//!
//! A small non-ignored test drives the same code so the harness cannot rot.

use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// What one run measured.
struct Growth {
    first: Duration,
    last: Duration,
    window: usize,
    rows: i64,
    db_bytes: i64,
    insert_loop: Duration,
    insert_select: Duration,
}

fn entries(files: usize, generation: usize, changed: usize) -> Vec<TreeEntry> {
    (0..files)
        .map(|i| {
            let body = if i == changed { generation } else { 0 };
            TreeEntry::file(
                format!("dir{}/file{i}.txt", i % 20),
                format!("file {i} body {body}\n").into_bytes(),
            )
        })
        .collect()
}

/// Build `files` files and `snapshots` snapshots, each changing one file, and
/// time the saves, then the two ways of writing a whole tree's rows.
fn measure(files: usize, snapshots: usize, window: usize) -> Growth {
    let (_tmp, repo) = setup();
    let branch: BranchName = "main".parse().unwrap();
    let mut parent: Option<SnapshotId> = None;
    let mut times = Vec::with_capacity(snapshots);
    for n in 0..snapshots {
        let started = Instant::now();
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: None,
                message: &format!("step {n}"),
                entries: entries(files, n + 1, n % files),
                meta: Default::default(),
                author: None,
                timestamp_ms: Some(1_000 + n as i64),
                renames: &[],
            })
            .unwrap();
        times.push(started.elapsed());
        parent = Some(id);
    }
    let tip = parent.expect("at least one snapshot");
    let sum = |s: &[Duration]| s.iter().sum::<Duration>();
    let window = window.min(snapshots);

    let conn = repo.conn();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_map", [], |r| r.get(0))
        .unwrap();
    let db_bytes: i64 = conn
        .query_row(
            "SELECT (SELECT page_count FROM pragma_page_count)
                  * (SELECT page_size FROM pragma_page_size)",
            [],
            |r| r.get(0),
        )
        .unwrap();

    // Both candidates write one snapshot's worth of rows under a scratch id and
    // are rolled back, so the repository is left as it was.
    let existing: Vec<(String, String, i64)> = conn
        .prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")
        .unwrap()
        .query_map([&tip], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    conn.execute_batch("BEGIN").unwrap();
    let started = Instant::now();
    {
        let mut stmt = conn
            .prepare("INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)")
            .unwrap();
        for (path, hash, mode) in &existing {
            stmt.execute(rusqlite::params!["scratch-loop", path, hash, mode])
                .unwrap();
        }
    }
    let insert_loop = started.elapsed();
    conn.execute_batch("ROLLBACK").unwrap();

    conn.execute_batch("BEGIN").unwrap();
    let started = Instant::now();
    conn.execute(
        "INSERT INTO file_map (snapshot_hash, path, hash, mode)
         SELECT 'scratch-select', path, hash, mode FROM file_map
          WHERE snapshot_hash = ?1 AND path <> ?2",
        rusqlite::params![&tip, &existing[0].0],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        rusqlite::params![
            "scratch-select",
            &existing[0].0,
            &existing[0].1,
            existing[0].2
        ],
    )
    .unwrap();
    let insert_select = started.elapsed();
    conn.execute_batch("ROLLBACK").unwrap();

    Growth {
        first: sum(&times[..window]),
        last: sum(&times[times.len() - window..]),
        window,
        rows,
        db_bytes,
        insert_loop,
        insert_select,
    }
}

#[allow(clippy::print_stderr)]
fn report(files: usize, snapshots: usize, g: &Growth) {
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / g.window as f64;
    eprintln!("--- file_map growth: {files} files x {snapshots} snapshots ---");
    eprintln!(
        "save latency, first {w}: {:.2} ms/save; last {w}: {:.2} ms/save",
        per(g.first),
        per(g.last),
        w = g.window
    );
    eprintln!(
        "{files}-row insert loop: {:?}; insert-select carry-forward + 1 insert: {:?}",
        g.insert_loop, g.insert_select
    );
    eprintln!(
        "file_map rows: {}; database size: {} bytes",
        g.rows, g.db_bytes
    );
}

/// Every distinct statement in `velo-core` that reads `file_map`, with where it
/// lives. Kept by hand: the list is the audit, and
/// `grep -rn file_map crates/velo-core/src` re-checks it.
const READERS: &[(&str, &str)] = &[
    (
        "apply/compact/fsck/merge/restore/save/commands::mod: load a tree",
        "SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?",
    ),
    (
        "tree.rs tree_at (public API): a tree in path order, with mode",
        "SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ? ORDER BY path",
    ),
    (
        "grep.rs: a tree in path order",
        "SELECT path, hash FROM file_map WHERE snapshot_hash = ? ORDER BY path",
    ),
    (
        "restore.rs: paths of a snapshot",
        "SELECT path FROM file_map WHERE snapshot_hash = ?",
    ),
    (
        "blame.rs / tree.rs read_file_at (public API): one file in one snapshot",
        "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
    ),
    (
        "diff.rs: stored hash of a path against PARENT (path first in the WHERE)",
        "SELECT hash FROM file_map WHERE path = ? AND snapshot_hash = ?",
    ),
    (
        "fsck.rs: does a snapshot hold a path",
        "SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ? AND path = ?)",
    ),
    (
        "fsck.rs / gc.rs: every referenced object",
        "SELECT DISTINCT hash FROM file_map",
    ),
    (
        "bundle.rs: export",
        "SELECT snapshot_hash, path, hash, mode FROM file_map",
    ),
    (
        "diff.rs tracks_path: prefix match",
        "SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ? AND (path = ? OR path LIKE ? || '/%'))",
    ),
    (
        "history.rs --file: root snapshot holds a path in the slice",
        "SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ?1 AND (path = ?2 OR path LIKE ?3 ESCAPE '\\'))",
    ),
    (
        "history.rs --file: slice differs from the parent's",
        "SELECT EXISTS(SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?1 AND (path = ?3 OR path LIKE ?4 ESCAPE '\\') EXCEPT SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?2 AND (path = ?3 OR path LIKE ?4 ESCAPE '\\'))",
    ),
    (
        "gc.rs: stale index_cache rows by path",
        "SELECT COUNT(*) FROM index_cache WHERE path NOT IN (SELECT path FROM file_map WHERE snapshot_hash IN (SELECT hash FROM snapshots))",
    ),
    (
        "gc.rs: orphaned file_map rows",
        "SELECT COUNT(*) FROM file_map WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots) AND snapshot_hash NOT IN (SELECT hash FROM trash)",
    ),
];

/// `EXPLAIN QUERY PLAN` for each reader: label, plan text, and whether the plan
/// names the path index.
fn plans(repo: &Repo) -> Vec<(&'static str, String, bool)> {
    let conn = repo.conn();
    READERS
        .iter()
        .map(|(label, sql)| {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            let n = stmt.parameter_count();
            let params = rusqlite::params_from_iter(std::iter::repeat_n("x", n));
            let detail: Vec<String> = stmt
                .query_map(params, |r| r.get::<_, String>(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let text = detail.join(" | ");
            let uses_path = text.contains("idx_filemap_path");
            (*label, text, uses_path)
        })
        .collect()
}

#[allow(clippy::print_stderr)]
fn report_plans(repo: &Repo) {
    eprintln!("--- query plans of file_map readers ---");
    for (label, text, uses_path) in plans(repo) {
        eprintln!(
            "[{}] {label}\n    {text}",
            if uses_path {
                "uses idx_filemap_path"
            } else {
                "no path index"
            }
        );
    }
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn file_map_growth_at_1000_files_200_snapshots() {
    let g = measure(1000, 200, 50);
    report(1000, 200, &g);
    assert_eq!(g.rows, 1000 * 200);
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn file_map_reader_query_plans() {
    let (_tmp, repo) = setup();
    report_plans(&repo);
}

#[test]
fn measurement_harness_runs_at_small_size() {
    let g = measure(20, 5, 2);
    assert_eq!(g.rows, 20 * 5);
    assert!(g.db_bytes > 0);
    let (_tmp, repo) = setup();
    let all = plans(&repo);
    assert_eq!(all.len(), READERS.len());
    // Lookups keyed on `snapshot_hash` must not need the path index.
    assert!(!all[0].2);
}
