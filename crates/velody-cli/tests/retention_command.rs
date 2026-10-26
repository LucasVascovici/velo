//! `velody retention` and `velody gc --vacuum`, driven through the binary.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use velody_core::commands::{fsck, history, tag};
use velody_core::sign::{self, SigningKey};
use velody_core::tree::{SaveTree, TreeEntry};
use velody_core::{BranchName, Repo, SnapshotId, SnapshotMeta, TagName};

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

const N: usize = 20;

/// A repository with `N` snapshots one minute apart, starting ten days ago,
/// the 6th signed and the 11th tagged, with PARENT at the tip.
fn history() -> (TempDir, Vec<SnapshotId>) {
    let tmp = TempDir::new().unwrap();
    let d = tmp.path();
    assert!(velody(d, &["init"]).1);
    let repo = Repo::open_and_migrate(d).unwrap();
    let main: BranchName = "main".parse().unwrap();
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 10 * 86_400_000;
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..N {
        let id = repo
            .write_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .save_tree(SaveTree {
                branch: &main,
                parent: ids.last(),
                merge_parent: None,
                message: &format!("step {i}"),
                entries: vec![TreeEntry::file("f.txt", format!("body {i}\n").into_bytes())],
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(start + 60_000 * i as i64),
                renames: &[],
            })
            .unwrap();
        ids.push(id);
    }
    {
        // One guard for both mutations, so the second does not race a release.
        let guard = repo
            .write_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        sign::sign(&guard, &ids[5], &SigningKey::from_seed([7; 32])).unwrap();
        let name: TagName = "keepme".parse().unwrap();
        tag::create(&guard, &name, Some(&ids[10]), false).unwrap();
    }
    std::fs::write(d.join(".velo/PARENT"), ids[N - 1].as_str()).unwrap();
    (tmp, ids)
}

fn hashes(d: &Path) -> Vec<SnapshotId> {
    let repo = Repo::open_and_migrate(d).unwrap();
    let main: BranchName = "main".parse().unwrap();
    let h = history::run(
        &repo,
        history::Options {
            branch: Some(&main),
            ..Default::default()
        },
    )
    .unwrap();
    h.entries.into_iter().map(|e| e.hash).collect()
}

fn snapshot_count(d: &Path) -> usize {
    hashes(d).len()
}

#[test]
fn dry_run_lists_protected_and_changes_nothing() {
    let (tmp, ids) = history();
    let d = tmp.path();
    let (out, ok) = velody(d, &["retention", "--dry-run"]);
    assert!(ok, "{out}");
    // Only the snapshots newer than the signed one (index 5) are examined.
    assert!(out.contains(&format!("examined {}", N - 6)), "{out}");
    assert!(out.contains("would remove"), "{out}");
    assert!(out.contains("signed"), "{out}");
    assert!(out.contains("tagged"), "{out}");
    assert!(out.contains(&ids[5].as_str()[..8]), "{out}");
    assert!(out.contains(&ids[10].as_str()[..8]), "{out}");
    assert_eq!(snapshot_count(d), N);
}

#[test]
fn real_run_thins_keeps_protected_and_fsck_is_clean() {
    let (tmp, ids) = history();
    let d = tmp.path();
    let (out, ok) = velody(d, &["retention"]);
    assert!(ok, "{out}");
    assert!(out.contains("Removed"), "{out}");
    assert!(
        out.contains("GC complete") || out.contains("clean"),
        "{out}"
    );
    let after = snapshot_count(d);
    assert!(after < N, "{after}");
    let repo = Repo::open_and_migrate(d).unwrap();
    let left = hashes(d);
    // A tagged snapshot survives under its tag; its id may change when the
    // snapshots below it are squashed.
    let tags = tag::list(&repo).unwrap();
    assert_eq!(tags.len(), 1);
    assert!(tags[0].message.is_some() && hashes(d).contains(&tags[0].snapshot));
    assert!(left.contains(&ids[5]), "the signed snapshot was removed");
    assert!(fsck::check(&repo).unwrap().is_healthy());
    let (out, ok) = velody(d, &["fsck"]);
    assert!(ok, "{out}");
    // A second pass has nothing left to do.
    let (out, ok) = velody(d, &["retention"]);
    assert!(ok && out.contains("Nothing to thin."), "{out}");
}

#[test]
fn keep_tiers_are_parsed_and_bad_values_rejected() {
    let (tmp, _) = history();
    let d = tmp.path();
    let (out, ok) = velody(
        d,
        &[
            "retention",
            "--dry-run",
            "--keep",
            "1d=1h",
            "--keep",
            "30d=1d",
        ],
    );
    assert!(ok, "{out}");
    for bad in ["1x=1h", "1d", "d=1h", "1d=0h", "30d=1d,"] {
        let (out, ok) = velody(d, &["retention", "--dry-run", "--keep", bad]);
        assert!(!ok, "{bad}: {out}");
    }
    let (out, ok) = velody(d, &["retention", "--dry-run", "--keep", "1x=1h"]);
    assert!(!ok && out.contains("1x=1h"), "{out}");
    // Tiers that do not ascend are invalid input from the policy itself.
    let (_, ok) = velody(
        d,
        &[
            "retention",
            "--dry-run",
            "--keep",
            "2d=1h",
            "--keep",
            "1d=2h",
        ],
    );
    assert!(!ok);
    assert_eq!(snapshot_count(d), N);
}

#[test]
fn gc_vacuum_runs_and_rejects_dry_run() {
    let (tmp, _) = history();
    let d = tmp.path();
    let (out, ok) = velody(d, &["gc", "--dry-run", "--vacuum"]);
    assert!(!ok && out.contains("--vacuum"), "{out}");
    let (out, ok) = velody(d, &["gc", "--vacuum"]);
    assert!(ok, "{out}");
    assert!(out.contains("incremental vacuum"), "{out}");
    let (out, ok) = velody(d, &["gc"]);
    assert!(ok, "{out}");
    assert!(velody(d, &["fsck"]).1);
}
