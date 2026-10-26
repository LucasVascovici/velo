//! `velody resolve --mark` refuses an edit that drops the other side's clean
//! changes; `--force` accepts it, and `velody conflicts` shows the gap first.

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

fn write(d: &Path, body: &str) {
    std::fs::write(d.join("r.txt"), body).unwrap();
}

/// One conflict hunk after `stock`, plus a clean `carol` line from `other`,
/// with a hand edit that resolves the hunk but lacks `carol`.
fn carol() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    write(d, "H\nstock\nbulk\nwh\n");
    assert!(velody(d, &["save", "base"]).1);
    assert!(velody(d, &["switch", "other"]).1);
    write(d, "H\ncarol\nstock\nheidi\ncat\nbulk\nwh\n");
    assert!(velody(d, &["save", "other edit"]).1);
    assert!(velody(d, &["switch", "main"]).1);
    write(d, "H\nstock\ncancel\nbulk\nwh\n");
    assert!(velody(d, &["save", "main edit"]).1);
    let (out, _) = velody(d, &["merge", "other"]);
    assert!(out.contains("onflict"), "expected a conflict: {out}");
    write(d, "H\nstock\ncancel\nheidi\ncat\nbulk\nwh\n");
    tmp
}

#[test]
fn mark_refuses_and_names_the_ways_out() {
    let tmp = carol();
    let d = tmp.path();
    let (out, ok) = velody(d, &["resolve", "r.txt", "--mark"]);
    assert!(!ok, "{out}");
    assert!(out.contains("carol"), "{out}");
    assert!(out.contains("--take both --force"), "{out}");
    assert!(out.contains("velody resolve --mark r.txt"), "{out}");
}

#[test]
fn mark_force_accepts_and_reports_the_drop() {
    let tmp = carol();
    let d = tmp.path();
    let (out, ok) = velody(d, &["resolve", "r.txt", "--mark", "--force"]);
    assert!(ok, "{out}");
    assert!(out.contains("dropped 1 line(s)"), "{out}");
    assert!(out.contains("carol"), "{out}");
    assert!(out.contains("All conflicts resolved"), "{out}");
}

#[test]
fn force_alone_is_rejected() {
    let tmp = carol();
    let d = tmp.path();
    let (out, ok) = velody(d, &["resolve", "r.txt", "--force"]);
    assert!(!ok, "{out}");
    assert!(out.contains("--force needs --take or --mark"), "{out}");
}

#[test]
fn conflicts_shows_the_missing_clean_changes() {
    let tmp = carol();
    let d = tmp.path();
    let (out, ok) = velody(d, &["conflicts", "--json"]);
    assert!(ok, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let f = &v["files"][0];
    assert!(f["clean_changes_missing"].as_u64().unwrap() >= 1, "{out}");
    let lines = f["clean_changes_missing_lines"].as_array().unwrap();
    assert!(lines.iter().any(|l| l == "carol"), "{out}");

    let (out, ok) = velody(d, &["conflicts"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("changed cleanly are not in your working file"),
        "{out}"
    );
}
