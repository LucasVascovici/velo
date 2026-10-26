//! Prototype for ARCHITECTURE.md 19.5 option A: snapshots stored as deltas
//! against their parent, with a full keyframe every K snapshots. Phase 19.5-a.
//!
//! Nothing here touches the real schema. The prototype keeps three scratch
//! tables in a real repository's connection (`proto_snap`, `proto_delta`,
//! `proto_key`) and implements only the operations the decision needs:
//! save (`proto_save`) and tree reconstruction (`proto_tree`, `proto_read`).
//! The snapshot id is computed with the real `snapshot_id_of` recipe over the
//! full tree, so identity is unchanged and its cost (reading the parent tree,
//! then hashing it) is measured honestly. Option (d), an id over
//! `(parent id, sorted changes)`, is timed alongside to show what that
//! alternative identity would save.
//!
//! The matrix is `#[ignore]`d and run in release mode:
//!
//! ```text
//! VELODY_PROFILE_FILES=100000 VELODY_PROFILE_OUT=out.json cargo test --release \
//!     -p velody-core delta_prototype_matrix -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Knobs: `VELODY_PROFILE_FILES` (100000), `VELODY_PROFILE_SAVES` (200),
//! `VELODY_PROFILE_KS` (`8,32,128,512`), `VELODY_PROFILE_BASELINE` (20; 0 skips; one-file
//! `save_tree` saves on today's model), `VELODY_PROFILE_DIR`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::perf_support::{project, Layout, Rng};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotMeta};

/// One path changed by a save: `Some((hash, mode))` sets it, `None` deletes it.
type Change = (String, Option<(String, i64)>);
type Row = (String, String, i64);

fn create_scratch(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE proto_snap(id TEXT PRIMARY KEY, parent TEXT, keyframe INTEGER, depth INTEGER);
         CREATE TABLE proto_delta(snapshot TEXT, path TEXT, hash TEXT, mode INTEGER, deleted INTEGER,
             PRIMARY KEY(snapshot, path)) WITHOUT ROWID;
         CREATE TABLE proto_key(snapshot TEXT, path TEXT, hash TEXT, mode INTEGER,
             PRIMARY KEY(snapshot, path)) WITHOUT ROWID;",
    )
    .unwrap();
}

/// The nearest keyframe plus the deltas after it, applied oldest first.
fn proto_tree(conn: &Connection, id: &str) -> Vec<Row> {
    let mut chain = Vec::new();
    let mut at = id.to_owned();
    loop {
        let (parent, keyframe): (String, i64) = conn
            .prepare_cached("SELECT parent, keyframe FROM proto_snap WHERE id = ?")
            .unwrap()
            .query_row([&at], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        chain.push(at.clone());
        if keyframe == 1 {
            break;
        }
        at = parent;
    }
    let mut tree: BTreeMap<String, (String, i64)> = BTreeMap::new();
    let key = chain.pop().unwrap();
    let mut stmt = conn
        .prepare_cached("SELECT path, hash, mode FROM proto_key WHERE snapshot = ? ORDER BY path")
        .unwrap();
    for row in stmt
        .query_map([&key], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
    {
        let (p, h, m): Row = row.unwrap();
        tree.insert(p, (h, m));
    }
    let mut stmt = conn
        .prepare_cached("SELECT path, hash, mode, deleted FROM proto_delta WHERE snapshot = ?")
        .unwrap();
    for snap in chain.iter().rev() {
        for row in stmt
            .query_map([snap], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)?))
            })
            .unwrap()
        {
            let (p, h, m, deleted): (String, String, i64, i64) = row.unwrap();
            if deleted == 1 {
                tree.remove(&p);
            } else {
                tree.insert(p, (h, m));
            }
        }
    }
    tree.into_iter().map(|(p, (h, m))| (p, h, m)).collect()
}

/// One path in `id`: walks back through at most K deltas, then the keyframe.
fn proto_read(conn: &Connection, id: &str, path: &str) -> Option<(String, i64)> {
    let mut at = id.to_owned();
    loop {
        let hit: Option<(String, i64, i64)> = conn
            .prepare_cached(
                "SELECT hash, mode, deleted FROM proto_delta WHERE snapshot = ? AND path = ?",
            )
            .unwrap()
            .query_row([&at, path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .unwrap();
        if let Some((h, m, deleted)) = hit {
            return if deleted == 1 { None } else { Some((h, m)) };
        }
        let (parent, keyframe): (String, i64) = conn
            .prepare_cached("SELECT parent, keyframe FROM proto_snap WHERE id = ?")
            .unwrap()
            .query_row([&at], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        if keyframe == 1 {
            return conn
                .prepare_cached("SELECT hash, mode FROM proto_key WHERE snapshot = ? AND path = ?")
                .unwrap()
                .query_row([&at, path], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()
                .unwrap();
        }
        at = parent;
    }
}

/// The laps of one prototype save, plus the timed-only option (d).
#[derive(Default, Clone, Copy)]
struct Laps {
    parent_tree: f64,
    id: f64,
    write: f64,
    commit: f64,
    id_over_changes: f64,
    keyframe: bool,
}

impl Laps {
    fn total(&self) -> f64 {
        self.parent_tree + self.id + self.write + self.commit
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Save `changes` on top of `parent` (`None` for the first snapshot, which is
/// always a keyframe). Returns the new id and its laps.
fn proto_save(
    conn: &Connection,
    parent: Option<&str>,
    changes: &[Change],
    k: i64,
    message: &str,
    timestamp_ms: i64,
) -> (String, Laps) {
    let mut laps = Laps::default();

    // (a) the parent's whole tree, in memory.
    let t = Instant::now();
    let (parent_tree, depth) = match parent {
        Some(p) => {
            let depth: i64 = conn
                .query_row("SELECT depth FROM proto_snap WHERE id = ?", [p], |r| {
                    r.get(0)
                })
                .unwrap();
            (proto_tree(conn, p), depth + 1)
        }
        None => (Vec::new(), 0),
    };
    laps.parent_tree = ms(t.elapsed());

    // The id, with the real recipe over the full new tree.
    let t = Instant::now();
    let mut tree: BTreeMap<&str, (&str, i64)> = parent_tree
        .iter()
        .map(|(p, h, m)| (p.as_str(), (h.as_str(), *m)))
        .collect();
    for (path, change) in changes {
        match change {
            Some((h, m)) => {
                tree.insert(path.as_str(), (h.as_str(), *m));
            }
            None => {
                tree.remove(path.as_str());
            }
        }
    }
    let id = crate::commands::snapshot_id_of(
        tree.iter().map(|(p, (h, m))| (*p, *h, *m)),
        parent.unwrap_or(""),
        "",
        message,
        timestamp_ms,
        &SnapshotMeta::new(),
    );
    laps.id = ms(t.elapsed());

    // Option (d): an id over (parent id, sorted changes). Timed only.
    let t = Instant::now();
    let mut sorted: Vec<&Change> = changes.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = blake3::Hasher::new();
    h.update(b"velody-snapshot-delta-proto\n");
    h.update(parent.unwrap_or("").as_bytes());
    for (path, change) in sorted {
        h.update(path.as_bytes());
        h.update(b"\0");
        match change {
            Some((hash, mode)) => {
                h.update(hash.as_bytes());
                h.update(b"\0");
                h.update(mode.to_string().as_bytes());
            }
            None => {
                h.update(b"deleted");
            }
        }
        h.update(b"\n");
    }
    h.update(message.as_bytes());
    h.update(timestamp_ms.to_string().as_bytes());
    std::hint::black_box(h.finalize());
    laps.id_over_changes = ms(t.elapsed());

    // (b) the rows: a delta, or the full tree on a keyframe.
    let keyframe = depth % k == 0;
    laps.keyframe = keyframe;
    let t = Instant::now();
    conn.execute_batch("BEGIN").unwrap();
    conn.execute(
        "INSERT INTO proto_snap VALUES (?, ?, ?, ?)",
        params![id, parent.unwrap_or(""), keyframe as i64, depth],
    )
    .unwrap();
    if keyframe {
        let mut ins = conn
            .prepare_cached("INSERT INTO proto_key VALUES (?, ?, ?, ?)")
            .unwrap();
        for (path, (hash, mode)) in &tree {
            ins.execute(params![id, path, hash, mode]).unwrap();
        }
    } else {
        let mut ins = conn
            .prepare_cached("INSERT INTO proto_delta VALUES (?, ?, ?, ?, ?)")
            .unwrap();
        for (path, change) in changes {
            match change {
                Some((hash, mode)) => ins.execute(params![id, path, hash, mode, 0]),
                None => ins.execute(params![id, path, "", 0, 1]),
            }
            .unwrap();
        }
    }
    laps.write = ms(t.elapsed());
    let t = Instant::now();
    conn.execute_batch("COMMIT").unwrap();
    laps.commit = ms(t.elapsed());
    (id, laps)
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

fn median(values: &[f64]) -> f64 {
    quantile(values, 0.5)
}

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
    if let Ok(out) = std::env::var("VELODY_PROFILE_OUT") {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)
            .unwrap();
        writeln!(f, "{text}").unwrap();
    }
}

fn temp_repo() -> (TempDir, Repo) {
    let tmp = match std::env::var("VELODY_PROFILE_DIR") {
        Ok(p) => TempDir::new_in(PathBuf::from(p)).unwrap(),
        Err(_) => TempDir::new().unwrap(),
    };
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn db_bytes(conn: &Connection) -> i64 {
    let pages: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    let size: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .unwrap();
    pages * size
}

fn paths_of(files: usize) -> Vec<String> {
    let mut rng = Rng::new(1);
    let mut p: Vec<String> = project(Layout::Scale, files, &mut rng)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    p.sort();
    p
}

fn fake_hash(n: u64) -> String {
    blake3::hash(&n.to_le_bytes()).to_hex().to_string()
}

fn timed_reps<T>(reps: usize, mut f: impl FnMut() -> T) -> f64 {
    let mut v = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        std::hint::black_box(f());
        v.push(ms(t.elapsed()));
    }
    median(&v)
}

/// One K: seed a keyframe of `files` rows, run the saves, then probe reads.
fn run_k(files: usize, saves: usize, k: i64) -> Value {
    let (_tmp, repo) = temp_repo();
    let conn = repo.conn();
    create_scratch(conn);
    let paths = paths_of(files);
    let mut rng = Rng::new(7);

    // Seed: the depth-0 keyframe (not measured).
    let seed_changes: Vec<Change> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.clone(), Some((fake_hash(i as u64), 0))))
        .collect();
    let (root, _) = proto_save(conn, None, &seed_changes, k, "seed", 0);

    // Enough saves to land at least one keyframe after the seed.
    let saves = saves.max(k as usize + 16);
    let before = db_bytes(conn);
    let mut ids = vec![root];
    let mut all: Vec<Laps> = Vec::new();
    for n in 0..saves {
        let path = paths[rng.below(paths.len())].clone();
        let change = vec![(path, Some((fake_hash(1_000_000 + n as u64), 0)))];
        let (id, laps) = proto_save(
            conn,
            ids.last().map(String::as_str),
            &change,
            k,
            "s",
            1 + n as i64,
        );
        ids.push(id);
        all.push(laps);
    }
    let disk = db_bytes(conn) - before;

    let plain: Vec<&Laps> = all.iter().filter(|l| !l.keyframe).collect();
    let keys: Vec<&Laps> = all.iter().filter(|l| l.keyframe).collect();
    let totals: Vec<f64> = plain.iter().map(|l| l.total()).collect();
    let key_totals: Vec<f64> = keys.iter().map(|l| l.total()).collect();
    let med = |f: fn(&Laps) -> f64| median(&plain.iter().map(|l| f(l)).collect::<Vec<_>>());
    let parent_tree = med(|l| l.parent_tree);
    let id_ms = med(|l| l.id);

    // Read latency at the depths the brief names: 0, K/2 and K-1 within the
    // first cycle (ids[d] is the snapshot at depth d).
    let mut probes = Vec::new();
    for d in [0usize, (k / 2) as usize, (k - 1) as usize] {
        let id = &ids[d];
        let tree_ms = timed_reps(5, || proto_tree(conn, id));
        // A path that no delta touched walks all the way back to the keyframe.
        let miss = &paths[rng.below(paths.len())];
        let read_ms = timed_reps(50, || proto_read(conn, id, miss));
        probes.push(json!({"depth": d, "tree_ms": tree_ms, "read_ms": read_ms}));
    }
    json!({
        "op": "proto_save", "files": files, "k": k, "saves": saves,
        "save_median_ms": median(&totals),
        "save_p95_ms": quantile(&totals, 0.95),
        "save_max_nonkeyframe_ms": totals.iter().cloned().fold(0.0, f64::max),
        "keyframe_saves": keys.len(),
        "keyframe_save_ms": key_totals.iter().cloned().fold(0.0, f64::max),
        "stage_median_ms": {
            "proto.parent_tree": parent_tree, "proto.id": id_ms,
            "proto.write": med(|l| l.write), "proto.commit": med(|l| l.commit),
        },
        "identity_alone_ms": parent_tree + id_ms,
        "id_over_changes_ms": med(|l| l.id_over_changes),
        "disk_bytes_per_snapshot": disk as f64 / saves as f64,
        "probes": probes,
    })
}

/// Today's model at the same size: a one-file `save_tree` from the parent's
/// tree, plus `tree_at` and `read_file_at` for comparison.
fn run_today(files: usize, samples: usize) -> Value {
    let (_tmp, repo) = temp_repo();
    let paths = paths_of(files);
    let branch: BranchName = "main".parse().unwrap();
    let entries: Vec<TreeEntry> = paths
        .iter()
        .map(|p| TreeEntry::file(p.clone(), format!("file {p}\n").into_bytes()))
        .collect();
    let mut parent = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "first",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap();
    let mut rng = Rng::new(3);
    let before = db_bytes(repo.conn());
    let (mut totals, mut tree_at) = (Vec::new(), Vec::new());
    for n in 0..samples {
        let t = Instant::now();
        let tree = repo.tree_at(&parent).unwrap();
        tree_at.push(ms(t.elapsed()));
        let mut entries: Vec<TreeEntry> = tree
            .into_iter()
            .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
            .collect();
        let at = rng.below(entries.len());
        let path = entries[at].path.clone();
        entries[at] = TreeEntry::file(path, format!("edit {n} {}\n", rng.next_u64()).into_bytes());
        parent = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent: Some(&parent),
                merge_parent: None,
                message: "edit",
                entries,
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(2 + n as i64),
                renames: &[],
            })
            .unwrap();
        totals.push(ms(t.elapsed()));
    }
    let disk = db_bytes(repo.conn()) - before;
    let read_ms = timed_reps(50, || {
        repo.read_file_at(&parent, &paths[rng.below(paths.len())])
            .unwrap()
    });
    json!({
        "op": "today_save_tree", "files": files, "samples": samples,
        "save_median_ms": median(&totals), "save_p95_ms": quantile(&totals, 0.95),
        "tree_at_ms": median(&tree_at), "read_file_at_ms": read_ms,
        "disk_bytes_per_snapshot": disk as f64 / samples as f64,
    })
}

/// Rows a one-file change rewrites under option B (a tree object per
/// directory): the files in the changed file's directory plus one entry per
/// ancestor directory above it. Returns the figure for the largest directory,
/// which is what a flat layout makes the common case.
fn option_b_rows(paths: &[String]) -> usize {
    let mut per_dir: BTreeMap<&str, usize> = BTreeMap::new();
    for p in paths {
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
        *per_dir.entry(dir).or_default() += 1;
    }
    per_dir
        .iter()
        .map(|(dir, files)| {
            let ancestors = if dir.is_empty() {
                0
            } else {
                dir.matches('/').count() + 1
            };
            files + ancestors
        })
        .max()
        .unwrap_or(0)
}

fn option_b_lines(files: usize) -> Vec<Value> {
    let mut rng = Rng::new(1);
    let mut names = |layout: Layout, n: usize| -> Vec<String> {
        project(layout, n, &mut rng)
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    };
    let agent_small = names(Layout::Agent, 1000);
    let agent_big = names(Layout::Agent, files);
    let scale = names(Layout::Scale, files);
    let flat: Vec<String> = (0..files).map(|i| format!("f{i:06}.txt")).collect();
    [
        ("agent_benchmark", agent_small),
        ("agent_layout_at_size", agent_big),
        ("scale_layout", scale),
        ("flat_directory", flat),
    ]
    .into_iter()
    .map(|(layout, paths)| {
        json!({"op": "option_b_rows", "layout": layout, "files": paths.len(),
               "rows_rewritten_per_one_file_change": option_b_rows(&paths),
               "rows_option_a_delta": 1})
    })
    .collect()
}

#[test]
#[ignore = "measurement: run with --release"]
fn delta_prototype_matrix() {
    let files = env_usize("VELODY_PROFILE_FILES", 100_000);
    let saves = env_usize("VELODY_PROFILE_SAVES", 200);
    let ks: Vec<i64> = std::env::var("VELODY_PROFILE_KS")
        .unwrap_or_else(|_| "8,32,128,512".into())
        .split(',')
        .filter_map(|k| k.trim().parse().ok())
        .collect();
    // A baseline of 0 skips today's model and the option B counts, so the
    // matrix can be split across runs one K at a time without repeating them.
    let baseline = env_usize("VELODY_PROFILE_BASELINE", 20);
    if baseline > 0 {
        emit(&run_today(files, baseline));
        for line in option_b_lines(files) {
            emit(&line);
        }
    }
    for k in ks {
        emit(&run_k(files, saves, k));
    }
}

fn object_of(repo: &Repo, id: &crate::SnapshotId, path: &str) -> String {
    repo.object_at(id, path).unwrap().unwrap().to_string()
}

/// The prototype is only worth measuring if it is right: over a 30-snapshot
/// history with adds, edits and deletes, `proto_tree` equals `tree_at`, the
/// prototype id equals the real id, and `proto_read` agrees, for every K.
#[test]
fn proto_tree_reproduces_tree_at_for_every_k() {
    for k in [1i64, 4, 16] {
        let (_tmp, repo) = temp_repo();
        create_scratch(repo.conn());
        let branch: BranchName = "main".parse().unwrap();
        let mut rng = Rng::new(11);
        let mut current: BTreeMap<String, String> = (0..12)
            .map(|i| (format!("d{}/f{i}.txt", i % 3), format!("start {i}\n")))
            .collect();
        let mut real_parent = None;
        let mut proto_parent: Option<String> = None;
        let mut pairs = Vec::new();
        for n in 0..30i64 {
            let path = format!("d{}/f{}.txt", rng.below(3), rng.below(16));
            let edit: (String, Option<String>) =
                if rng.below(3) == 0 && current.len() > 1 && current.contains_key(&path) {
                    (path, None)
                } else {
                    (path, Some(format!("v{n} {}\n", rng.next_u64())))
                };
            match &edit.1 {
                Some(c) => current.insert(edit.0.clone(), c.clone()),
                None => current.remove(&edit.0),
            };
            let entries: Vec<TreeEntry> = current
                .iter()
                .map(|(p, c)| TreeEntry::file(p.clone(), c.clone().into_bytes()))
                .collect();
            let id = repo
                .write()
                .unwrap()
                .save_tree(SaveTree {
                    branch: &branch,
                    parent: real_parent.as_ref(),
                    merge_parent: None,
                    message: "m",
                    entries,
                    meta: SnapshotMeta::new(),
                    author: None,
                    timestamp_ms: Some(100 + n),
                    renames: &[],
                })
                .unwrap();
            let changes: Vec<Change> = if n == 0 {
                current
                    .keys()
                    .map(|p| (p.clone(), Some((object_of(&repo, &id, p), 0))))
                    .collect()
            } else {
                vec![(
                    edit.0.clone(),
                    edit.1.as_ref().map(|_| (object_of(&repo, &id, &edit.0), 0)),
                )]
            };
            let (pid, _) = proto_save(
                repo.conn(),
                proto_parent.as_deref(),
                &changes,
                k,
                "m",
                100 + n,
            );
            assert_eq!(pid, id.as_str(), "K={k} snapshot {n}: ids differ");
            real_parent = Some(id.clone());
            proto_parent = Some(pid.clone());
            pairs.push((id, pid));
        }
        for (real, proto) in &pairs {
            let want: Vec<Row> = repo
                .tree_at(real)
                .unwrap()
                .into_iter()
                .map(|f| (f.path, f.object.to_string(), 0))
                .collect();
            let conn = repo.conn();
            assert_eq!(proto_tree(conn, proto), want, "K={k} {proto}");
            for (p, h, _) in &want {
                assert_eq!(proto_read(conn, proto, p).map(|r| r.0), Some(h.clone()));
            }
            assert_eq!(proto_read(conn, proto, "nope.txt"), None);
        }
    }
}
