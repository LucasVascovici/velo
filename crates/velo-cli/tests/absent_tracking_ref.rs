//! After `undo` of a pushed tip, the CLI says the remote still has it, and a
//! merge of the tracking ref points at `velo fetch`.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run `velo <args>` in `dir`; return (stdout, stderr, exit code).
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
fn undo_names_the_tracking_ref_and_merge_says_fetch() {
    let tmp = TempDir::new().unwrap();
    let origin = tmp.path().join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    ok(&origin, &["init"]);
    save(&origin, "a.txt", "base");

    let copy = tmp.path().join("copy");
    ok(tmp.path(), &["clone", origin.to_str().unwrap(), "copy"]);
    save(&copy, "p.txt", "pushed");
    ok(&copy, &["push"]);

    let out = ok(&copy, &["undo"]);
    assert!(out.contains("remotes/origin/main"), "{out}");
    assert!(out.contains("velo fetch"), "{out}");

    save(&copy, "q.txt", "other");
    let (_, err, code) = velo(&copy, &["merge", "origin/main"]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("velo fetch"), "{err}");

    let (_, err, code) = velo(&copy, &["push"]);
    assert_eq!(code, 4, "{err}");
    assert!(err.contains("velo fetch brings it back"), "{err}");
}
