//! Git habits that fail get one `hint:` line, and `switch` asks about a typo.

use std::path::Path;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn velo(dir: &Path, args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.code())
}

fn repo() -> TempDir {
    let d = TempDir::new().unwrap();
    let (out, code) = velo(d.path(), &["init"]);
    assert_eq!(code, Some(0), "{out}");
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    let (out, code) = velo(d.path(), &["save", "one"]);
    assert_eq!(code, Some(0), "{out}");
    d
}

fn assert_hint(args: &[&str], hint: &str) {
    let d = repo();
    let (out, code) = velo(d.path(), args);
    assert_ne!(code, Some(0), "{args:?}: {out}");
    assert!(
        out.lines()
            .any(|l| l.starts_with("hint: ") && l.contains(hint)),
        "{args:?} lacks hint containing {hint:?}:\n{out}"
    );
}

#[test]
fn habit_hints() {
    assert_hint(&["show", "HEAD~1"], "velo has no HEAD~N");
    assert_hint(&["show", "main:f.txt"], "velo restore <ref> -- <path>");
    assert_hint(&["history", "main"], "velo history --branch <ref>");
    assert_hint(&["restore", "main:f.txt"], "velo restore <ref> -- <path>");
    assert_hint(&["diff", "main:f.txt"], "velo diff <ref> -- <path>");
    assert_hint(&["history", "main:f.txt"], "velo show <ref> -- <path>");
    assert_hint(&["blame", "f.txt", "--at", "main:f.txt"], "--at <ref>");
    assert_hint(&["grep", "-n", "a"], "always shows line numbers");
    assert_hint(&["diff", "--stat"], "no summary mode");
    assert_hint(&["-C", ".", "status"], "cd <dir> first");
}

fn branch_names(d: &Path) -> String {
    velo(d, &["branches"]).0
}

#[test]
fn switch_create_flags() {
    let d = repo();
    let (out, code) = velo(d.path(), &["switch", "-c", "new1"]);
    assert_eq!(code, Some(0), "{out}");
    let (out, code) = velo(d.path(), &["switch", "--create", "new2"]);
    assert_eq!(code, Some(0), "{out}");
    let names = branch_names(d.path());
    assert!(names.contains("new1") && names.contains("new2"), "{names}");
    let (out, code) = velo(d.path(), &["switch", "--create", "new1"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("'new1' already exists"), "{out}");
}

#[test]
fn switch_near_miss_refuses_without_tty() {
    let d = repo();
    let (out, code) = velo(d.path(), &["switch", "mian"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains(
            "no branch 'mian'; did you mean 'main'? To create it: velo switch --create mian"
        ),
        "{out}"
    );
    assert!(!branch_names(d.path()).contains("mian"));
    // --create skips the check.
    let (out, code) = velo(d.path(), &["switch", "--create", "mian"]);
    assert_eq!(code, Some(0), "{out}");
}

#[test]
fn switch_without_near_miss_still_creates() {
    let d = repo();
    let (out, code) = velo(d.path(), &["switch", "brandnew"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(branch_names(d.path()).contains("brandnew"));
}

#[test]
fn help_push_mentions_exit_code_and_sync() {
    let d = TempDir::new().unwrap();
    let (out, code) = velo(d.path(), &["help", "push"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("code 4") && out.contains("velo sync"), "{out}");
}

#[test]
fn stash_push_accepts_dash_m() {
    let d = repo();
    std::fs::write(
        d.path().join("f.txt"),
        "changed
",
    )
    .unwrap();
    let (out, code) = velo(d.path(), &["stash", "push", "-m", "foo"]);
    assert_eq!(code, Some(0), "{out}");
    let (out, _) = velo(d.path(), &["stash", "list"]);
    assert!(out.contains("foo"), "{out}");
}

#[test]
fn show_head_is_the_current_position() {
    let d = repo();
    let (out, code) = velo(d.path(), &["show", "HEAD"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("HEAD means the current position"), "{out}");
}

#[test]
fn switch_new_name_says_it_created_the_branch() {
    let d = repo();
    let (out, code) = velo(d.path(), &["switch", "brandnew2"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Created branch 'brandnew2'"), "{out}");
    assert!(out.contains("--create"), "{out}");
}
