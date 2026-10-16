//! 19.1-c: how much each cheap SQLite option moves a one-file `save_tree` at a
//! large tree size. Nothing here changes production settings: pragmas are set
//! on the open connection and put back, and the schema variants run on scratch
//! tables in a separate database file. Phase 19.1.
//!
//! ```text
//! cargo test -p velo-core --release --features timing perf_sqlite_options \
//!     -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Knobs, with defaults: `VELO_PROFILE_FILES` (100000), `VELO_PROFILE_HISTORY`
//! (200), `VELO_PROFILE_BULK` (0), `VELO_PROFILE_SAMPLES` (5),
//! `VELO_PROFILE_DIR` and `VELO_PROFILE_OUT`. One repository is built once and
//! is the scratch copy for every variant: copying 100,000 object files per
//! variant would cost more than the measurement. Each sample adds one snapshot,
//! so the history grows by SAMPLES per variant, which is negligible against the
//! built history (each JSON line says so).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{json, Value};
use tempfile::TempDir;

use super::metadata_growth::{bin_hash, db_bytes, hex_hash, row_path};
use super::perf_support::{build_history, project, Layout, Rng};
use crate::tree::{SaveTree, TreeEntry};
use crate::{timing, BranchName, Repo, SnapshotId, SnapshotMeta};

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[at]
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

#[allow(clippy::print_stderr)]
fn emit(line: &Value) {
    let text = line.to_string();
    eprintln!("{text}");
    if let Ok(out) = std::env::var("VELO_PROFILE_OUT") {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)
            .unwrap();
        writeln!(f, "{text}").unwrap();
    }
}

struct Params {
    files: usize,
    history: usize,
    samples: usize,
    bulk: bool,
    dir: Option<PathBuf>,
}

impl Params {
    fn from_env() -> Self {
        Params {
            files: env_usize("VELO_PROFILE_FILES", 100_000),
            history: env_usize("VELO_PROFILE_HISTORY", 200),
            samples: env_usize("VELO_PROFILE_SAMPLES", 5),
            bulk: env_usize("VELO_PROFILE_BULK", 0) != 0,
            dir: std::env::var("VELO_PROFILE_DIR").ok().map(PathBuf::from),
        }
    }
}

struct Scratch {
    _tmp: TempDir,
    repo: Repo,
    tip: SnapshotId,
    rng: Rng,
    branch: BranchName,
}

fn build(p: &Params) -> Scratch {
    let tmp = match &p.dir {
        Some(d) => TempDir::new_in(d).unwrap(),
        None => TempDir::new().unwrap(),
    };
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let mut rng = Rng::new(1);
    let branch: BranchName = "main".parse().unwrap();
    let entries = project(Layout::Scale, p.files, &mut rng)
        .into_iter()
        .map(|(path, bytes)| TreeEntry::file(path, bytes))
        .collect();
    let base = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "base",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap();
    let history = build_history(&repo, &branch, &base, p.history, p.bulk, &mut rng);
    let tip = history.last().unwrap_or(&base).clone();
    Scratch {
        _tmp: tmp,
        repo,
        tip,
        rng,
        branch,
    }
}

/// One sample: `save_tree` with every unchanged file stored and one rewritten.
/// Building the entries is not timed; the call is.
fn sample(s: &mut Scratch, n: usize) -> (f64, Vec<timing::Lap>) {
    let mut entries: Vec<TreeEntry> = s
        .repo
        .tree_at(&s.tip)
        .unwrap()
        .into_iter()
        .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
        .collect();
    let at = s.rng.below(entries.len());
    let path = entries[at].path.clone();
    entries[at] = TreeEntry::file(
        path,
        format!("option sample {n} {}\n", s.rng.next_u64()).into_bytes(),
    );
    timing::drain_laps();
    let t = Instant::now();
    let id = s
        .repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &s.branch,
            parent: Some(&s.tip),
            merge_parent: None,
            message: "option sample",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: None,
            renames: &[],
        })
        .unwrap();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    s.tip = id;
    (ms, timing::drain_laps())
}

/// Time `samples` samples: the totals, and the median per stage.
fn run_variant(s: &mut Scratch, samples: usize) -> (Vec<f64>, serde_json::Map<String, Value>) {
    let mut totals = Vec::new();
    let mut per_stage: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for n in 0..samples {
        let (ms, laps) = sample(s, n);
        totals.push(ms);
        let mut sum: BTreeMap<&str, f64> = BTreeMap::new();
        for l in laps {
            *sum.entry(l.name).or_default() += l.elapsed.as_secs_f64() * 1000.0;
        }
        for (k, v) in sum {
            per_stage.entry(k.to_owned()).or_default().push(v);
        }
    }
    let stages = per_stage
        .into_iter()
        .map(|(k, v)| (k, json!(quantile(&sorted(&v), 0.5))))
        .collect();
    (totals, stages)
}

fn set_pragma(s: &Scratch, name: &str, value: i64) {
    s.repo.conn().pragma_update(None, name, value).unwrap();
}

/// The `db.rs` defaults, put back after a variant.
fn restore_defaults(s: &Scratch) {
    s.repo
        .conn()
        .pragma_update(None, "synchronous", "NORMAL")
        .unwrap();
    set_pragma(s, "cache_size", -65_536);
    set_pragma(s, "mmap_size", 268_435_456);
    set_pragma(s, "wal_autocheckpoint", 1000);
}

fn checkpoint_ms(s: &Scratch) -> f64 {
    let t = Instant::now();
    s.repo
        .conn()
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| r.get::<_, i64>(0))
        .unwrap();
    t.elapsed().as_secs_f64() * 1000.0
}

/// Insert one tree's `rows` rows into a scratch table shaped as `shape` and
/// report insert time and bytes per row. Row-insert stage only: none of the
/// rest of `save_tree` is involved.
fn row_insert(shape: &str, rows: usize) -> Value {
    let tmp = TempDir::new().unwrap();
    let conn = rusqlite::Connection::open(tmp.path().join("scratch.db")).unwrap();
    let blob = shape == "blob_hashes";
    let ty = if blob { "BLOB" } else { "TEXT" };
    conn.execute_batch(&if shape == "without_rowid" {
        "CREATE TABLE fm (snapshot_hash TEXT NOT NULL, path TEXT NOT NULL, hash TEXT NOT NULL,
           mode INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (snapshot_hash, path)) WITHOUT ROWID;"
            .to_owned()
    } else {
        format!(
            "CREATE TABLE fm (snapshot_hash {ty} NOT NULL, path TEXT NOT NULL, hash {ty} NOT NULL,
               mode INTEGER NOT NULL DEFAULT 0);
             CREATE INDEX idx_filemap_snap ON fm (snapshot_hash);
             CREATE INDEX idx_filemap_path ON fm (path);"
        )
    })
    .unwrap();
    let before = db_bytes(&conn);
    let t = Instant::now();
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut stmt = conn
            .prepare("INSERT INTO fm (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, 420)")
            .unwrap();
        for i in 0..rows {
            if blob {
                stmt.execute(rusqlite::params![
                    bin_hash(1_000_000),
                    row_path(i),
                    bin_hash(i)
                ])
            } else {
                stmt.execute(rusqlite::params![
                    hex_hash(1_000_000),
                    row_path(i),
                    hex_hash(i)
                ])
            }
            .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
    let insert_ms = t.elapsed().as_secs_f64() * 1000.0;
    let bytes = db_bytes(&conn) - before;
    json!({
        "variant": format!("schema_{shape}"),
        "scope": "row-insert stage only",
        "rows": rows,
        "insert_ms": insert_ms,
        "bytes_per_row": bytes as f64 / rows as f64,
    })
}

type Setup = fn(&Scratch);

/// Run every variant; returns the emitted JSON lines.
fn run(p: &Params) -> Vec<Value> {
    let mut s = build(p);
    let mut out: Vec<Value> = Vec::new();
    let mut base_median = 0.0;
    let note = format!(
        "{} samples per variant over 11 variants add {} snapshots to a history of {}: negligible",
        p.samples,
        p.samples * 11,
        p.history
    );

    let variants: Vec<(&str, Option<Setup>)> = vec![
        ("baseline", None),
        (
            "synchronous=OFF",
            Some(|s| {
                s.repo
                    .conn()
                    .pragma_update(None, "synchronous", "OFF")
                    .unwrap()
            }),
        ),
        (
            "synchronous=FULL",
            Some(|s| {
                s.repo
                    .conn()
                    .pragma_update(None, "synchronous", "FULL")
                    .unwrap()
            }),
        ),
        (
            "wal_autocheckpoint=0",
            Some(|s| set_pragma(s, "wal_autocheckpoint", 0)),
        ),
        (
            "wal_autocheckpoint=10000",
            Some(|s| set_pragma(s, "wal_autocheckpoint", 10_000)),
        ),
        (
            "cache_size=-16384",
            Some(|s| set_pragma(s, "cache_size", -16_384)),
        ),
        (
            "cache_size=-262144",
            Some(|s| set_pragma(s, "cache_size", -262_144)),
        ),
        ("mmap_size=0", Some(|s| set_pragma(s, "mmap_size", 0))),
        (
            "mmap_size=1073741824",
            Some(|s| set_pragma(s, "mmap_size", 1_073_741_824)),
        ),
        ("baseline_again", None),
        // Destructive, so last.
        (
            "drop_idx_filemap_path",
            Some(|s| {
                s.repo
                    .conn()
                    .execute_batch("DROP INDEX idx_filemap_path")
                    .unwrap()
            }),
        ),
    ];

    for (i, (name, setup)) in variants.iter().enumerate() {
        if let Some(f) = setup {
            f(&s);
        }
        let (totals, mut stages) = run_variant(&mut s, p.samples);
        if *name == "wal_autocheckpoint=0" {
            stages.insert("wal_checkpoint_passive_ms".into(), json!(checkpoint_ms(&s)));
        }
        restore_defaults(&s);
        let median = quantile(&sorted(&totals), 0.5);
        if i == 0 {
            base_median = median;
        }
        let delta = median - base_median;
        let line = json!({
            "variant": name,
            "files": p.files,
            "history": p.history,
            "samples": totals.len(),
            "median_ms": median,
            "p95_ms": quantile(&sorted(&totals), 0.95),
            "delta_ms": delta,
            "delta_pct": if base_median > 0.0 { delta / base_median * 100.0 } else { 0.0 },
            "stages": Value::Object(stages),
            "note": note,
        });
        emit(&line);
        out.push(line);
    }

    for shape in ["rowid_two_indexes", "without_rowid", "blob_hashes"] {
        let line = row_insert(shape, p.files);
        emit(&line);
        out.push(line);
    }
    out
}

#[test]
#[ignore = "measurement: run with --release --features timing"]
fn sqlite_options_one_file_save() {
    run(&Params::from_env());
}

#[test]
fn sqlite_options_run_at_a_small_size() {
    let lines = run(&Params {
        files: 200,
        history: 2,
        samples: 1,
        bulk: false,
        dir: None,
    });
    let names: Vec<&str> = lines
        .iter()
        .map(|l| l["variant"].as_str().unwrap())
        .collect();
    for want in [
        "baseline",
        "synchronous=OFF",
        "synchronous=FULL",
        "wal_autocheckpoint=0",
        "wal_autocheckpoint=10000",
        "cache_size=-16384",
        "cache_size=-262144",
        "mmap_size=0",
        "mmap_size=1073741824",
        "baseline_again",
        "drop_idx_filemap_path",
        "schema_without_rowid",
        "schema_blob_hashes",
    ] {
        assert!(names.contains(&want), "missing variant {want}");
    }
    assert_eq!(lines[0]["delta_ms"], json!(0.0));
    assert!(lines[3]["stages"]["wal_checkpoint_passive_ms"].is_number());
    assert!(lines[1]["delta_pct"].is_number());
    assert!(lines[12]["bytes_per_row"].as_f64().unwrap() > 0.0);
}
