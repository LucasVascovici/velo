//! Filesystem operations per save (19.6-a): what a save costs in `stat`,
//! open-for-read, create, rename and mkdir calls, and the guarantee that a
//! first save opens each new working file once.
//!
//! The counters are process-wide, so a measurement taken while other tests run
//! would include their calls. The asserting test therefore re-runs the
//! measuring test in a child process of its own.

// The report and the child half print on purpose; a test has no other channel.
#![allow(clippy::print_stdout)]

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::commands::{fsck, save, status};
use crate::{timing, Repo};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn body(i: usize) -> String {
    format!("file number {i}\nsome shared text\n")
}

fn write_files(root: &Path, n: usize) {
    for i in 0..n {
        let dir = root.join(format!("d{}", i % 10));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("f{i}.txt")), body(i)).unwrap();
    }
}

fn save_all(repo: &Repo, msg: &str) -> String {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default())
        .unwrap()
        .into_result()
        .expect("a snapshot")
        .hash
        .to_string()
}

type Counts = Vec<(&'static str, u64)>;

fn get(counts: &Counts, name: &str) -> u64 {
    counts.iter().find(|(n, _)| *n == name).map_or(0, |c| c.1)
}

fn report(label: &str, counts: &Counts, files: usize) {
    let mut line = format!("{label:<28}");
    for name in [
        "fs.stat",
        "fs.exists",
        "fs.open_read",
        "fs.create",
        "fs.rename",
        "fs.mkdir",
    ] {
        let n = get(counts, name);
        line.push_str(&format!(
            " {name}={n} ({:.2}/file)",
            n as f64 / files as f64
        ));
    }
    println!("{line}");
}

/// Counters for the first save of `n` new files.
fn first_save_counts(n: usize) -> Counts {
    let (tmp, repo) = setup();
    write_files(tmp.path(), n);
    timing::take_counts();
    save_all(&repo, "first");
    timing::take_counts()
}

#[test]
#[ignore = "prints a report; run with --features timing -- --ignored --nocapture"]
fn fs_ops_per_save() {
    let n = 1000;
    let (tmp, repo) = setup();
    write_files(tmp.path(), n);
    timing::take_counts();
    save_all(&repo, "first");
    report("first save, 1000 new files", &timing::take_counts(), n);

    fs::write(tmp.path().join("d0/f0.txt"), "changed\n").unwrap();
    timing::take_counts();
    save_all(&repo, "one file");
    report("one-file save at 1000", &timing::take_counts(), n);

    timing::take_counts();
    let s = status::run(&repo, &[]).unwrap();
    assert!(s.is_clean());
    report("clean status at 1000", &timing::take_counts(), n);
}

/// Child half of [`first_save_opens_each_new_file_once`]: alone in its
/// process, so the counters hold only this save.
#[test]
#[ignore = "run by first_save_opens_each_new_file_once in its own process"]
fn first_save_counts_child() {
    let counts = first_save_counts(50);
    println!("OPEN_READ={}", get(&counts, "fs.open_read"));
}

#[test]
fn first_save_opens_each_new_file_once() {
    let name = format!(
        "{}::first_save_counts_child",
        module_path!().split_once("::").unwrap().1
    );
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name.as_str(), "--exact", "--ignored", "--nocapture"])
        .args(["--test-threads=1"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let opens: u64 = text
        .lines()
        .find_map(|l| l.split_once("OPEN_READ=").map(|p| p.1))
        .unwrap_or_else(|| panic!("child printed no count: {text}"))
        .trim()
        .parse()
        .unwrap();
    // 50 files plus the `.veloignore` that `init` writes.
    assert!(opens <= 51, "51 new files were opened {opens} times");
}

#[test]
fn first_save_keeps_ids_status_and_a_warm_cache() {
    let (tmp, repo) = setup();
    write_files(tmp.path(), 20);
    fs::write(tmp.path().join("crlf.txt"), "a\r\nb\r\n").unwrap();

    let before = status::run(&repo, &[]).unwrap();
    assert_eq!(before.new_files.len(), 22); // plus .veloignore
    assert!(before.modified.is_empty() && before.deleted.is_empty());
    // Asking twice must not change the answer (the scan no longer caches new
    // files, so this is also the cold path).
    assert_eq!(status::run(&repo, &[]).unwrap().new_files, before.new_files);

    let id = save_all(&repo, "first");

    // The tree holds the true content hashes (CRLF-normalised), and the id
    // commits to it: fsck recomputes every snapshot id.
    let rows: Vec<(String, String)> = repo
        .conn()
        .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ? ORDER BY path")
        .unwrap()
        .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 22);
    let crlf = rows.iter().find(|r| r.0 == "crlf.txt").unwrap();
    assert_eq!(crlf.1, blake3::hash(b"a\nb\n").to_hex().to_string());
    let f3 = rows.iter().find(|r| r.0 == "d3/f3.txt").unwrap();
    assert_eq!(f3.1, blake3::hash(body(3).as_bytes()).to_hex().to_string());
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);

    // Saving seeded the cache, so the next status is clean.
    let after = status::run(&repo, &[]).unwrap();
    assert!(after.is_clean());
    let cached: i64 = repo
        .conn()
        .query_row("SELECT COUNT(*) FROM index_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cached, 22);

    // An edit after the save is still seen.
    fs::write(tmp.path().join("d1/f1.txt"), "edited, longer\n").unwrap();
    assert_eq!(status::run(&repo, &[]).unwrap().modified, ["d1/f1.txt"]);
}
