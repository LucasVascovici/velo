//! `velody init --objects database` keeps objects in velo.db, not in files.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn velody(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_velody"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velody binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, err, code) = velody(dir, args);
    assert_eq!(code, 0, "velody {args:?} failed: {err}");
    out
}

fn file_count(dir: &Path) -> usize {
    walk(dir)
}

fn walk(dir: &Path) -> usize {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| {
                if e.path().is_dir() {
                    walk(&e.path())
                } else {
                    1
                }
            })
            .sum(),
        Err(_) => 0,
    }
}

#[test]
fn database_objects_stay_out_of_the_filesystem() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let out = ok(d, &["init", "--objects", "database"]);
    assert!(out.contains("velo.db"), "{out}");
    std::fs::write(d.join("a.txt"), "hello database").unwrap();
    ok(d, &["save", "first"]);
    assert_eq!(file_count(&d.join(".velo").join("objects")), 0);
    let shown = ok(d, &["show", "main"]);
    assert!(
        shown.contains("a.txt") && shown.contains("hello database"),
        "{shown}"
    );
    ok(d, &["fsck"]);
}

#[test]
fn default_init_still_writes_object_files() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let out = ok(d, &["init"]);
    assert!(!out.contains("no file per object"), "{out}");
    std::fs::write(d.join("a.txt"), "hello files").unwrap();
    ok(d, &["save", "first"]);
    assert!(file_count(&d.join(".velo").join("objects")) > 0);
}

#[test]
fn bad_objects_value_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    let (_, err, code) = velody(tmp.path(), &["init", "--objects", "bogus"]);
    assert_eq!(code, 2, "{err}");
    assert!(!tmp.path().join(".velo").exists());
}
