//! Squash help and output say the originals are not restorable.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run `velody <args>` in `dir`; return (stdout, stderr, exit code).
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

#[test]
fn squash_output_and_help_say_originals_are_gone() {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    let (_, e, c) = velody(d, &["init"]);
    assert_eq!(c, 0, "{e}");
    for (i, m) in ["a", "b", "c"].iter().enumerate() {
        std::fs::write(d.join("f.txt"), format!("{i}")).unwrap();
        let (_, e, c) = velody(d, &["save", m]);
        assert_eq!(c, 0, "{e}");
    }
    let (out, e, c) = velody(d, &["squash", "2", "x"]);
    assert_eq!(c, 0, "{e}");
    assert!(out.contains("are no longer snapshots"), "{out}");
    assert!(out.contains("it does not restore them"), "{out}");

    let (help, _, c) = velody(d, &["squash", "--help"]);
    assert_eq!(c, 0);
    assert!(!help.contains("remain in the store until"), "{help}");
    assert!(help.contains("does not bring them back"), "{help}");
}
