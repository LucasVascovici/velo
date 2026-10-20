//! `velo repack`, driven through the binary.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use velo_core::tree::{SaveTree, TreeEntry};
use velo_core::{BranchName, Repo, SnapshotMeta};

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

/// A repository of twelve snapshots written at K = 1, so each stores its whole
/// tree, the shape a repository from before deltas has.
fn whole_history() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velo(d, &["init"]).1);
    let mut repo = Repo::open_and_migrate(d).unwrap();
    repo.set_keyframe_interval(1);
    let main: BranchName = "main".parse().unwrap();
    let mut parent = None;
    for i in 0..12 {
        let mut entries: Vec<TreeEntry> = (0..30)
            .map(|f| TreeEntry::file(format!("f{f}.txt"), format!("file {f}\n").into_bytes()))
            .collect();
        entries.push(TreeEntry::file(
            "moving.txt",
            format!("v{i}\n").into_bytes(),
        ));
        let id = repo
            .write_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .save_tree(SaveTree {
                branch: &main,
                parent: parent.as_ref(),
                merge_parent: None,
                message: &format!("step {i}"),
                entries,
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: None,
                renames: &[],
            })
            .unwrap();
        parent = Some(id);
    }
    tmp
}

#[test]
fn dry_run_writes_nothing_then_repack_converts_then_nothing_to_do() {
    let tmp = whole_history();
    let d = tmp.path();
    let db = d.join(".velo/velo.db");

    let before = std::fs::read(&db).unwrap();
    let (out, ok) = velo(d, &["repack", "--dry-run"]);
    assert!(ok, "{out}");
    assert!(out.contains("would save"), "{out}");
    assert!(out.contains("(estimated)"), "{out}");
    assert!(out.contains("convert"), "{out}");
    // A dry run is rolled back, so the file is byte for byte what it was.
    assert_eq!(std::fs::read(&db).unwrap(), before);

    let (out, ok) = velo(d, &["repack"]);
    assert!(ok, "{out}");
    assert!(out.contains("Converted"), "{out}");
    assert!(!out.contains("Converted 0 "), "{out}");

    let (out, ok) = velo(d, &["repack"]);
    assert!(ok, "{out}");
    assert!(out.contains("Nothing to repack"), "{out}");
    let (out, ok) = velo(d, &["repack", "--dry-run"]);
    assert!(ok, "{out}");
    assert!(out.contains("Nothing to repack"), "{out}");

    let (out, ok) = velo(d, &["fsck"]);
    assert!(ok, "{out}");
}
