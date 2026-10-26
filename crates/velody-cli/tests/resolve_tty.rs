//! `velody resolve <file>` without `--take` must refuse at once when stdin or
//! stdout is not a terminal, rather than hang holding the write lock.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn velody(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velody binary");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

fn conflicted() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    std::fs::write(d.join("f.txt"), "a\n").unwrap();
    assert!(velody(d, &["save", "base"]).1);
    assert!(velody(d, &["switch", "feature"]).1);
    std::fs::write(d.join("f.txt"), "b\n").unwrap();
    assert!(velody(d, &["save", "feature"]).1);
    assert!(velody(d, &["switch", "main"]).1);
    std::fs::write(d.join("f.txt"), "c\n").unwrap();
    assert!(velody(d, &["save", "main"]).1);
    let (out, _) = velody(d, &["merge", "feature"]);
    assert!(out.contains("onflict"), "expected a conflict: {out}");
    tmp
}

#[test]
fn interactive_resolve_refuses_without_terminal() {
    let tmp = conflicted();
    let d = tmp.path();
    let before = std::fs::read_to_string(d.join("f.txt")).unwrap();
    let start = Instant::now();
    let (out, ok) = velody(d, &["resolve", "f.txt"]);
    assert!(!ok, "{out}");
    assert!(out.contains("needs a terminal"), "{out}");
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), before);

    // No lock left behind.
    let (out, ok) = velody(d, &["resolve", "f.txt", "--take", "theirs"]);
    assert!(ok, "{out}");
}

#[test]
fn resolve_without_merge_reports_no_merge() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    std::fs::write(d.join("f.txt"), "a\n").unwrap();
    assert!(velody(d, &["save", "base"]).1);
    let (out, ok) = velody(d, &["resolve", "f.txt"]);
    assert!(!ok);
    assert!(out.contains("No merge in progress"), "{out}");
    assert!(!out.contains("needs a terminal"), "{out}");
}

#[test]
fn resolve_non_conflicted_file_is_not_found() {
    let tmp = conflicted();
    let d = tmp.path();
    let (out, ok) = velody(d, &["resolve", "other.txt"]);
    assert!(!ok);
    assert!(!out.contains("needs a terminal"), "{out}");
}
