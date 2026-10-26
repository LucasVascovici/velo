//! `velody gc --dry-run` wording and the single `[default: 30]` in its help.

use std::path::Path;
use std::process::Command;

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

#[test]
fn dry_run_says_would_and_removes_nothing() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    std::fs::write(d.join("a.txt"), "one").unwrap();
    assert!(velody(d, &["save", "one"]).1);
    std::fs::write(d.join("a.txt"), "two").unwrap();
    assert!(velody(d, &["save", "two", "--amend"]).1);

    let (out, ok) = velody(d, &["gc", "--dry-run"]);
    assert!(ok, "{out}");
    assert!(out.contains("Would"), "{out}");
    assert!(out.contains("Nothing was removed"), "{out}");
    // The same garbage is still there for a real run.
    let (out, ok) = velody(d, &["gc"]);
    assert!(ok && out.contains("GC complete"), "{out}");
}

#[test]
fn help_shows_the_default_once_and_lists_the_example() {
    let tmp = TempDir::new().unwrap();
    let (out, ok) = velody(tmp.path(), &["help", "gc"]);
    assert!(ok, "{out}");
    assert_eq!(out.matches("[default: 30]").count(), 1, "{out}");
    assert!(out.contains("gc --dry-run"), "{out}");
}
