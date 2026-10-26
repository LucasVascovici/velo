//! Profiling harness: where `save`, `save_tree` and `restore` spend their time
//! at a chosen tree size and history depth, and the agent-benchmark workload
//! driven straight through `save_tree` so the engine can be told apart from a
//! binding. Phase 19.1-b.
//!
//! The measurements are `#[ignore]`d and run on demand, in release mode, with
//! the stage timers compiled in:
//!
//! ```text
//! cargo test -p velody-core --release --features timing perf_profile \
//!     -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Each op prints one JSON object to stderr and appends it to the file named by
//! `VELODY_PROFILE_OUT` when that is set. Knobs, with defaults: `VELODY_PROFILE_FILES`
//! (1000), `VELODY_PROFILE_HISTORY` (0), `VELODY_PROFILE_SAMPLES` (10),
//! `VELODY_PROFILE_BULK` (0), `VELODY_PROFILE_DIR` (the system temp directory) and
//! `VELODY_PROFILE_STEPS` (500). A small non-ignored test drives the same code so
//! the harness cannot rot.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use super::perf_support::{agent_edit, build_history, predict_object, project, Layout, Rng};
use crate::commands::{restore, save};
use crate::tree::{Content, SaveTree, TreeEntry};
use crate::{timing, Author, BranchName, ObjectHash, Repo, SnapshotId, SnapshotMeta};

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn temp_dir(parent: &Option<PathBuf>) -> TempDir {
    match parent {
        Some(p) => TempDir::new_in(p).unwrap(),
        None => TempDir::new().unwrap(),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[at]
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    quantile(&v, 0.5)
}

fn p95(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    quantile(&v, 0.95)
}

/// Median per lap name over the samples; a lap that repeats within one sample
/// counts as the sum of its repeats.
#[derive(Default)]
struct Stages(BTreeMap<String, Vec<f64>>);

impl Stages {
    fn add(&mut self, name: &str, millis: f64) {
        self.0.entry(name.to_owned()).or_default().push(millis);
    }

    /// Record one sample's laps, as drained from the timers.
    fn add_laps(&mut self, laps: Vec<timing::Lap>) {
        let mut sum: BTreeMap<&str, f64> = BTreeMap::new();
        for l in laps {
            *sum.entry(l.name).or_default() += ms(l.elapsed);
        }
        for (name, v) in sum {
            self.add(name, v);
        }
    }

    fn medians(&self) -> Value {
        let map: serde_json::Map<String, Value> = self
            .0
            .iter()
            .map(|(k, v)| (k.clone(), json!(median(v))))
            .collect();
        Value::Object(map)
    }
}

/// Print `line` to stderr and append it to `VELODY_PROFILE_OUT` when set.
#[allow(clippy::print_stderr)]
fn emit(line: &Value) {
    let text = line.to_string();
    eprintln!("{text}");
    if let Ok(out) = std::env::var("VELODY_PROFILE_OUT") {
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
            files: env_usize("VELODY_PROFILE_FILES", 1000),
            history: env_usize("VELODY_PROFILE_HISTORY", 0),
            samples: env_usize("VELODY_PROFILE_SAMPLES", 10),
            bulk: env_usize("VELODY_PROFILE_BULK", 0) != 0,
            dir: std::env::var("VELODY_PROFILE_DIR").ok().map(PathBuf::from),
        }
    }

    fn op(&self, op: &str, totals: &[f64], stages: &Stages) -> Value {
        json!({
            "op": op,
            "files": self.files,
            "history": self.history,
            "bulk": self.bulk,
            "samples": totals.len(),
            "os": std::env::consts::OS,
            "total_ms": { "median": median(totals), "p95": p95(totals) },
            "stages": stages.medians(),
        })
    }
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

fn save_now(repo: &Repo, message: &str) -> SnapshotId {
    repo.write()
        .unwrap()
        .pipe_save(message)
        .expect("the working tree changed, so a snapshot is expected")
}

/// Small shim so the call site reads as one step.
trait PipeSave {
    fn pipe_save(&self, message: &str) -> Option<SnapshotId>;
}

impl PipeSave for crate::WriteGuard<'_> {
    fn pipe_save(&self, message: &str) -> Option<SnapshotId> {
        save::run(self, Some(message), save::Options::default())
            .unwrap()
            .into_result()
            .map(|r| r.hash)
    }
}

fn restore_to(repo: &Repo, id: &SnapshotId) {
    restore::run(
        &repo.write().unwrap(),
        id,
        restore::Options {
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
}

/// Materialise `id` into `dest` through the library's read API, as a caller
/// without `restore` would. Returns the (tree_at, read, write) times.
fn read_loop_once(repo: &Repo, id: &SnapshotId, dest: &Path) -> (Duration, Duration, Duration) {
    let t = Instant::now();
    let tree = repo.tree_at(id).unwrap();
    let tree_at = t.elapsed();
    let (mut read, mut write) = (Duration::ZERO, Duration::ZERO);
    for f in &tree {
        let t = Instant::now();
        let bytes = repo.read_file_at(id, &f.path).unwrap();
        read += t.elapsed();
        let t = Instant::now();
        let target = dest.join(&f.path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
        write += t.elapsed();
    }
    (tree_at, read, write)
}

/// The save / save_tree / restore / read-loop profile. Returns the JSON lines it
/// emitted, one per op.
fn save_restore(p: &Params) -> Vec<Value> {
    let tmp = temp_dir(&p.dir);
    let root = tmp.path();
    crate::commands::init::run(root).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let mut rng = Rng::new(1);
    let files = project(Layout::Scale, p.files, &mut rng);
    for (path, bytes) in &files {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
    let mut out = Vec::new();

    // First save.
    timing::drain_laps();
    let t = Instant::now();
    let first = save_now(&repo, "first");
    let total = ms(t.elapsed());
    let mut stages = Stages::default();
    stages.add_laps(timing::drain_laps());
    out.push(p.op("first_save", &[total], &stages));

    // History, then put the disk where PARENT is.
    let history = build_history(&repo, &main_branch(), &first, p.history, p.bulk, &mut rng);
    let mid = history.get(history.len() / 2).unwrap_or(&first).clone();
    let tip_of_history = history.last().unwrap_or(&first).clone();
    restore_to(&repo, &tip_of_history);

    // Save: one file rewritten on disk per sample.
    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    let mut tip = tip_of_history;
    for i in 0..p.samples {
        let path = paths[rng.below(paths.len())];
        fs::write(
            root.join(path),
            format!("sample {i} {}\n", rng.next_u64()).as_bytes(),
        )
        .unwrap();
        timing::drain_laps();
        let t = Instant::now();
        tip = save_now(&repo, "sample");
        totals.push(ms(t.elapsed()));
        stages.add_laps(timing::drain_laps());
    }
    out.push(p.op("save", &totals, &stages));

    // save_tree on a side branch, from the tip, as a library caller would.
    let side: BranchName = "side".parse().unwrap();
    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    let mut parent = tip.clone();
    for i in 0..p.samples {
        let t = Instant::now();
        let tree = repo.tree_at(&parent).unwrap();
        let tree_at = ms(t.elapsed());
        let t2 = Instant::now();
        let mut entries: Vec<TreeEntry> = tree
            .into_iter()
            .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
            .collect();
        let at = rng.below(entries.len());
        let path = entries[at].path.clone();
        entries[at] = TreeEntry::file(path, format!("side {i} {}\n", rng.next_u64()).into_bytes());
        let entries_ms = ms(t2.elapsed());
        timing::drain_laps();
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &side,
                parent: Some(&parent),
                merge_parent: None,
                message: "side",
                entries,
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: None,
                renames: &[],
            })
            .unwrap();
        totals.push(ms(t.elapsed()));
        stages.add("caller.tree_at", tree_at);
        stages.add("caller.entries", entries_ms);
        stages.add_laps(timing::drain_laps());
        parent = id;
    }
    out.push(p.op("save_tree", &totals, &stages));

    // Full restores, alternating between the middle of the history and the tip.
    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    for i in 0..p.samples {
        let target = if i % 2 == 0 { &mid } else { &tip };
        timing::drain_laps();
        let t = Instant::now();
        restore_to(&repo, target);
        totals.push(ms(t.elapsed()));
        stages.add_laps(timing::drain_laps());
    }
    out.push(p.op("restore", &totals, &stages));

    // The library read loop into an empty directory.
    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    for _ in 0..p.samples {
        let dest = TempDir::new().unwrap();
        let t = Instant::now();
        let (tree_at, read, write) = read_loop_once(&repo, &tip, dest.path());
        totals.push(ms(t.elapsed()));
        stages.add("tree_at", ms(tree_at));
        stages.add("read_file_at", ms(read));
        stages.add("write", ms(write));
    }
    out.push(p.op("read_loop", &totals, &stages));

    for line in &out {
        emit(line);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    AsHarness,
    NoReread,
    SaveTreeOnly,
}

impl Variant {
    fn name(self) -> &'static str {
        match self {
            Variant::AsHarness => "as_harness",
            Variant::NoReread => "no_reread",
            Variant::SaveTreeOnly => "save_tree_only",
        }
    }
}

/// Run the agent workload once in a fresh repository. Returns the JSON for the
/// variant and the snapshot ids per step (index 0 is the initial save), with the
/// repository kept alive for the restore measurement.
fn agent_variant(
    variant: Variant,
    files: usize,
    steps: usize,
    dir: &Option<PathBuf>,
) -> (Value, Vec<SnapshotId>, TempDir, Repo) {
    let tmp = temp_dir(dir);
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let mut rng = Rng::new(1);
    let initial = project(Layout::Agent, files, &mut rng);
    let paths: Vec<String> = initial.iter().map(|(p, _)| p.clone()).collect();
    let mut contents: HashMap<String, Vec<u8>> = initial.into_iter().collect();
    let author = Author::new("agent-run-1").unwrap();
    let branch = main_branch();

    let save = |parent: Option<&SnapshotId>, entries: Vec<TreeEntry>, step: usize| {
        let mut meta = SnapshotMeta::new();
        meta.set("agent", "run", "run-1").unwrap();
        meta.set("agent", "tool_call_id", step.to_string()).unwrap();
        meta.set("agent", "model", "bench").unwrap();
        repo.write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent,
                merge_parent: None,
                message: &format!("step {step}"),
                entries,
                meta,
                author: Some(&author),
                timestamp_ms: None,
                renames: &[],
            })
            .unwrap()
    };

    let first = save(
        None,
        paths
            .iter()
            .map(|p| TreeEntry::file(p.clone(), contents[p].clone()))
            .collect(),
        0,
    );
    let mut ids = vec![first];
    let mut known: HashMap<String, ObjectHash> = repo
        .tree_at(&ids[0])
        .unwrap()
        .into_iter()
        .map(|f| (f.path, f.object))
        .collect();

    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    let wall = Instant::now();
    for step in 1..=steps {
        let edits = agent_edit(&mut rng, &paths, &contents);
        let t = Instant::now();
        let changed: HashMap<&str, &Vec<u8>> = edits.iter().map(|(p, b)| (p.as_str(), b)).collect();
        let entries: Vec<TreeEntry> = paths
            .iter()
            .map(|p| match changed.get(p.as_str()) {
                Some(b) => TreeEntry::file(p.clone(), (*b).clone()),
                None => {
                    TreeEntry::stored(p.clone(), known[p].clone(), crate::tree::FileKind::Regular)
                }
            })
            .collect();
        timing::drain_laps();
        let call = Instant::now();
        let id = save(ids.last(), entries, step);
        let call_ms = ms(call.elapsed());
        stages.add_laps(
            timing::drain_laps()
                .into_iter()
                .filter(|l| l.name.starts_with("save_tree."))
                .collect(),
        );
        match variant {
            Variant::AsHarness => {
                known = repo
                    .tree_at(&id)
                    .unwrap()
                    .into_iter()
                    .map(|f| (f.path, f.object))
                    .collect();
            }
            Variant::NoReread | Variant::SaveTreeOnly => {
                for (p, b) in &edits {
                    known.insert(p.clone(), predict_object(b));
                }
            }
        }
        totals.push(if variant == Variant::SaveTreeOnly {
            call_ms
        } else {
            ms(t.elapsed())
        });
        for (p, b) in edits {
            contents.insert(p, b);
        }
        ids.push(id);
    }
    let wall_s = wall.elapsed().as_secs_f64();

    let mut v = json!({
        "wall_s": wall_s,
        "total_s": totals.iter().sum::<f64>() / 1000.0,
        "step_ms": { "median": median(&totals), "p95": p95(&totals) },
        "stages": stages.medians(),
    });
    if variant != Variant::AsHarness {
        // The prediction is only worth reporting if it was right.
        let last = repo.tree_at(ids.last().unwrap()).unwrap();
        v["prediction_ok"] = json!(last.iter().all(|f| known[&f.path] == f.object));
    }
    (v, ids, tmp, repo)
}

/// The agent workload three ways plus the restore measurement.
fn agent_workload(files: usize, steps: usize, restores: usize, dir: &Option<PathBuf>) -> Value {
    let mut variants = serde_json::Map::new();
    let mut restore_ms = 0.0;
    let restore_step = 250.min(steps / 2).max(1).min(steps);
    for variant in [Variant::AsHarness, Variant::NoReread, Variant::SaveTreeOnly] {
        let (v, ids, _tmp, repo) = agent_variant(variant, files, steps, dir);
        variants.insert(variant.name().to_owned(), v);
        if variant == Variant::AsHarness {
            let mut times = Vec::new();
            for _ in 0..restores {
                let dest = TempDir::new().unwrap();
                let t = Instant::now();
                read_loop_once(&repo, &ids[restore_step], dest.path());
                times.push(ms(t.elapsed()));
            }
            restore_ms = median(&times);
        }
    }
    let line = json!({
        "op": "agent_workload",
        "rng": "xorshift",
        "files": files,
        "steps": steps,
        "os": std::env::consts::OS,
        "variants": Value::Object(variants),
        "restore_step": restore_step,
        "restore_ms_median": restore_ms,
    });
    emit(&line);
    line
}

#[test]
#[ignore = "measurement: run with --release --features timing"]
fn profile_save_restore() {
    save_restore(&Params::from_env());
}

#[test]
#[ignore = "measurement: run with --release --features timing"]
fn profile_agent_workload() {
    let p = Params::from_env();
    agent_workload(p.files, env_usize("VELODY_PROFILE_STEPS", 500), 100, &p.dir);
}

#[test]
fn save_restore_profile_runs_at_a_small_size() {
    let lines = save_restore(&Params {
        files: 50,
        history: 3,
        samples: 2,
        bulk: false,
        dir: None,
    });
    let ops: Vec<&str> = lines.iter().map(|l| l["op"].as_str().unwrap()).collect();
    assert_eq!(
        ops,
        ["first_save", "save", "save_tree", "restore", "read_loop"]
    );
    let has = |op: &str, prefix: &str| {
        lines.iter().find(|l| l["op"] == op).unwrap()["stages"]
            .as_object()
            .unwrap()
            .keys()
            .any(|k| k.starts_with(prefix))
    };
    assert!(has("save", "save."));
    assert!(has("save_tree", "caller.tree_at"));
    assert!(has("save_tree", "save_tree."));
    assert!(has("restore", "restore."));
}

#[test]
fn agent_workload_runs_at_a_small_size() {
    let line = agent_workload(30, 6, 2, &None);
    for name in ["as_harness", "no_reread", "save_tree_only"] {
        assert!(line["variants"][name]["step_ms"]["median"].is_number());
    }
    assert_eq!(line["variants"]["no_reread"]["prediction_ok"], json!(true));
    assert!(line["restore_ms_median"].as_f64().unwrap() > 0.0);
    // Entries built from stored hashes never carry bytes.
    assert!(matches!(
        TreeEntry::stored("a", predict_object(b"x"), crate::tree::FileKind::Regular).content,
        Content::Stored(_)
    ));
}
