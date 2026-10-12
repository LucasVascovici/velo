//! `velo resolve --take` refuses to replace hand edits; `--force` replaces them.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn velo(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_velo"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run velo binary");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (s, out.status.success())
}

fn conflicted() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velo(d, &["init"]).1);
    std::fs::write(d.join("f.txt"), "a\n").unwrap();
    assert!(velo(d, &["save", "base"]).1);
    assert!(velo(d, &["switch", "feature"]).1);
    std::fs::write(d.join("f.txt"), "b\n").unwrap();
    assert!(velo(d, &["save", "feature"]).1);
    assert!(velo(d, &["switch", "main"]).1);
    std::fs::write(d.join("f.txt"), "c\n").unwrap();
    assert!(velo(d, &["save", "main"]).1);
    let (out, _) = velo(d, &["merge", "feature"]);
    assert!(out.contains("onflict"), "expected a conflict: {out}");
    tmp
}

#[test]
fn take_refuses_hand_edits_and_force_replaces_them() {
    let tmp = conflicted();
    let d = tmp.path();
    std::fs::write(d.join("f.txt"), "c\nmine\n").unwrap();

    let (out, ok) = velo(d, &["resolve", "f.txt", "--take", "ours"]);
    assert!(!ok, "{out}");
    assert!(out.contains("f.txt"), "{out}");
    assert!(out.contains("--force"), "{out}");
    assert_eq!(
        std::fs::read_to_string(d.join("f.txt")).unwrap(),
        "c\nmine\n"
    );

    let (out, ok) = velo(d, &["resolve", "f.txt", "--take", "theirs", "--force"]);
    assert!(ok, "{out}");
    assert!(
        out.contains("replaced your unsaved edits in f.txt"),
        "{out}"
    );
    assert_eq!(std::fs::read_to_string(d.join("f.txt")).unwrap(), "b\n");
}

#[test]
fn a_clean_take_says_what_ours_means() {
    let tmp = conflicted();
    let d = tmp.path();
    let (out, ok) = velo(d, &["resolve", "f.txt", "--take", "ours"]);
    assert!(ok, "{out}");
    assert!(out.contains("your last saved snapshot"), "{out}");
    assert!(out.contains("discarded 1 line(s) from theirs"), "{out}");
}
