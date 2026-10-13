//! A mutating command waits a bounded time for the write lock, and when it
//! gives up it names the holder and whether that process is still alive.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn velo_cmd(dir: &Path, args: &[&str], wait_ms: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_velo"));
    c.args(args)
        .current_dir(dir)
        .env("VELO_LOCK_WAIT_MS", wait_ms);
    c
}

fn velo(dir: &Path, args: &[&str], wait_ms: &str) -> (String, bool) {
    let out = velo_cmd(dir, args, wait_ms).output().unwrap();
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

/// A `velo import-git` that holds the lock while it blocks reading stdin.
fn holder(dir: &Path) -> Child {
    let child = velo_cmd(dir, &["import-git"], "3000")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let holder_file = dir.join(".velo").join("lock.holder");
    let start = Instant::now();
    while !holder_file.exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "no lock holder");
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

fn repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    assert!(velo(tmp.path(), &["init"], "3000").1);
    tmp
}

#[test]
fn refused_save_names_the_live_holder_then_succeeds_after_it_dies() {
    let tmp = repo();
    let d = tmp.path();
    let mut child = holder(d);
    std::fs::write(d.join("x.txt"), "1\n").unwrap();
    let (out, ok) = velo(d, &["save", "x"], "200");
    assert!(!ok, "{out}");
    assert!(out.contains(&child.id().to_string()), "{out}");
    assert!(out.contains("velo import-git"), "{out}");
    assert!(out.contains("still running"), "{out}");
    child.kill().unwrap();
    child.wait().unwrap();
    let (out, ok) = velo(d, &["save", "x"], "3000");
    assert!(ok, "{out}");
}

#[test]
fn save_waits_out_a_holder_that_dies_within_the_window() {
    let tmp = repo();
    let d = tmp.path().to_path_buf();
    let mut child = holder(&d);
    std::fs::write(d.join("x.txt"), "1\n").unwrap();
    let d2 = d.clone();
    let t = std::thread::spawn(move || velo(&d2, &["save", "x"], "5000"));
    std::thread::sleep(Duration::from_millis(300));
    child.kill().unwrap();
    child.wait().unwrap();
    let (out, ok) = t.join().unwrap();
    assert!(ok, "{out}");
}
