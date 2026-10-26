//! Windows first save against the number of reader threads (20.10-c).
//!
//! Ignored: it is a measurement. On Windows the first save of a large tree is
//! one open per file under Defender (348 to 448 s at 100,000 files). If that
//! cost is per-open latency rather than throughput, a rayon pool with more
//! threads than cores could overlap it. For each thread count the harness
//! writes the files into a FRESH temp directory (so Defender scans every file
//! once, not a warmed cache), initialises a repository and runs the real
//! `commands::save::run` inside a local `rayon::ThreadPool` of that size. The
//! production pool is not touched.
//!
//! ```text
//! VELODY_PROFILE_FILES=10000 VELODY_PROFILE_THREADS=20,40,80 VELODY_PROFILE_OUT=out.json \
//!     cargo test -p velody-core --release first_save_threads \
//!     -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Knobs: `VELODY_PROFILE_FILES` (10000), `VELODY_PROFILE_THREADS` (`20,40,80`),
//! `VELODY_PROFILE_OUT` (append JSON lines).

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use serde_json::json;
use tempfile::TempDir;

use super::perf_support::{project, Layout, Rng};
use crate::{commands, Repo};

#[test]
#[ignore = "measurement; run by hand"]
#[allow(clippy::print_stderr)]
fn first_save_threads() {
    let files: usize = std::env::var("VELODY_PROFILE_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let threads: Vec<usize> = std::env::var("VELODY_PROFILE_THREADS")
        .unwrap_or_else(|_| "20,40,80".into())
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    for n in threads {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let mut rng = Rng::new(7);
        for (rel, bytes) in project(Layout::Scale, files, &mut rng) {
            let p = root.join(&rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
        commands::init::run(root).unwrap();
        let secs = timed_first_save(root, n);
        let line = json!({
            "test": "first_save_threads",
            "files": files,
            "threads": n,
            "first_save_secs": (secs * 1000.0).round() / 1000.0,
        })
        .to_string();
        eprintln!("{line}");
        if let Ok(out) = std::env::var("VELODY_PROFILE_OUT") {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(out)
                .unwrap();
            writeln!(f, "{line}").unwrap();
        }
    }
}

fn timed_first_save(root: &Path, threads: usize) -> f64 {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    // The repository handle is not `Send`, so it is opened inside the pool;
    // opening is a few milliseconds and is not part of the timed region.
    pool.install(|| {
        let repo = Repo::open_and_migrate(root).unwrap();
        let guard = repo.write().unwrap();
        let start = Instant::now();
        commands::save::run(&guard, Some("first"), commands::save::Options::default()).unwrap();
        start.elapsed().as_secs_f64()
    })
}
