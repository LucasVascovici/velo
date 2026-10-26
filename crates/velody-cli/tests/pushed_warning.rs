//! Rewriting history that was already pushed prints a warning, and only a warning.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run `velody <args>` in `dir`; return (stderr, success).
fn velody(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velody binary");
    (
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn ok(dir: &Path, args: &[&str]) {
    let (err, success) = velody(dir, args);
    assert!(success, "velody {args:?} failed: {err}");
}

fn save(dir: &Path, content: &str, msg: &str) {
    std::fs::write(dir.join("f.txt"), content).unwrap();
    ok(dir, &["save", msg]);
}

const WARNING: &str = "already pushed to origin/main";

/// A repo with two snapshots, with `origin` pointing at a second repo.
fn setup() -> (TempDir, TempDir) {
    let remote = TempDir::new().unwrap();
    ok(remote.path(), &["init"]);
    let local = TempDir::new().unwrap();
    ok(local.path(), &["init"]);
    save(local.path(), "one", "one");
    save(local.path(), "two", "two");
    ok(
        local.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    (remote, local)
}

#[test]
fn amend_after_push_warns_but_succeeds() {
    let (_r, local) = setup();
    let d = local.path();
    ok(d, &["push"]);
    std::fs::write(d.join("f.txt"), "three").unwrap();
    let (err, success) = velody(d, &["save", "--amend", "x"]);
    assert!(success, "{err}");
    assert!(err.contains(WARNING), "{err}");
}

#[test]
fn amend_before_push_is_silent() {
    let (_r, local) = setup();
    let d = local.path();
    std::fs::write(d.join("f.txt"), "three").unwrap();
    let (err, success) = velody(d, &["save", "--amend", "x"]);
    assert!(success, "{err}");
    assert!(!err.contains("already pushed"), "{err}");
}

#[test]
fn undo_after_push_warns() {
    let (_r, local) = setup();
    let d = local.path();
    ok(d, &["push"]);
    let (err, success) = velody(d, &["undo"]);
    assert!(success, "{err}");
    assert!(err.contains(WARNING), "{err}");
}

#[test]
fn squashing_only_local_snapshots_is_silent() {
    let (_r, local) = setup();
    let d = local.path();
    ok(d, &["push"]);
    save(d, "three", "three");
    save(d, "four", "four");
    let (err, success) = velody(d, &["squash", "2", "m"]);
    assert!(success, "{err}");
    assert!(!err.contains("already pushed"), "{err}");
}

/// Undoing a local merge removes only the merge snapshot. Its second parent is
/// a pushed side branch, but nothing on that branch is rewritten, so the
/// warning must stay quiet (run six, triage: it named origin/release).
#[test]
fn undoing_a_local_merge_of_a_pushed_branch_is_silent() {
    let (_r, local) = setup();
    let d = local.path();
    ok(d, &["push"]);
    ok(d, &["switch", "--create", "release"]);
    std::fs::write(d.join("g.txt"), "release").unwrap();
    ok(d, &["save", "release work"]);
    ok(d, &["push", "origin", "release"]);
    ok(d, &["switch", "main"]);
    std::fs::write(d.join("h.txt"), "main only").unwrap();
    ok(d, &["save", "local main change"]);
    ok(d, &["fetch"]);
    let (err, _) = velody(d, &["merge", "origin/release"]);
    assert!(!err.contains("error"), "{err}");
    ok(d, &["save", "Merge origin/release"]);
    let (err, success) = velody(d, &["undo"]);
    assert!(success, "{err}");
    assert!(!err.contains("already pushed"), "{err}");
}

/// The same undo after the merge itself was pushed must still warn.
#[test]
fn undoing_a_pushed_merge_warns() {
    let (_r, local) = setup();
    let d = local.path();
    ok(d, &["push"]);
    ok(d, &["switch", "--create", "release"]);
    std::fs::write(d.join("g.txt"), "release").unwrap();
    ok(d, &["save", "release work"]);
    ok(d, &["push", "origin", "release"]);
    ok(d, &["switch", "main"]);
    std::fs::write(d.join("h.txt"), "main only").unwrap();
    ok(d, &["save", "local main change"]);
    ok(d, &["fetch"]);
    velody(d, &["merge", "origin/release"]);
    ok(d, &["save", "Merge origin/release"]);
    ok(d, &["push"]);
    let (err, success) = velody(d, &["undo"]);
    assert!(success, "{err}");
    assert!(err.contains(WARNING), "{err}");
}
