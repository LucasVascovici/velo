//! The performance guard CI runs on every pull request. Phase 19.7-a.
//!
//! Absolute milliseconds on a shared runner measure the runner, so each
//! workload is divided by a calibration run on the same machine in the same
//! minute (a fixed amount of hashing, small-file I/O and SQLite inserts). The
//! ratio travels between machines far better than a time does. It is compared
//! against `bench/perf/baseline.json`, which a pull request that moves a ratio
//! updates deliberately:
//!
//! ```text
//! VELO_PERF_UPDATE=1 cargo test -p velo-core --release perf_guard -- --ignored
//! ```
//!
//! The measuring test is `#[ignore]`d; the judging and the baseline file
//! handling are plain functions with ordinary tests.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use super::perf_support::{agent_edit, project, Layout, Rng};
use crate::commands::save;
use crate::tree::{Change, SaveChanges, TreeEntry};
use crate::{Author, BranchName, Repo, SnapshotMeta};

/// How far a ratio may drift from its baseline, in either direction, before the
/// guard says something. Shared runners are noisy; a real regression doubles.
const TOLERANCE: f64 = 1.6;
const ROUNDS: usize = 3;
const AGENT_FILES: usize = 1_000;
const AGENT_STEPS: usize = 500;

/// `VELO_PERF_AGENT_STEPS` shortens the agent workload where it is too slow to
/// run in full (the Windows CI runner). The ratio then scales with the step
/// count, so a shortened run is advisory, not comparable to the committed baseline.
fn agent_steps_count() -> usize {
    std::env::var("VELO_PERF_AGENT_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(AGENT_STEPS)
}
const SCALE_FILES: usize = 10_000;
const SAVE_SAMPLES: usize = 5;

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

/// A fixed amount of hashing, small-file I/O and SQLite inserts. Returns the
/// total time.
fn calibrate() -> Duration {
    let start = Instant::now();

    // Hashing: 64 MiB of seeded bytes, in memory.
    let mut rng = Rng::new(7);
    let mut block = vec![0u8; 1 << 20];
    for chunk in block.chunks_mut(8) {
        let v = rng.next_u64().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    let mut hasher = blake3::Hasher::new();
    for i in 0..64u8 {
        block[0] = i;
        hasher.update(&block);
    }
    std::hint::black_box(hasher.finalize());

    // File I/O: 1,000 files of 1 KiB, temp file plus rename, then read back.
    let dir = TempDir::new().unwrap();
    let payload = vec![0xA5u8; 1024];
    for i in 0..1_000 {
        let tmp = dir.path().join(format!("f{i}.tmp"));
        let dst = dir.path().join(format!("f{i}"));
        let mut f = fs::File::create(&tmp).unwrap();
        f.write_all(&payload).unwrap();
        drop(f);
        fs::rename(&tmp, &dst).unwrap();
    }
    for i in 0..1_000 {
        std::hint::black_box(fs::read(dir.path().join(format!("f{i}"))).unwrap());
    }

    // SQLite: 100,000 rows into a table with two indexes, one transaction.
    let conn = rusqlite::Connection::open(dir.path().join("scratch.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE t (a INTEGER, b BLOB, c TEXT);
         CREATE INDEX t_b ON t (b);
         CREATE INDEX t_c ON t (c);
         BEGIN;",
    )
    .unwrap();
    {
        let mut ins = conn
            .prepare("INSERT INTO t (a, b, c) VALUES (?, ?, ?)")
            .unwrap();
        for i in 0..100_000i64 {
            let h = blake3::hash(&i.to_le_bytes());
            ins.execute(rusqlite::params![
                i,
                &h.as_bytes()[..],
                format!("path/{i:06}")
            ])
            .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
    drop(conn);
    start.elapsed()
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

/// 500 `save_changes` steps over a 1,000-file Agent project, as the agent
/// benchmark does them. Setup is not timed.
fn agent_steps() -> Duration {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let mut rng = Rng::new(1);
    let initial = project(Layout::Agent, AGENT_FILES, &mut rng);
    let paths: Vec<String> = initial.iter().map(|(p, _)| p.clone()).collect();
    let mut contents: HashMap<String, Vec<u8>> = initial.into_iter().collect();
    let author = Author::new("agent-run-1").unwrap();
    let branch = main_branch();
    let guard = repo.write().unwrap();

    let mut spec = SaveChanges::new(&branch, None, "first");
    spec.changes = paths
        .iter()
        .map(|p| Change::Put(TreeEntry::file(p.clone(), contents[p].clone())))
        .collect();
    let mut parent = guard.save_changes(spec).unwrap().snapshot;

    let mut total = Duration::ZERO;
    for step in 1..=agent_steps_count() {
        let edits = agent_edit(&mut rng, &paths, &contents);
        let message = format!("step {step}");
        let mut meta = SnapshotMeta::new();
        meta.set("agent", "run", "run-1").unwrap();
        meta.set("agent", "tool_call_id", step.to_string()).unwrap();
        meta.set("agent", "model", "bench").unwrap();
        let mut spec = SaveChanges::new(&branch, Some(&parent), &message);
        spec.author = Some(&author);
        spec.meta = meta;
        spec.changes = edits
            .iter()
            .map(|(p, b)| Change::Put(TreeEntry::file(p.clone(), b.clone())))
            .collect();
        let t = Instant::now();
        parent = guard.save_changes(spec).unwrap().snapshot;
        total += t.elapsed();
        for (p, b) in edits {
            contents.insert(p, b);
        }
    }
    total
}

/// A working-tree repository of 10,000 Scale files, saved once untimed; then
/// the median of five one-file saves.
fn save_10k() -> Duration {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    crate::commands::init::run(root).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let mut rng = Rng::new(1);
    let files = project(Layout::Scale, SCALE_FILES, &mut rng);
    for (path, bytes) in &files {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let guard = repo.write().unwrap();
    save::run(&guard, Some("first"), save::Options::default()).unwrap();

    let mut samples = Vec::new();
    for n in 0..SAVE_SAMPLES {
        let (path, bytes) = &files[(n * 997) % files.len()];
        let mut edited = bytes.clone();
        edited.extend_from_slice(format!("edit {n}\n").as_bytes());
        fs::write(root.join(path), edited).unwrap();
        let t = Instant::now();
        let out = save::run(&guard, Some("one file"), save::Options::default()).unwrap();
        samples.push(t.elapsed().as_secs_f64());
        assert!(
            out.into_result().is_some(),
            "the edit must produce a snapshot"
        );
    }
    Duration::from_secs_f64(median(&mut samples))
}

/// What the guard concluded about one workload.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Outcome {
    /// Within tolerance of the baseline.
    Ok,
    Faster,
    Regressed,
    /// No baseline entry yet: reported, passes.
    NoBaseline,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Verdict {
    pub(super) name: String,
    pub(super) measured: f64,
    pub(super) baseline: Option<f64>,
    pub(super) outcome: Outcome,
}

impl Verdict {
    pub(super) fn failed(&self) -> bool {
        self.outcome == Outcome::Regressed
    }

    fn line(&self) -> String {
        let base = self.baseline.map_or("-".to_string(), |b| format!("{b:.4}"));
        let note = match self.outcome {
            Outcome::Ok => "ok",
            Outcome::Faster => "faster: update the baseline",
            Outcome::Regressed => "FAIL: slower than the baseline allows",
            Outcome::NoBaseline => "no baseline entry: reported, passes",
        };
        format!(
            "{:<12} measured {:.4}  baseline {base}  {note}",
            self.name, self.measured
        )
    }
}

/// Compare measured ratios to the baseline. Above `baseline * tolerance` fails;
/// below `baseline / tolerance` is an improvement to record, not a failure.
pub(super) fn judge(
    measured: &BTreeMap<String, f64>,
    baseline: &BTreeMap<String, f64>,
    tolerance: f64,
) -> Vec<Verdict> {
    measured
        .iter()
        .map(|(name, &m)| {
            let base = baseline.get(name).copied();
            let outcome = match base {
                None => Outcome::NoBaseline,
                Some(b) if m > b * tolerance => Outcome::Regressed,
                Some(b) if m < b / tolerance => Outcome::Faster,
                Some(_) => Outcome::Ok,
            };
            Verdict {
                name: name.clone(),
                measured: m,
                baseline: base,
                outcome,
            }
        })
        .collect()
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/perf/baseline.json")
}

/// The whole file, `{os: {workload: ratio}}`. A missing file is an empty one.
fn read_file(path: &Path) -> BTreeMap<String, BTreeMap<String, f64>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).expect("baseline.json is not valid"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => panic!("cannot read {}: {e}", path.display()),
    }
}

/// Replace `os`'s entry, leaving the others as they were.
fn write_entry(path: &Path, os: &str, ratios: &BTreeMap<String, f64>) {
    let mut all = read_file(path);
    all.insert(os.to_string(), ratios.clone());
    let mut text = serde_json::to_string_pretty(&all).unwrap();
    text.push('\n');
    fs::write(path, text).unwrap();
}

fn measure() -> BTreeMap<String, f64> {
    let (mut cal, mut agent, mut save) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        cal.push(calibrate().as_secs_f64());
        agent.push(agent_steps().as_secs_f64());
        save.push(save_10k().as_secs_f64());
    }
    let cal = median(&mut cal);
    let mut out = BTreeMap::new();
    out.insert("agent_steps".to_string(), median(&mut agent) / cal);
    out.insert("save_10k".to_string(), median(&mut save) / cal);
    out
}

#[test]
#[ignore = "performance guard: run in release mode, see the module docs"]
#[allow(clippy::print_stderr)]
fn perf_guard() {
    let measured = measure();
    let os = std::env::consts::OS;
    let report: Value = json!({ os: &measured });
    // The test harness captures this; CI uploads the line as an artifact.
    eprintln!("PERF_JSON {report}");

    let path = baseline_path();
    if std::env::var("VELO_PERF_UPDATE").as_deref() == Ok("1") {
        write_entry(&path, os, &measured);
        return;
    }
    let all = read_file(&path);
    let baseline = all.get(os).cloned().unwrap_or_default();
    let verdicts = judge(&measured, &baseline, TOLERANCE);
    let table: Vec<String> = verdicts.iter().map(Verdict::line).collect();
    eprintln!("{}", table.join("\n"));
    if verdicts.iter().any(Verdict::failed) {
        panic!(
            "performance regression (tolerance {TOLERANCE}x):\n{}",
            table.join("\n")
        );
    }
}

fn map(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn judge_fails_double_the_baseline() {
    let v = judge(&map(&[("a", 2.0)]), &map(&[("a", 1.0)]), TOLERANCE);
    assert_eq!(v[0].outcome, Outcome::Regressed);
    assert!(v[0].failed());
}

#[test]
fn judge_passes_within_tolerance() {
    let v = judge(&map(&[("a", 1.2)]), &map(&[("a", 1.0)]), TOLERANCE);
    assert_eq!(v[0].outcome, Outcome::Ok);
    assert!(!v[0].failed());
}

#[test]
fn judge_reports_an_improvement_without_failing() {
    let v = judge(&map(&[("a", 0.5)]), &map(&[("a", 1.0)]), TOLERANCE);
    assert_eq!(v[0].outcome, Outcome::Faster);
    assert!(!v[0].failed());
    assert!(v[0].line().contains("faster: update the baseline"));
}

#[test]
fn judge_passes_a_missing_baseline_entry() {
    let v = judge(&map(&[("a", 9.0)]), &BTreeMap::new(), TOLERANCE);
    assert_eq!(v[0].outcome, Outcome::NoBaseline);
    assert!(!v[0].failed());
}

#[test]
fn baseline_file_round_trips_and_keeps_other_systems() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("baseline.json");
    assert!(read_file(&path).is_empty());
    write_entry(&path, "linux", &map(&[("agent_steps", 1.5)]));
    write_entry(&path, "windows", &map(&[("save_10k", 2.5)]));
    write_entry(
        &path,
        "linux",
        &map(&[("agent_steps", 1.75), ("save_10k", 3.0)]),
    );
    let all = read_file(&path);
    assert_eq!(all["windows"], map(&[("save_10k", 2.5)]));
    assert_eq!(
        all["linux"],
        map(&[("agent_steps", 1.75), ("save_10k", 3.0)])
    );
}

#[test]
fn the_committed_baseline_parses() {
    let all = read_file(&baseline_path());
    for ratios in all.values() {
        assert!(ratios.values().all(|r| r.is_finite() && *r > 0.0));
    }
}
