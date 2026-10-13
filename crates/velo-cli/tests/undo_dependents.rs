//! `velo undo` refuses when another branch builds on the tip, and leaves a clean repo.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run `velo <args>` in `dir`; return (stderr, exit code).
fn velo(dir: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velo binary");
    (
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(dir: &Path, args: &[&str]) {
    let (err, code) = velo(dir, args);
    assert_eq!(code, 0, "velo {args:?} failed: {err}");
}

fn save(dir: &Path, content: &str, msg: &str) {
    std::fs::write(dir.join("f.txt"), content).unwrap();
    ok(dir, &["save", msg]);
}

#[test]
fn undo_refuses_and_fsck_stays_clean() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    save(d, "base", "base");
    ok(d, &["switch", "wip"]);
    save(d, "a", "A");
    ok(d, &["switch", "side"]);
    save(d, "b", "B");
    ok(d, &["switch", "wip"]);

    let (err, code) = velo(d, &["undo"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("is built on by 1 snapshot(s) on side"),
        "{err}"
    );
    assert!(err.contains("1 snapshot(s) on branch 'side'"), "{err}");
    assert!(err.contains("velo switch side"), "{err}");
    assert!(err.contains("velo restore"), "{err}");

    let (err, code) = velo(d, &["fsck"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn undo_refusal_names_a_stash_shelf() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    ok(d, &["init"]);
    save(d, "one", "one");
    save(d, "two", "two");
    std::fs::write(d.join("f.txt"), "dirty").unwrap();
    ok(d, &["stash", "push"]);
    let (err, code) = velo(d, &["undo"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("a stash shelf"), "{err}");
}
