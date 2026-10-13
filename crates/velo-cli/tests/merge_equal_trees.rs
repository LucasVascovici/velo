//! The recipe for a diverged tip whose trees are equal must end in a push, not
//! loop on "already up to date".

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

/// Two clones that made the same change independently; A has pushed.
fn equal_trees() -> (TempDir, PathBuf, PathBuf) {
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
    write(&a, "x.txt", "same\n");
    ok(&a, &["save", "A made x"]);
    ok(&a, &["push"]);
    write(&b, "x.txt", "same\n");
    ok(&b, &["save", "B made x"]);
    (root, a, b)
}

#[test]
fn the_recipe_ends_with_a_successful_push() {
    let (_root, _a, b) = equal_trees();
    let (_, code) = velo(&b, &["pull"]);
    assert_eq!(code, Some(4));

    let out = ok(&b, &["merge", "origin/main"]);
    assert!(out.contains("trees were already equal"), "{out}");
    assert!(out.contains("Recorded merge"), "{out}");

    let out = ok(&b, &["save", "Merge origin/main"]);
    assert!(out.to_lowercase().contains("nothing to save"), "{out}");
    ok(&b, &["push"]);
}

#[test]
fn merging_an_ancestor_is_up_to_date() {
    let (_root, _a, b) = equal_trees();
    ok(&b, &["merge", "origin/main"]);
    let (out, code) = velo(&b, &["merge", "origin/main"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Already up to date"), "{out}");
}
