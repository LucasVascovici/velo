//! `origin/main` and `remotes/origin/main` name the same snapshot everywhere,
//! and `history --all` decorates the remote-tracking tip from `remote_refs`.

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

/// b saves and pushes; c saves, fetches, merges, saves the merge, pushes and
/// fetches again. Returns the holder and clone c.
fn two_clones() -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let origin = root.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    ok(&origin, &["init"]);
    write(&origin, "shared.txt", "base\n");
    ok(&origin, &["save", "C0"]);
    let url = origin.to_str().unwrap();
    ok(root.path(), &["clone", url, "b"]);
    ok(root.path(), &["clone", url, "c"]);
    let (b, c) = (root.path().join("b"), root.path().join("c"));
    write(&b, "b.txt", "b\n");
    ok(&b, &["save", "b work"]);
    ok(&b, &["push"]);
    write(&c, "c.txt", "c\n");
    ok(&c, &["save", "c work"]);
    ok(&c, &["fetch"]);
    ok(&c, &["merge", "origin/main"]);
    ok(&c, &["save", "merge from b"]);
    ok(&c, &["push"]);
    ok(&c, &["fetch"]);
    (root, c)
}

#[test]
fn both_spellings_show_and_diff_alike() {
    let (_root, c) = two_clones();
    let a = ok(&c, &["show", "origin/main"]);
    let b = ok(&c, &["show", "remotes/origin/main"]);
    assert_eq!(a, b);
    assert!(a.contains("merge from b"), "{a}");
    let d = ok(&c, &["diff", "origin/main", "remotes/origin/main"]);
    assert!(!d.contains("+++") && !d.contains("---"), "{d}");
}

#[test]
fn history_all_decorates_the_real_tip_only() {
    let (_root, c) = two_clones();
    let out = ok(&c, &["history", "--all", "--oneline"]);
    let merge = out.lines().find(|l| l.contains("merge from b")).unwrap();
    assert!(merge.contains("remotes/origin/main"), "{out}");
    let old = out.lines().find(|l| l.contains("b work")).unwrap();
    assert!(!old.contains("remotes/origin/main"), "{out}");
}
