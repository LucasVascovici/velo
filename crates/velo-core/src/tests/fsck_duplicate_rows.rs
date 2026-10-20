//! `fsck --repair` heals duplicate `file_map` rows left by 5.1.1, and a
//! mismatch on a remote-tracking snapshot is named as such.

use std::fs;
use tempfile::TempDir;

use crate::commands::{self, fsck};
use crate::Repo;

/// The receiving repository's keyframe interval: the default, and a short chain
/// so imported snapshots really are deltas.
const RECEIVER_KS: [u32; 2] = [128, 2];

fn setup() -> (TempDir, Repo, String) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    fs::write(tmp.path().join("a.txt"), "one\n").unwrap();
    fs::write(tmp.path().join("b.txt"), "two\n").unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let id = {
        let guard = repo.write().unwrap();
        commands::save::run(&guard, Some("base"), commands::save::Options::default())
            .unwrap()
            .into_result()
            .expect("snapshot")
            .hash
            .into_string()
    };
    (tmp, repo, id)
}

fn ids_verified(report: &fsck::Report) -> usize {
    report
        .sections
        .iter()
        .find_map(|s| match s {
            fsck::Section::Snapshots { ids_verified, .. } => Some(*ids_verified),
            _ => None,
        })
        .expect("snapshots section")
}

fn rows(repo: &Repo) -> usize {
    repo.conn()
        .query_row("SELECT COUNT(*) FROM file_map", [], |r| r.get::<_, i64>(0))
        .unwrap() as usize
}

fn duplicate_rows(repo: &Repo, id: &str) {
    repo.conn()
        .execute(
            "INSERT INTO file_map SELECT * FROM file_map WHERE snapshot_hash = ?",
            [id],
        )
        .unwrap();
}

#[test]
fn exact_duplicates_are_cruft_not_an_id_mismatch() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        exact_duplicates_are_cruft_not_an_id_mismatch_at_k();
    }
}

fn exact_duplicates_are_cruft_not_an_id_mismatch_at_k() {
    let (_t, repo, id) = setup();
    let clean = fsck::check(&repo).unwrap();
    let n = rows(&repo);
    duplicate_rows(&repo, &id);
    let report = fsck::check(&repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
    assert_eq!(ids_verified(&report), ids_verified(&clean));
    assert!(
        report.cruft.contains(&fsck::Cruft::DuplicateFileMapRows(n)),
        "{:?}",
        report.cruft
    );
}

#[test]
fn repair_removes_duplicates_and_recheck_is_clean() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        repair_removes_duplicates_and_recheck_is_clean_at_k();
    }
}

fn repair_removes_duplicates_and_recheck_is_clean_at_k() {
    let (_t, repo, id) = setup();
    let before = ids_verified(&fsck::check(&repo).unwrap());
    let n = rows(&repo);
    duplicate_rows(&repo, &id);
    let report = {
        let guard = repo.write().unwrap();
        fsck::repair(&guard).unwrap()
    };
    assert!(report
        .repaired
        .contains(&fsck::Cruft::DuplicateFileMapRows(n)));
    let after = fsck::check(&repo).unwrap();
    assert!(after.is_healthy());
    assert!(after.cruft.is_empty(), "{:?}", after.cruft);
    assert_eq!(ids_verified(&after), before);
    let rows: i64 = repo
        .conn()
        .query_row("SELECT COUNT(*) FROM file_map", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows as usize, n);
    assert_eq!(
        fsck::Cruft::DuplicateFileMapRows(3).describe_repaired(),
        "removed 3 duplicate tree row(s)"
    );
}

#[test]
fn duplicate_path_with_another_hash_is_still_corruption() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        duplicate_path_with_another_hash_is_still_corruption_at_k();
    }
}

fn duplicate_path_with_another_hash_is_still_corruption_at_k() {
    let (_t, repo, id) = setup();
    repo.conn()
        .execute(
            "INSERT INTO file_map SELECT snapshot_hash, path, ?, mode FROM file_map \
             WHERE snapshot_hash = ? AND path = 'a.txt'",
            [&"f".repeat(64), &id],
        )
        .unwrap();
    let report = fsck::check(&repo).unwrap();
    assert!(report
        .problems
        .iter()
        .any(|p| matches!(p, fsck::Problem::IdMismatch { .. })));
    let guard = repo.write().unwrap();
    let fixed = fsck::repair(&guard).unwrap();
    assert!(!fixed.is_healthy());
}

#[test]
fn remote_tracking_mismatch_is_named_as_such() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        remote_tracking_mismatch_is_named_as_such_at_k();
    }
}

fn remote_tracking_mismatch_is_named_as_such_at_k() {
    let (_t, repo, id) = setup();
    repo.conn()
        .execute(
            "UPDATE file_map SET hash = ? WHERE snapshot_hash = ? AND path = 'a.txt'",
            [&"e".repeat(64), &id],
        )
        .unwrap();
    let own = fsck::check(&repo).unwrap();
    assert!(own.problems.iter().all(|p| !p.is_remote_tracking()));
    assert!(!own.problems[0].to_string().contains("re-fetch"));

    repo.conn()
        .execute(
            "UPDATE snapshots SET branch = 'remotes/origin/x' WHERE hash = ?",
            [&id],
        )
        .unwrap();
    let report = fsck::check(&repo).unwrap();
    let p = report
        .problems
        .iter()
        .find(|p| matches!(p, fsck::Problem::IdMismatch { .. }))
        .expect("mismatch");
    assert!(p.is_remote_tracking());
    assert!(p.to_string().contains("re-fetch"));
    assert!(!fsck::Problem::DanglingPosition { hash: "x".into() }.is_remote_tracking());
}
