//! `resolve --take both`, `resolve --mark`, `velody conflicts`, and help text
//! that no longer promises sidecars.

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

fn conflicted() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    std::fs::write(d.join("f.txt"), "a\n").unwrap();
    assert!(velody(d, &["save", "base"]).1);
    assert!(velody(d, &["switch", "feature"]).1);
    std::fs::write(d.join("f.txt"), "theirs line\n").unwrap();
    assert!(velody(d, &["save", "feature"]).1);
    assert!(velody(d, &["switch", "main"]).1);
    std::fs::write(d.join("f.txt"), "ours line\n").unwrap();
    assert!(velody(d, &["save", "main"]).1);
    let (out, _) = velody(d, &["merge", "feature"]);
    assert!(out.contains("onflict"), "expected a conflict: {out}");
    tmp
}

#[test]
fn take_both_keeps_ours_then_theirs() {
    let tmp = conflicted();
    let d = tmp.path();
    let (out, ok) = velody(d, &["resolve", "f.txt", "--take", "both"]);
    assert!(ok, "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join("f.txt")).unwrap(),
        "ours line\ntheirs line\n"
    );
}

#[test]
fn mark_accepts_a_hand_edit_and_echoes_counts() {
    let tmp = conflicted();
    let d = tmp.path();
    std::fs::write(d.join("f.txt"), "merged by hand\nsecond\n").unwrap();
    let (out, ok) = velody(d, &["resolve", "--mark", "f.txt"]);
    assert!(ok, "{out}");
    assert!(out.contains("Accepted your edit of 'f.txt'"), "{out}");
    assert!(out.contains("2 lines"), "{out}");
    assert!(out.contains("All conflicts resolved"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join("f.txt")).unwrap(),
        "merged by hand\nsecond\n"
    );
}

#[test]
fn mark_refuses_an_untouched_file_and_suggests_take() {
    let tmp = conflicted();
    let d = tmp.path();
    let (out, ok) = velody(d, &["resolve", "--mark", "f.txt"]);
    assert!(!ok, "{out}");
    assert!(out.contains("--take ours"), "{out}");
}

#[test]
fn non_tty_refusal_names_the_alternatives() {
    let tmp = conflicted();
    let (out, ok) = velody(tmp.path(), &["resolve", "f.txt"]);
    assert!(!ok, "{out}");
    assert!(out.contains("--take ours|theirs|both"), "{out}");
    assert!(out.contains("velody resolve --mark <file>"), "{out}");
}

#[test]
fn conflicts_text_shows_both_sides() {
    let tmp = conflicted();
    let (out, ok) = velody(tmp.path(), &["conflicts", "f.txt"]);
    assert!(ok, "{out}");
    assert!(out.contains("hunk 1/1"), "{out}");
    assert!(out.contains("ours line"), "{out}");
    assert!(out.contains("theirs (feature)"), "{out}");
    assert!(out.contains("theirs line"), "{out}");
}

#[test]
fn conflicts_json_parses() {
    let tmp = conflicted();
    let (out, ok) = velody(tmp.path(), &["conflicts", "--json"]);
    assert!(ok, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(v["source"], "feature");
    let h = &v["files"][0]["hunks"][0];
    assert_eq!(v["files"][0]["path"], "f.txt");
    assert!(!h["ours"].as_array().unwrap().is_empty());
    assert!(!h["theirs"].as_array().unwrap().is_empty());
    assert!(h["decision"].is_null());
}

#[test]
fn conflicts_with_no_merge_is_quiet_and_succeeds() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    let (out, ok) = velody(d, &["conflicts"]);
    assert!(ok, "{out}");
    assert!(out.contains("No conflicts."), "{out}");
    let (out, ok) = velody(d, &["conflicts", "--json"]);
    assert!(ok, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["source"].is_null());
    assert!(v["files"].as_array().unwrap().is_empty());
}

#[test]
fn help_no_longer_promises_sidecars() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    for args in [
        &["help", "merge"][..],
        &["help", "resolve"],
        &["help", "cherry-pick"],
        &["--help"],
    ] {
        let (out, _) = velody(d, args);
        assert!(!out.contains(".conflict"), "{args:?}: {out}");
        assert!(!out.contains("diff <file> --conflict"), "{args:?}: {out}");
    }
}
