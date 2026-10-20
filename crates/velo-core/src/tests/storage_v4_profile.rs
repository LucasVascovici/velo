//! 11.5's prototype table, reproduced on the real v4 engine (20.9-a).
//!
//! Ignored: it is a measurement. Every save is the real `WriteGuard::save_tree`
//! with `Repo::set_keyframe_interval(k)`, one file changed per save, so the
//! numbers include everything the prototype left out (object validation, the
//! parent tree read, the snapshot id, the delta or keyframe rows, the ref
//! update and the commit). Per save it records wall time and whether the save
//! became a keyframe; per K it records disk per snapshot (`page_count *
//! page_size` growth, as the prototype did), `tree_at` and `read_file_at` at
//! depth 0, K/2 and K-1, and the median, p90, p99 and max of the keyframe saves
//! (the spike). With `--features timing` the `save_tree.*` laps are summed per
//! stage and the medians are reported for plain and keyframe saves.
//!
//! ```text
//! VELO_PROFILE_FILES=100000 VELO_PROFILE_KS=8,32,128,512 VELO_PROFILE_OUT=out.json \
//!     cargo test -p velo-core --release --features timing storage_v4_profile \
//!     -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Knobs: `VELO_PROFILE_FILES` (100000), `VELO_PROFILE_SAVES` (default
//! `k * keyframes + 16`; an explicit value is used as given), `VELO_PROFILE_KEYFRAMES` (4: how
//! many keyframe saves the default count yields, so the spike has a distribution), `VELO_PROFILE_KS` (`8,32,128,512`),
//! `VELO_PROFILE_OUT` (append JSON lines), `VELO_PROFILE_DIR` (where the
//! repository lives; use ext4 under WSL).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use super::perf_support::{project, Layout, Rng};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[allow(clippy::print_stderr)]
fn emit(line: &Value) {
    let text = line.to_string();
    eprintln!("{text}");
    if let Ok(out) = std::env::var("VELO_PROFILE_OUT") {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)
            .unwrap();
        writeln!(f, "{text}").unwrap();
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn quantile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let at = ((v.len() as f64 * q).ceil() as usize).clamp(1, v.len()) - 1;
    v[at]
}

fn temp_repo() -> (TempDir, Repo) {
    let tmp = match std::env::var("VELO_PROFILE_DIR") {
        Ok(p) => TempDir::new_in(PathBuf::from(p)).unwrap(),
        Err(_) => TempDir::new().unwrap(),
    };
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn db_bytes(repo: &Repo) -> i64 {
    let q = |pragma: &str| -> i64 {
        repo.conn()
            .query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))
            .unwrap()
    };
    q("page_count") * q("page_size")
}

fn is_keyframe(repo: &Repo, id: &SnapshotId) -> bool {
    let deltas: i64 = repo
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tree_deltas WHERE snapshot_hash = ?",
            [id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    deltas == 0
}

fn timed_reps<T>(reps: usize, mut f: impl FnMut() -> T) -> f64 {
    let mut v = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        std::hint::black_box(f());
        v.push(ms(t.elapsed()));
    }
    quantile(&v, 0.5)
}

/// One save of `entries` on top of `parent`.
fn save(repo: &Repo, parent: Option<&SnapshotId>, entries: Vec<TreeEntry>, n: i64) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: "s",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(n),
            renames: &[],
        })
        .unwrap()
}

/// Stage laps of one save: name to milliseconds. Empty without `timing`.
fn laps() -> BTreeMap<&'static str, f64> {
    #[cfg(feature = "timing")]
    {
        let mut m = BTreeMap::new();
        for l in crate::timing::drain_laps() {
            *m.entry(l.name).or_insert(0.0) += ms(l.elapsed);
        }
        m
    }
    #[cfg(not(feature = "timing"))]
    BTreeMap::new()
}

fn stage_medians(saves: &[BTreeMap<&'static str, f64>]) -> Value {
    let mut names: Vec<&str> = saves.iter().flat_map(|s| s.keys().copied()).collect();
    names.sort_unstable();
    names.dedup();
    let mut out = serde_json::Map::new();
    for name in names {
        let v: Vec<f64> = saves
            .iter()
            .map(|s| s.get(name).copied().unwrap_or(0.0))
            .collect();
        out.insert(name.into(), json!(quantile(&v, 0.5)));
    }
    Value::Object(out)
}

fn run_k(files: usize, saves: Option<usize>, k: u32) -> Value {
    let (_tmp, mut repo) = temp_repo();
    repo.set_keyframe_interval(k);
    let mut rng = Rng::new(1);
    let mut paths: Vec<String> = project(Layout::Scale, files, &mut rng)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    paths.sort();
    let seed: Vec<TreeEntry> = paths
        .iter()
        .map(|p| TreeEntry::file(p.clone(), format!("file {p}\n").into_bytes()))
        .collect();

    // Seed: the first snapshot is a keyframe and is not measured.
    let t = Instant::now();
    let root = save(&repo, None, seed, 0);
    let seed_ms = ms(t.elapsed());
    laps();
    // Carry the stored objects forward so later saves hash only the edit.
    let mut entries: Vec<TreeEntry> = repo
        .tree_at(&root)
        .unwrap()
        .into_iter()
        .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
        .collect();

    // An explicit save count is honoured. The default runs long enough for
    // `VELO_PROFILE_KEYFRAMES` keyframes after the seed, so the keyframe spike
    // is a distribution and not a single sample.
    let keyframes = env_usize("VELO_PROFILE_KEYFRAMES", 4);
    let saves = saves.unwrap_or(k as usize * keyframes + 16);
    let before = db_bytes(&repo);
    let mut ids = vec![root];
    let (mut plain, mut keys) = (Vec::new(), Vec::new());
    let (mut plain_laps, mut key_laps) = (Vec::new(), Vec::new());
    for n in 0..saves {
        let at = rng.below(entries.len());
        let path = entries[at].path.clone();
        entries[at] = TreeEntry::file(path, format!("edit {n} {}\n", rng.next_u64()).into_bytes());
        // The clone is the caller's cost, not the save's: it is outside the timer.
        let input = entries.clone();
        let t = Instant::now();
        let id = save(&repo, ids.last(), input, 1 + n as i64);
        let wall = ms(t.elapsed());
        let l = laps();
        if is_keyframe(&repo, &id) {
            keys.push(wall);
            key_laps.push(l);
        } else {
            plain.push(wall);
            plain_laps.push(l);
        }
        ids.push(id);
    }
    let disk = db_bytes(&repo) - before;

    // Reads at depth 0, K/2 and K-1 within the first cycle (ids[d] is depth d).
    let mut probes = Vec::new();
    // Depths past the history of a short run are clamped to its newest snapshot.
    for d in [0usize, (k / 2) as usize, (k - 1) as usize] {
        let d = d.min(ids.len() - 1);
        let id = &ids[d];
        let tree_ms = timed_reps(5, || repo.tree_at(id).unwrap());
        // A path no delta touched walks all the way back to the keyframe.
        let miss = paths[rng.below(paths.len())].clone();
        let read_ms = timed_reps(50, || repo.read_file_at(id, &miss).unwrap());
        probes.push(json!({"depth": d, "tree_ms": tree_ms, "read_ms": read_ms}));
    }
    json!({
        "op": "real_save", "files": files, "k": k, "saves": saves,
        "seed_save_ms": seed_ms,
        "save_median_ms": quantile(&plain, 0.5),
        "save_p95_ms": quantile(&plain, 0.95),
        "save_max_nonkeyframe_ms": quantile(&plain, 1.0),
        "keyframe_saves": keys.len(),
        "keyframe_ms": {
            "median": quantile(&keys, 0.5), "p90": quantile(&keys, 0.9),
            "p99": quantile(&keys, 0.99), "max": quantile(&keys, 1.0),
        },
        "raw_plain_ms": plain, "raw_keyframe_ms": keys,
        "stage_median_ms": stage_medians(&plain_laps),
        "keyframe_stage_median_ms": stage_medians(&key_laps),
        "disk_bytes_per_snapshot": disk as f64 / saves as f64,
        "probes": probes,
    })
}

#[test]
#[ignore = "measurement: run with --release, see the module docs"]
fn storage_v4_profile() {
    let files = env_usize("VELO_PROFILE_FILES", 100_000);
    let saves = std::env::var("VELO_PROFILE_SAVES")
        .ok()
        .and_then(|v| v.parse().ok());
    let ks: Vec<u32> = std::env::var("VELO_PROFILE_KS")
        .unwrap_or_else(|_| "8,32,128,512".into())
        .split(',')
        .filter_map(|k| k.trim().parse().ok())
        .collect();
    for k in ks {
        emit(&run_k(files, saves, k));
    }
}

/// The same body at a size small enough to run on every `cargo test`.
#[test]
fn storage_v4_profile_smoke() {
    let line = run_k(30, Some(20), 4);
    assert!(line["keyframe_saves"].as_u64().unwrap() >= 2);
    assert!(line["save_median_ms"].as_f64().unwrap() > 0.0);
}
