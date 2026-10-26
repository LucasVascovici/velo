//! A path restore only answers for unsaved changes under its paths.
//!
//! Run six (team simulation): `restore <snap> --force -- a.txt` with two dirty
//! files printed "Discarding 2 unsaved change(s)" and kept the other file's
//! edit, and without --force it refused over a file it would never touch.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn velody(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(args)
        .current_dir(dir)
        .env("VELODY_AUTHOR_NAME", "test")
        .output()
        .expect("failed to run velody binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
        out.status.success(),
    )
}

/// Two saved files, then both edited. Returns the repo and the saved snapshot.
fn two_dirty_files() -> (TempDir, String) {
    let t = TempDir::new().unwrap();
    let d = t.path();
    assert!(velody(d, &["init"]).1);
    std::fs::write(d.join("a.txt"), "a\n").unwrap();
    std::fs::write(d.join("n.txt"), "n\n").unwrap();
    assert!(velody(d, &["save", "s1"]).1);
    let (hist, _) = velody(d, &["history", "--oneline"]);
    let snap = hist
        .split_whitespace()
        .find(|w| w.len() == 16 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_string();
    std::fs::write(d.join("a.txt"), "a2\n").unwrap();
    std::fs::write(d.join("n.txt"), "n2\n").unwrap();
    (t, snap)
}

#[test]
fn forced_path_restore_counts_only_the_paths_it_restores() {
    let (t, snap) = two_dirty_files();
    let d = t.path();
    let (out, ok) = velody(d, &["restore", &snap, "--force", "--", "a.txt"]);
    assert!(ok, "{out}");
    assert!(out.contains("Discarding 1 unsaved"), "{out}");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "a\n");
    assert_eq!(std::fs::read_to_string(d.join("n.txt")).unwrap(), "n2\n");
}

#[test]
fn path_restore_is_not_refused_over_a_file_outside_the_path() {
    let (t, snap) = two_dirty_files();
    let d = t.path();
    // Put a.txt back by hand so only n.txt, outside the path, is dirty.
    std::fs::write(d.join("a.txt"), "a\n").unwrap();
    let (out, ok) = velody(d, &["restore", &snap, "--", "a.txt"]);
    assert!(ok, "{out}");
    assert_eq!(std::fs::read_to_string(d.join("n.txt")).unwrap(), "n2\n");
}

#[test]
fn path_restore_still_refuses_over_unsaved_changes_in_the_path() {
    let (t, snap) = two_dirty_files();
    let d = t.path();
    let (out, ok) = velody(d, &["restore", &snap, "--", "a.txt"]);
    assert!(!ok, "{out}");
    assert!(out.contains("a.txt"), "{out}");
    assert!(!out.contains("n.txt"), "{out}");
    assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "a2\n");
}
