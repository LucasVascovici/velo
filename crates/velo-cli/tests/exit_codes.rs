//! Scripts decide on the exit code: conflicts, divergence and a held lock each
//! have their own, and "nothing to do" outcomes stay at 0.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Run `velo <args>` in `dir`; return (stdout+stderr, exit code).
fn velo(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .env("VELO_LOCK_WAIT_MS", "3000")
        .output()
        .unwrap();
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.code())
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, code) = velo(dir, args);
    assert_eq!(code, Some(0), "velo {args:?} failed:\n{out}");
    out
}

fn write(dir: &Path, rel: &str, content: &str) {
    std::fs::write(dir.join(rel), content).unwrap();
}

/// A repo whose `feature` branch and `main` both edited `f.txt`.
fn conflicted() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    write(d, "f.txt", "base\n");
    ok(d, &["save", "base"]);
    ok(d, &["switch", "feature"]);
    write(d, "f.txt", "feature\n");
    ok(d, &["save", "feature edit"]);
    ok(d, &["switch", "main"]);
    write(d, "f.txt", "main\n");
    ok(d, &["save", "main edit"]);
    tmp
}

/// An origin plus two clones, A and B, in sync.
fn pair() -> (TempDir, PathBuf, PathBuf) {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    ok(&origin, &["init"]);
    write(&origin, "shared.txt", "base\n");
    ok(&origin, &["save", "C0"]);
    let url = origin.to_str().unwrap();
    ok(root.path(), &["clone", url, "A"]);
    ok(root.path(), &["clone", url, "B"]);
    let a = root.path().join("A");
    let b = root.path().join("B");
    (root, a, b)
}

/// A has pushed; B has committed on top of the old tip, and has fetched.
fn diverged() -> (TempDir, PathBuf, PathBuf) {
    let (root, a, b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    write(&b, "b.txt", "b\n");
    ok(&b, &["save", "B1"]);
    (root, a, b)
}

/// The commands a refusal suggests, in order, one per recipe step.
fn suggested(out: &str) -> Vec<&str> {
    out.lines()
        .filter_map(|l| l.trim_start().strip_prefix("· "))
        .map(|l| l.split("  ").next().unwrap().trim())
        .collect()
}

/// The recipe for a repository that has not fetched the remote's tip.
fn assert_not_fetched_recipe(out: &str) {
    assert_eq!(
        suggested(out),
        [
            "velo sync",
            "velo fetch",
            "velo merge origin/main",
            "velo save \"Merge origin/main\"",
            "velo push"
        ],
        "{out}"
    );
    assert!(!out.contains("velo pull"), "{out}");
}

/// The recipe once the remote's tip is here and not yet merged.
fn assert_fetched_recipe(out: &str) {
    assert_eq!(
        suggested(out),
        [
            "velo sync",
            "velo merge origin/main",
            "velo save \"Merge origin/main\"",
            "velo push"
        ],
        "{out}"
    );
    assert!(!out.contains("velo pull"), "{out}");
}

#[test]
fn conflicted_merge_exits_3_and_still_prints_the_summary() {
    let tmp = conflicted();
    let (out, code) = velo(tmp.path(), &["merge", "feature"]);
    assert_eq!(code, Some(3), "{out}");
    assert!(out.contains("f.txt"), "{out}");
    assert!(out.contains("onflict"), "{out}");
}

#[test]
fn diverged_pull_exits_4_and_prints_the_recipe() {
    let (_root, _a, b) = diverged();
    let (out, code) = velo(&b, &["pull"]);
    assert_eq!(code, Some(4), "{out}");
    assert_fetched_recipe(&out);
}

#[test]
fn refused_push_exits_4_and_leads_with_velo_sync_and_no_false_undo() {
    let (_root, _a, b) = diverged();
    let (out, code) = velo(&b, &["push"]);
    assert_eq!(code, Some(4), "{out}");
    assert_not_fetched_recipe(&out);
    assert!(!out.contains("undone here"), "{out}");
    assert!(!out.contains("undo"), "{out}");
    assert!(out.contains("origin/main is at "), "{out}");
    assert!(out.contains("not fetched yet"), "{out}");
}

#[test]
fn refused_push_mentions_undo_only_when_the_tip_is_in_the_trash() {
    let (_root, _a, b) = pair();
    write(
        &b, "b.txt", "b
",
    );
    ok(&b, &["save", "B1"]);
    ok(&b, &["push"]);
    ok(&b, &["undo"]);
    let (out, code) = velo(&b, &["push"]);
    assert_eq!(code, Some(4), "{out}");
    assert!(out.contains("undone here"), "{out}");
    assert!(!out.contains("not fetched yet"), "{out}");
    assert_eq!(suggested(&out)[0], "velo sync", "{out}");
}

#[test]
fn refused_push_during_a_pending_merge_points_at_conflicts_and_save() {
    let (_root, a, b) = pair();
    write(
        &a,
        "shared.txt",
        "from a
",
    );
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    write(
        &b,
        "shared.txt",
        "from b
",
    );
    ok(&b, &["save", "B1"]);
    ok(&b, &["fetch"]);
    let (out, code) = velo(&b, &["merge", "origin/main"]);
    assert_eq!(code, Some(3), "{out}");
    let (out, code) = velo(&b, &["push"]);
    assert_eq!(code, Some(4), "{out}");
    assert_eq!(
        suggested(&out),
        [
            "velo conflicts",
            "velo save \"Merge origin/main\"",
            "velo push"
        ],
        "{out}"
    );
}

#[test]
fn ahead_only_pull_exits_0() {
    let (_root, a, _b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    let (out, code) = velo(&a, &["pull"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Nothing to pull"), "{out}");
}

#[test]
fn nothing_to_save_and_already_up_to_date_exit_0() {
    let tmp = conflicted();
    let d = tmp.path();
    let (out, code) = velo(d, &["save", "again"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.to_lowercase().contains("nothing to save"), "{out}");
    let (_root, a, _b) = pair();
    let (out, code) = velo(&a, &["pull"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.to_lowercase().contains("already up to date"), "{out}");
}

#[test]
fn locked_repository_exits_5() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(["import-git"])
        .current_dir(d)
        .env("VELO_LOCK_WAIT_MS", "3000")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let holder_file = d.join(".velo").join("lock.holder");
    let start = Instant::now();
    while !holder_file.exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "no lock holder");
        std::thread::sleep(Duration::from_millis(20));
    }
    write(d, "x.txt", "1\n");
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(["save", "x"])
        .current_dir(d)
        .env("VELO_LOCK_WAIT_MS", "100")
        .output()
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(out.status.code(), Some(5));
}

#[test]
fn status_during_a_conflicted_merge_gives_the_merge_next_step() {
    // A diverged clone that has fetched, so status would print the
    // reconcile recipe (starting with `velo pull`) if it were not suppressed.
    let (_root, a, b) = pair();
    write(&a, "shared.txt", "from a\n");
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    write(&b, "shared.txt", "from b\n");
    ok(&b, &["save", "B1"]);
    let (out, code) = velo(&b, &["pull"]);
    assert_eq!(code, Some(4), "{out}");
    let (out, code) = velo(&b, &["merge", "origin/main"]);
    assert_eq!(code, Some(3), "{out}");
    let out = ok(&b, &["status"]);
    assert!(out.contains("to finish merging"), "{out}");
    assert!(!out.contains("velo pull"), "{out}");
}

#[test]
fn tracking_counts_say_as_of_last_fetch() {
    let (_root, a, b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    let out = ok(&a, &["status"]);
    assert!(
        out.contains("1 ahead of origin/main (as of last fetch)"),
        "{out}"
    );
    ok(&a, &["push"]);
    ok(&b, &["fetch"]);
    let out = ok(&b, &["status"]);
    assert!(
        out.contains("1 behind origin/main (as of last fetch)"),
        "{out}"
    );
    write(&b, "b.txt", "b\n");
    ok(&b, &["save", "B1"]);
    let out = ok(&b, &["status"]);
    assert!(out.contains("as of last fetch"), "{out}");
    assert_fetched_recipe(&out);
}

#[test]
fn help_lists_every_exit_code() {
    let tmp = TempDir::new().unwrap();
    let out = ok(tmp.path(), &["--help"]);
    assert!(out.contains("EXIT CODES"), "{out}");
    for code in 0..=5 {
        assert!(
            out.lines()
                .any(|l| l.trim_start().starts_with(&format!("{code} "))),
            "no line for exit code {code}:\n{out}"
        );
    }
}
