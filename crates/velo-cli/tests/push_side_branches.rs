//! `velo push` says when it also published a merged side branch.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn velo(dir: &Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velo binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (out, err, code) = velo(dir, args);
    assert_eq!(code, 0, "velo {args:?} failed: {err}");
    out
}

fn save(dir: &Path, file: &str, msg: &str) {
    std::fs::write(dir.join(file), msg).unwrap();
    ok(dir, &["save", msg]);
}

#[test]
fn push_says_it_published_the_merged_scratch_branch() {
    let tmp = TempDir::new().unwrap();
    let origin = tmp.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    ok(&origin, &["init"]);
    save(&origin, "a.txt", "base");
    let copy = tmp.path().join("copy");
    ok(tmp.path(), &["clone", origin.to_str().unwrap(), "copy"]);

    ok(&copy, &["switch", "attempt-1"]);
    save(&copy, "s.txt", "scratch");
    ok(&copy, &["switch", "main"]);
    save(&copy, "m.txt", "main");
    ok(&copy, &["merge", "attempt-1"]);
    save(&copy, "n.txt", "after");

    let out = ok(&copy, &["push"]);
    assert!(out.contains("also published: attempt-1"), "{out}");
    assert!(out.contains("merged into main"), "{out}");

    save(&copy, "p.txt", "plain");
    let out = ok(&copy, &["push"]);
    assert!(!out.contains("also published"), "{out}");

    let help = ok(&copy, &["push", "--help"]);
    assert!(help.contains("published with"), "{help}");
}
