//! `fsck` time on a v4 repository against the 5.3 storage shape (20.5).
//!
//! Ignored: it is a measurement. K = 1 stores every tree whole, which is the
//! 5.3 shape; larger K stores deltas that fsck must reconstruct. History is
//! built with real `save_tree` calls so every id verifies and the report is
//! healthy before it is timed.
//!
//! Knobs: `VELODY_PROFILE_FILES` (default 1000), `VELODY_PROFILE_SNAPSHOTS`
//! (default 200), `VELODY_PROFILE_K` (default 128), `VELODY_PROFILE_OUT` (append
//! JSON lines to this file). Full envelope, for the owner:
//! `VELODY_PROFILE_FILES=10000 VELODY_PROFILE_SNAPSHOTS=2000 cargo test --release
//! -p velody-core fsck_v4_scale -- --ignored --nocapture`.

use std::io::Write;
use std::time::Instant;

use serde_json::{json, Value};

use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotMeta};

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
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)
            .unwrap();
        writeln!(f, "{text}").unwrap();
    }
}

fn count(repo: &Repo, table: &str) -> i64 {
    repo.conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

/// Build `files` x `snapshots` at keyframe interval `k`, time fsck, return the line.
fn run(files: usize, snapshots: usize, k: u32) -> Value {
    let tmp = tempfile::TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let mut repo = Repo::open_and_migrate(tmp.path()).unwrap();
    repo.set_keyframe_interval(k);
    let branch: BranchName = "main".parse().unwrap();
    let mut entries: Vec<TreeEntry> = (0..files)
        .map(|i| {
            let p = format!("src/d{:03}/f{i}.txt", i % 100);
            TreeEntry::file(p.clone(), format!("file {p}\n").into_bytes())
        })
        .collect();
    let build = Instant::now();
    let mut parent = None;
    for n in 0..snapshots {
        if n > 0 {
            // A few files change per snapshot; the rest are carried forward.
            for j in 0..3 {
                let at = (n * 7919 + j * 104_729) % files;
                let p = entries[at].path.clone();
                entries[at] = TreeEntry::file(p, format!("edit {n} {j}\n").into_bytes());
            }
        }
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: None,
                message: "snap",
                entries: entries.clone(),
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(1 + n as i64),
                renames: &[],
            })
            .unwrap();
        // Carry the stored objects forward so later saves hash nothing new.
        entries = repo
            .tree_at(&id)
            .unwrap()
            .into_iter()
            .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
            .collect();
        parent = Some(id);
    }
    let build_ms = build.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let report = crate::commands::fsck::check(&repo).unwrap();
    let fsck_ms = t.elapsed().as_secs_f64() * 1e3;
    assert!(
        report.is_healthy(),
        "history must verify before it is timed"
    );
    json!({
        "op": "fsck_v4", "files": files, "snapshots": snapshots, "k": k,
        "fsck_ms": fsck_ms, "build_ms": build_ms,
        "file_map_rows": count(&repo, "file_map"),
        "tree_deltas_rows": count(&repo, "tree_deltas"),
        "tree_delta_rows_rows": count(&repo, "tree_delta_rows"),
    })
}

#[test]
#[ignore = "measurement; see the module docs"]
fn fsck_v4_scale() {
    let files = env_usize("VELODY_PROFILE_FILES", 1000);
    let snapshots = env_usize("VELODY_PROFILE_SNAPSHOTS", 200);
    let k = env_usize("VELODY_PROFILE_K", 128) as u32;
    emit(&run(files, snapshots, k));
}

/// The same body at a size small enough to run on every `cargo test`.
#[test]
fn fsck_v4_scale_smoke() {
    let line = run(20, 10, 4);
    assert!(line["fsck_ms"].as_f64().unwrap() >= 0.0);
}
