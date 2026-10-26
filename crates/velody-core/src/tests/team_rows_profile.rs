//! Profiling harness for the rows of the team run that sit well over their
//! target even on a quiet machine: clone over a path and over HTTP, bundle
//! create, a regex grep, stash push and pop, blame down a long history, push
//! and pull of a batch of snapshots, and several writers saving at once. Phase
//! 21.10-a.
//!
//! The measurement is `#[ignore]`d and runs on demand, in release mode, with
//! the stage timers compiled in, from the repository root. The `http` feature
//! is needed for the HTTP rows; without it they are left out.
//!
//! Windows (PowerShell or Git Bash):
//!
//! ```text
//! cargo test -p velody-core --release --features timing,http team_rows_profile \
//!     -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Linux, or Windows through WSL (use a Linux target directory there so the
//! Windows build is not disturbed):
//!
//! ```text
//! CARGO_TARGET_DIR=$HOME/velody-target cargo test -p velody-core --release \
//!     --features timing,http team_rows_profile -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Each row prints one JSON object to stderr (`op`, `total_ms` median and p95,
//! and the median of every `timing` lap in `stages`) and appends it to the file
//! named by `VELODY_PROFILE_OUT` when that is set. Knobs, with defaults:
//! `VELODY_PROFILE_FILES` (2000), `VELODY_PROFILE_HISTORY` (300),
//! `VELODY_PROFILE_SAMPLES` (3), `VELODY_PROFILE_BATCH` (50, the snapshots moved by
//! one push or pull), `VELODY_PROFILE_DIR` (the system temp directory, so a RAM
//! disk or a slow disk can be chosen) and `VELODY_PROFILE_ROWS`, a comma list
//! that picks rows from: `clone_path`, `clone_http`, `bundle_create`,
//! `grep_regex`, `stash_push_pop`, `blame_deep`, `push_path`, `pull_path`,
//! `push_http`, `pull_http` and `parallel_writers`. The push and pull rows of
//! one transport run together, so picking one of them runs both.
//!
//! The harness changes no library code, so a row can only be timed from
//! outside: the stage laps come from the library's own `timing` timers, which
//! are per thread. The HTTP server is a small in-process front end over the
//! core `Service` (the real one lives in the command line tool), so its own
//! socket handling is part of the HTTP rows. The parallel row gives every
//! thread its own `Repo` on the one repository, never a shared handle; because
//! `Repo::write` refuses a busy lock instead of waiting, each writer retries,
//! and the time it spent doing so is reported as `lock_wait_ms`. A small
//! non-ignored test drives the same code so the harness cannot rot.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use super::perf_support::{build_history, project, Layout, Rng};
use crate::commands::{blame, bundle, grep, restore, save, stash, sync};
use crate::error::VelodyError;
use crate::transport::Spawn;
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

/// The stage timers, when they are compiled in. Without the `timing` feature
/// the timers do not exist, so the harness still runs (and still measures wall
/// time) but reports no stages.
mod laps {
    #[cfg(feature = "timing")]
    pub(super) use crate::timing::{drain_laps as drain, Lap};

    #[cfg(not(feature = "timing"))]
    pub(super) struct Lap {
        pub name: &'static str,
        pub elapsed: std::time::Duration,
    }

    #[cfg(not(feature = "timing"))]
    pub(super) fn drain() -> Vec<Lap> {
        Vec::new()
    }
}

/// The bearer token the in-process server accepts, for reads and pushes alike.
#[cfg(feature = "http")]
const TOKEN: &str = "profile-token";

const HOT: &str = "hot.txt";
const WRITERS: usize = 4;
const SAVES_PER_WRITER: usize = 10;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
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

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

fn median(values: &[f64]) -> f64 {
    quantile(&sorted(values), 0.5)
}

fn p95(values: &[f64]) -> f64 {
    quantile(&sorted(values), 0.95)
}

/// Median per lap name over the samples; a lap that repeats within one sample
/// counts as the sum of its repeats.
#[derive(Default)]
struct Stages(BTreeMap<String, Vec<f64>>);

impl Stages {
    fn add(&mut self, name: &str, millis: f64) {
        self.0.entry(name.to_owned()).or_default().push(millis);
    }

    fn add_laps(&mut self, laps: Vec<laps::Lap>) {
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
    batch: usize,
    dir: Option<PathBuf>,
    rows: Option<Vec<String>>,
}

impl Params {
    fn from_env() -> Self {
        Params {
            files: env_usize("VELODY_PROFILE_FILES", 2000),
            history: env_usize("VELODY_PROFILE_HISTORY", 300),
            samples: env_usize("VELODY_PROFILE_SAMPLES", 3),
            batch: env_usize("VELODY_PROFILE_BATCH", 50),
            dir: std::env::var("VELODY_PROFILE_DIR").ok().map(PathBuf::from),
            rows: std::env::var("VELODY_PROFILE_ROWS")
                .ok()
                .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect()),
        }
    }

    fn wants(&self, row: &str) -> bool {
        self.rows
            .as_ref()
            .is_none_or(|r| r.iter().any(|x| x == row))
    }

    fn temp_dir(&self) -> TempDir {
        match &self.dir {
            Some(p) => TempDir::new_in(p).unwrap(),
            None => TempDir::new().unwrap(),
        }
    }

    fn op(&self, op: &str, totals: &[f64], stages: &Stages) -> Value {
        json!({
            "op": op,
            "files": self.files,
            "history": self.history,
            "batch": self.batch,
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

fn created_at(repo: &Repo, id: &SnapshotId) -> i64 {
    repo.conn()
        .query_row(
            "SELECT created_at_ms FROM snapshots WHERE hash = ?",
            [id.as_str()],
            |r| r.get(0),
        )
        .unwrap()
}

/// Time `sample` once per sample, collecting the wall time and the timer laps
/// of the calling thread. `sample` receives the sample number.
fn measure(p: &Params, op: &str, mut sample: impl FnMut(usize)) -> Value {
    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    for i in 0..p.samples {
        laps::drain();
        let t = Instant::now();
        sample(i);
        totals.push(ms(t.elapsed()));
        stages.add_laps(laps::drain());
    }
    p.op(op, &totals, &stages)
}

/// The repository every row starts from: a tree of `files` files, and
/// `history` snapshots each of which edits [`HOT`] and one other file.
struct Fixture {
    holder: TempDir,
    root: PathBuf,
    repo: Repo,
    tip: SnapshotId,
    rng: Rng,
}

impl Fixture {
    fn build(p: &Params) -> Self {
        let holder = p.temp_dir();
        let root = holder.path().join("origin");
        fs::create_dir_all(&root).unwrap();
        crate::commands::init::run(&root).unwrap();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let mut rng = Rng::new(21);
        for (path, bytes) in project(Layout::Scale, p.files, &mut rng) {
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
        let mut hot = String::from("hot file\n");
        fs::write(root.join(HOT), &hot).unwrap();
        let first = save_now(&repo, "first");

        let base_ms = created_at(&repo, &first);
        let branch = main_branch();
        let mut parent = first;
        for n in 0..p.history {
            hot.push_str(&format!("edit {n} {}\n", rng.next_u64()));
            let mut entries: Vec<TreeEntry> = repo
                .tree_at(&parent)
                .unwrap()
                .into_iter()
                .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
                .collect();
            let other = rng.below(entries.len());
            let other_path = entries[other].path.clone();
            entries[other] = TreeEntry::file(
                other_path,
                format!("other {n} {}\n", rng.next_u64()).into_bytes(),
            );
            let at = entries.iter().position(|e| e.path == HOT).unwrap();
            entries[at] = TreeEntry::file(HOT, hot.clone().into_bytes());
            parent = repo
                .write()
                .unwrap()
                .save_tree(SaveTree {
                    branch: &branch,
                    parent: Some(&parent),
                    merge_parent: None,
                    message: &format!("history {n}"),
                    entries,
                    meta: SnapshotMeta::new(),
                    author: None,
                    timestamp_ms: Some(base_ms + 1 + n as i64),
                    renames: &[],
                })
                .unwrap();
        }
        restore_to(&repo, &parent);
        Fixture {
            holder,
            root,
            repo,
            tip: parent,
            rng,
        }
    }

    fn url(&self) -> String {
        self.root.to_str().unwrap().to_owned()
    }
}

fn save_now(repo: &Repo, message: &str) -> SnapshotId {
    let guard = repo.write().unwrap();
    save::run(&guard, Some(message), save::Options::default())
        .unwrap()
        .into_result()
        .expect("the working tree changed, so a snapshot is expected")
        .hash
}

/// Clone `url` into `dir`, with the token for an HTTP source.
fn clone_into(url: &str, dir: &Path, token: Option<&str>) {
    sync::clone_with_token(
        url,
        &Spawn::new("velody"),
        sync::CloneOptions {
            dir: Some(dir),
            ..Default::default()
        },
        token,
    )
    .unwrap();
}

/// Clone `url` once per sample, keeping only the newest clone on disk.
fn clone_row(p: &Params, fx: &Fixture, op: &str, url: &str, token: Option<&str>) -> Value {
    let mut last: Option<PathBuf> = None;
    let line = measure(p, op, |i| {
        if let Some(old) = last.take() {
            fs::remove_dir_all(old).unwrap();
        }
        let dir = fx.holder.path().join(format!("{op}-{i}"));
        clone_into(url, &dir, token);
        last = Some(dir);
    });
    if let Some(old) = last {
        fs::remove_dir_all(old).unwrap();
    }
    line
}

/// Push and pull of `p.batch` new snapshots over `url`: each sample builds a
/// batch on the origin for the clone to pull, then a batch on the clone for it
/// to push back. Building the batches is not timed.
fn push_pull_rows(
    p: &Params,
    fx: &mut Fixture,
    label: &str,
    url: &str,
    token: Option<&str>,
) -> Vec<Value> {
    let dir = fx.holder.path().join(format!("sync-{label}"));
    clone_into(url, &dir, token);
    let owned = token.map(str::to_owned);
    let mut clone = Repo::open_and_migrate(&dir).unwrap();
    if let Some(t) = owned {
        clone = clone.credentials(move |_| Some(t.clone()));
    }
    let branch = main_branch();
    let spawn = Spawn::new("velody");
    let (mut pull_t, mut pull_s) = (Vec::new(), Stages::default());
    let (mut push_t, mut push_s) = (Vec::new(), Stages::default());
    for _ in 0..p.samples {
        let ids = build_history(&fx.repo, &branch, &fx.tip, p.batch, false, &mut fx.rng);
        fx.tip = ids.last().unwrap().clone();
        laps::drain();
        let t = Instant::now();
        {
            let guard = clone.write().unwrap();
            sync::pull(&guard, "origin", &spawn, Default::default()).unwrap();
        }
        pull_t.push(ms(t.elapsed()));
        pull_s.add_laps(laps::drain());

        let ids = build_history(&clone, &branch, &fx.tip, p.batch, false, &mut fx.rng);
        fx.tip = ids.last().unwrap().clone();
        laps::drain();
        let t = Instant::now();
        {
            let guard = clone.write().unwrap();
            sync::push(&guard, "origin", None, &spawn, Default::default()).unwrap();
        }
        push_t.push(ms(t.elapsed()));
        push_s.add_laps(laps::drain());
        // Put the clone's disk and position back on its tip for the next pull.
        restore_to(&clone, &fx.tip);
    }
    drop(clone);
    fs::remove_dir_all(dir).unwrap();
    // The origin's branch moved without its disk; line the two up again.
    let tip = fx.tip.clone();
    restore_to(&fx.repo, &tip);
    vec![
        p.op(&format!("push_{label}"), &push_t, &push_s),
        p.op(&format!("pull_{label}"), &pull_t, &pull_s),
    ]
}

/// Four threads, each with its own `Repo` on the origin, saving files of their
/// own. `Repo::write` refuses a busy lock, so a writer retries and the time
/// until it got the lock is the lock wait.
fn parallel_row(p: &Params, fx: &Fixture) -> Value {
    let (mut totals, mut stages) = (Vec::new(), Stages::default());
    for sample in 0..p.samples {
        let t = Instant::now();
        let handles: Vec<_> = (0..WRITERS)
            .map(|w| {
                let root = fx.root.clone();
                std::thread::spawn(move || {
                    let repo = Repo::open_and_migrate(&root).unwrap();
                    laps::drain();
                    let (mut wait, mut work) = (Duration::ZERO, Duration::ZERO);
                    let (mut retries, mut made) = (0u64, 0u64);
                    for i in 0..SAVES_PER_WRITER {
                        let rel = format!("writers/w{w}/s{sample}-{i}.txt");
                        fs::create_dir_all(root.join(&rel).parent().unwrap()).unwrap();
                        fs::write(root.join(&rel), format!("writer {w} save {i}\n")).unwrap();
                        let asked = Instant::now();
                        let guard = loop {
                            match repo.write() {
                                Ok(g) => break g,
                                Err(VelodyError::Locked { .. }) => {
                                    retries += 1;
                                    std::thread::sleep(Duration::from_millis(1));
                                }
                                Err(e) => panic!("writer {w}: {e}"),
                            }
                        };
                        wait += asked.elapsed();
                        let started = Instant::now();
                        // Another writer's save may already have taken this file
                        // in, so a save with nothing left to record is fine.
                        let outcome =
                            save::run(&guard, Some("writer"), save::Options::default()).unwrap();
                        made += u64::from(outcome.into_result().is_some());
                        work += started.elapsed();
                    }
                    (wait, work, retries, made, laps::drain())
                })
            })
            .collect();
        let (mut wait, mut work, mut retries, mut made) =
            (Duration::ZERO, Duration::ZERO, 0u64, 0u64);
        let mut laps = Vec::new();
        for h in handles {
            let (w, k, r, m, l) = h.join().unwrap();
            wait += w;
            work += k;
            retries += r;
            made += m;
            laps.extend(l);
        }
        totals.push(ms(t.elapsed()));
        stages.add("lock_wait_ms", ms(wait));
        stages.add("save_ms", ms(work));
        stages.add("lock_retries", retries as f64);
        stages.add("snapshots_created", made as f64);
        stages.add_laps(laps);
    }
    let mut line = p.op("parallel_writers", &totals, &stages);
    line["writers"] = json!(WRITERS);
    line["saves_per_writer"] = json!(SAVES_PER_WRITER);
    line
}

/// A blocking front end over the core `Service`, one thread per connection and
/// `Connection: close` throughout, bound to a free local port.
#[cfg(feature = "http")]
mod server {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use crate::serve::http::{Request, Service, Tokens};

    pub(super) struct Server {
        pub url: String,
        addr: SocketAddr,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Server {
        pub fn start(root: &Path) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let service =
                Arc::new(Service::single(root).authorize_with(Tokens::new().write(super::TOKEN)));
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread = std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if flag.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let service = service.clone();
                    std::thread::spawn(move || {
                        let _ = answer(&service, conn);
                    });
                }
            });
            Server {
                url: format!("http://{addr}/"),
                addr,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Wake the accept loop so it sees the flag.
            let _ = TcpStream::connect(self.addr);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn answer(service: &Service, conn: TcpStream) -> std::io::Result<()> {
        let mut reader = BufReader::new(conn.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_owned();
        let path = parts.next().unwrap_or("").to_owned();
        let (mut length, mut auth) = (0usize, None);
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                let value = value.trim();
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.parse().unwrap_or(0),
                    "authorization" => auth = Some(value.to_owned()),
                    _ => {}
                }
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body)?;
        let resp = service.handle(&Request::new(&method, &path, auth.as_deref(), &body));
        let mut out = conn;
        let challenge = if resp.www_authenticate {
            "WWW-Authenticate: Bearer\r\n"
        } else {
            ""
        };
        write!(
            out,
            "HTTP/1.1 {} Velody\r\nContent-Type: {}\r\nContent-Length: {}\r\n{challenge}Connection: close\r\n\r\n",
            resp.status,
            resp.content_type,
            resp.body.len()
        )?;
        out.write_all(&resp.body)?;
        out.flush()
    }
}

/// Run the selected rows and return the JSON lines they emitted, in order.
fn team_rows(p: &Params) -> Vec<Value> {
    let mut fx = Fixture::build(p);
    let mut out = Vec::new();

    if p.wants("clone_path") {
        out.push(clone_row(p, &fx, "clone_path", &fx.url(), None));
    }
    #[cfg(feature = "http")]
    if p.wants("clone_http") {
        let server = server::Server::start(&fx.root);
        out.push(clone_row(p, &fx, "clone_http", &server.url, Some(TOKEN)));
    }

    if p.wants("bundle_create") {
        let file = fx.holder.path().join("all.bundle");
        out.push(measure(p, "bundle_create", |_| {
            bundle::create(&fx.repo, &file, None).unwrap();
        }));
    }

    if p.wants("grep_regex") {
        out.push(measure(p, "grep_regex", |_| {
            let found = grep::run(&fx.repo, r"edit \d+ [0-9]{3,}", Default::default()).unwrap();
            assert!(!found.is_empty());
        }));
    }

    if p.wants("stash_push_pop") {
        let line = measure(p, "stash_push_pop", |i| {
            // Ten edited files, each with new bytes every sample.
            for f in 0..10.min(p.files) {
                let path = fx.root.join(format!("d000/s0/f{f:06}.txt"));
                fs::write(path, format!("stash edit {i} {f}\n")).unwrap();
            }
            let guard = fx.repo.write().unwrap();
            stash::push(&guard, None).unwrap();
            stash::pop(&guard, None).unwrap();
        });
        out.push(line);
        let tip = fx.tip.clone();
        restore_to(&fx.repo, &tip);
    }

    if p.wants("blame_deep") {
        out.push(measure(p, "blame_deep", |_| {
            let b = blame::run(&fx.repo, Path::new(HOT), Default::default()).unwrap();
            assert!(!b.lines.is_empty());
        }));
    }

    for label in ["path", "http"] {
        if label == "http" && !cfg!(feature = "http") {
            continue;
        }
        if !(p.wants(&format!("push_{label}")) || p.wants(&format!("pull_{label}"))) {
            continue;
        }
        let lines = match label {
            "path" => {
                let url = fx.url();
                push_pull_rows(p, &mut fx, label, &url, None)
            }
            #[cfg(feature = "http")]
            _ => {
                let server = server::Server::start(&fx.root);
                push_pull_rows(p, &mut fx, label, &server.url, Some(TOKEN))
            }
            #[cfg(not(feature = "http"))]
            _ => Vec::new(),
        };
        out.extend(lines);
    }

    if p.wants("parallel_writers") {
        out.push(parallel_row(p, &fx));
    }

    for line in &out {
        emit(line);
    }
    out
}

#[test]
#[ignore = "measurement: run with --release --features timing"]
fn profile_team_rows() {
    team_rows(&Params::from_env());
}

#[test]
fn team_rows_run_at_a_small_size() {
    let lines = team_rows(&Params {
        files: 50,
        history: 10,
        samples: 1,
        batch: 5,
        dir: None,
        rows: None,
    });
    let ops: Vec<&str> = lines.iter().map(|l| l["op"].as_str().unwrap()).collect();
    let mut expected = vec!["clone_path"];
    if cfg!(feature = "http") {
        expected.push("clone_http");
    }
    expected.extend([
        "bundle_create",
        "grep_regex",
        "stash_push_pop",
        "blame_deep",
        "push_path",
        "pull_path",
    ]);
    if cfg!(feature = "http") {
        expected.extend(["push_http", "pull_http"]);
    }
    expected.push("parallel_writers");
    assert_eq!(ops, expected);
    for l in &lines {
        assert!(l["total_ms"]["median"].as_f64().unwrap() > 0.0, "{l}");
    }
    let parallel = lines.last().unwrap();
    assert_eq!(parallel["writers"], json!(WRITERS));
    assert!(parallel["stages"]["lock_wait_ms"].is_number());
    assert!(parallel["stages"]["snapshots_created"].as_f64().unwrap() >= 1.0);
}
