//! `velo sync`: fetch, then fast-forward, push, or merge and push when clean.

use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

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

/// An origin plus two clones, A and B, in sync.
fn pair() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    ok(&origin, &["init"]);
    write(&origin, "shared.txt", "base\n");
    ok(&origin, &["save", "C0"]);
    let url = origin.to_str().unwrap();
    ok(root.path(), &["clone", url, "A"]);
    ok(root.path(), &["clone", url, "B"]);
    let (a, b) = (root.path().join("A"), root.path().join("B"));
    (root, origin, a, b)
}

#[test]
fn nothing_to_do_exits_0_with_plain_text() {
    let (_r, _o, a, _b) = pair();
    let (out, code) = velo(&a, &["sync"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.to_lowercase().contains("already up to date"), "{out}");
}

#[test]
fn ahead_of_the_remote_pushes() {
    let (_r, origin, a, _b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    let out = ok(&a, &["sync"]);
    assert!(out.contains("Pushed"), "{out}");
    assert!(ok(&origin, &["show", "main"]).contains("A1"));
}

#[test]
fn behind_the_remote_fast_forwards_the_working_tree() {
    let (_r, _o, a, b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    let out = ok(&b, &["sync"]);
    assert!(out.contains("Fast-forwarded"), "{out}");
    assert_eq!(std::fs::read_to_string(b.join("a.txt")).unwrap(), "a\n");
}

#[test]
fn diverged_and_clean_merges_and_pushes_a_two_parent_tip() {
    let (_r, origin, a, b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    write(&b, "b.txt", "b\n");
    ok(&b, &["save", "B1"]);
    let out = ok(&b, &["sync"]);
    assert!(out.contains("merged origin/main"), "{out}");
    assert!(out.contains("and pushed"), "{out}");
    assert!(b.join("a.txt").exists() && b.join("b.txt").exists());
    let shown = ok(&origin, &["show", "main"]);
    assert!(shown.contains("parents:"), "{shown}");
    assert!(shown.matches(", ").count() >= 1, "{shown}");
}

#[test]
fn conflicts_stop_with_exit_3_then_resolve_save_sync_finishes() {
    let (_r, origin, a, b) = pair();
    write(&a, "shared.txt", "from a\n");
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    write(&b, "shared.txt", "from b\n");
    ok(&b, &["save", "B1"]);
    let (out, code) = velo(&b, &["sync"]);
    assert_eq!(code, Some(3), "{out}");
    assert!(out.contains("shared.txt"), "{out}");
    assert!(out.contains("velo resolve"), "{out}");
    assert!(b.join(".velo/MERGE_HEAD").exists());
    assert!(
        !ok(&origin, &["show", "main"]).contains("B1"),
        "nothing pushed"
    );

    ok(&b, &["resolve", "shared.txt", "--take", "ours"]);
    ok(&b, &["save", "Merge origin/main"]);
    ok(&b, &["sync"]);
    assert!(ok(&origin, &["show", "main"]).contains("Merge origin/main"));
}

#[test]
fn a_dirty_tree_is_refused() {
    let (_r, _o, a, _b) = pair();
    write(&a, "dirty.txt", "x\n");
    let (out, code) = velo(&a, &["sync"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(!out.is_empty());
}

#[test]
fn refused_push_and_status_suggest_sync() {
    let (_r, _o, a, b) = pair();
    write(&a, "a.txt", "a\n");
    ok(&a, &["save", "A1"]);
    ok(&a, &["push"]);
    ok(&b, &["fetch"]);
    assert!(ok(&b, &["status"]).contains("velo sync"));
    write(&b, "b.txt", "b\n");
    ok(&b, &["save", "B1"]);
    assert!(ok(&b, &["status"]).contains("velo sync"));
    let (out, code) = velo(&b, &["push"]);
    assert_eq!(code, Some(4), "{out}");
    assert!(out.contains("velo sync"), "{out}");
    let (out, code) = velo(&b, &["pull"]);
    assert_eq!(code, Some(4), "{out}");
    assert!(out.contains("velo sync"), "{out}");
}
