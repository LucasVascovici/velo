//! `fsck` on delta snapshots: ids are verified from the reconstructed tree,
//! and each way the delta storage can be damaged has its own finding.

use std::fs;
use tempfile::TempDir;

use crate::commands::{self, fsck};
use crate::Repo;

/// A repository with `n` saves at keyframe interval `k`; every save edits
/// `a.txt` and adds its own `f<i>.txt`, so a delta has two rows and the added
/// file is never overwritten by a later snapshot.
fn build(k: u32, n: usize) -> (TempDir, Repo, Vec<String>) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let mut repo = Repo::open_and_migrate(tmp.path()).unwrap();
    repo.set_keyframe_interval(k);
    let mut ids = Vec::new();
    for i in 0..n {
        fs::write(tmp.path().join("a.txt"), format!("edit {i}\n")).unwrap();
        fs::write(tmp.path().join(format!("f{i}.txt")), format!("file {i}\n")).unwrap();
        let guard = repo.write().unwrap();
        let id = commands::save::run(&guard, Some("s"), commands::save::Options::default())
            .unwrap()
            .into_result()
            .expect("snapshot")
            .hash
            .into_string();
        ids.push(id);
    }
    (tmp, repo, ids)
}

fn exec(repo: &Repo, sql: &str, params: &[&str]) {
    repo.conn()
        .execute(sql, rusqlite::params_from_iter(params.iter()))
        .unwrap();
}

fn is_delta(repo: &Repo, id: &str) -> bool {
    repo.conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tree_deltas WHERE snapshot_hash = ?)",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

fn mismatched(report: &fsck::Report) -> Vec<String> {
    report
        .problems
        .iter()
        .filter_map(|p| match p {
            fsck::Problem::IdMismatch { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect()
}

fn assert_clean(repo: &Repo) {
    let report = fsck::check(repo).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(report.cruft.is_empty(), "{:?}", report.cruft);
}

fn clean_history(k: u32) {
    let (_t, repo, ids) = build(k, 10);
    assert!(ids.iter().skip(1).any(|i| is_delta(&repo, i)));
    assert_clean(&repo);
    let report = fsck::check(&repo).unwrap();
    let (verified, checked) = report
        .sections
        .iter()
        .find_map(|s| match s {
            fsck::Section::Snapshots {
                ids_verified,
                checked,
                ..
            } => Some((*ids_verified, *checked)),
            _ => None,
        })
        .unwrap();
    assert_eq!(verified, checked);
    assert_eq!(checked, 10);
}

#[test]
fn clean_history_k2() {
    clean_history(2);
}

#[test]
fn clean_history_k3() {
    clean_history(3);
}

#[test]
fn clean_history_k128() {
    clean_history(128);
}

#[test]
fn tampered_delta_row_breaks_the_snapshot_and_its_descendants() {
    let (_t, repo, ids) = build(128, 5);
    assert!(is_delta(&repo, &ids[1]));
    // Point snapshot 1's added file at another real object.
    exec(
        &repo,
        "UPDATE tree_delta_rows SET hash = (SELECT hash FROM tree_delta_rows
              WHERE snapshot_hash = ?1 AND path = 'f2.txt')
          WHERE snapshot_hash = ?2 AND path = 'f1.txt'",
        &[&ids[2], &ids[1]],
    );
    let bad = mismatched(&fsck::check(&repo).unwrap());
    for id in &ids[1..] {
        assert!(bad.contains(id), "{id} not reported: {bad:?}");
    }
    assert!(!bad.contains(&ids[0]));
}

#[test]
fn base_that_is_not_the_first_parent_is_reported() {
    let (_t, repo, ids) = build(128, 4);
    // Snapshot 3's first parent is 2; base it on 1 instead.
    exec(
        &repo,
        "UPDATE tree_deltas SET base_hash = ? WHERE snapshot_hash = ?",
        &[&ids[1], &ids[3]],
    );
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.iter().any(|p| matches!(
        p,
        fsck::Problem::DeltaBaseNotFirstParent { snapshot, base, parent }
            if snapshot == &ids[3] && base == &ids[1] && parent == &ids[2]
    )));
}

#[test]
fn deleted_base_is_reported() {
    let (_t, repo, ids) = build(128, 3);
    exec(&repo, "DELETE FROM snapshots WHERE hash = ?", &[&ids[0]]);
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.iter().any(|p| matches!(
        p,
        fsck::Problem::DeltaBaseMissing { snapshot, base }
            if snapshot == &ids[1] && base == &ids[0]
    )));
}

#[test]
fn two_node_cycle_is_reported() {
    let (_t, repo, ids) = build(128, 3);
    exec(
        &repo,
        "UPDATE tree_deltas SET base_hash = ? WHERE snapshot_hash = ?",
        &[&ids[2], &ids[1]],
    );
    let report = fsck::check(&repo).unwrap();
    for id in &ids[1..] {
        assert!(
            report
                .problems
                .iter()
                .any(|p| matches!(p, fsck::Problem::DeltaCycle { snapshot } if snapshot == id)),
            "{id}: {:?}",
            report.problems
        );
    }
    assert!(!report
        .problems
        .iter()
        .any(|p| matches!(p, fsck::Problem::DeltaCycle { snapshot } if snapshot == &ids[0])));
    // Trees that cannot be built are not also reported as bad ids.
    assert!(mismatched(&report).is_empty());
}

fn storage_mismatches(report: &fsck::Report) -> usize {
    report
        .problems
        .iter()
        .filter(|p| matches!(p, fsck::Problem::StorageMismatch { .. }))
        .count()
}

#[test]
fn stray_keyframe_rows_are_a_storage_mismatch_that_repair_drops() {
    let (_t, repo, ids) = build(128, 3);
    exec(
        &repo,
        "INSERT INTO file_map (snapshot_hash, path, hash, mode)
         SELECT ?1, path, hash, mode FROM file_map WHERE snapshot_hash = ?2",
        &[&ids[1], &ids[0]],
    );
    let report = fsck::check(&repo).unwrap();
    assert_eq!(storage_mismatches(&report), 1);
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    assert_eq!(storage_mismatches(&report), 0);
    assert!(is_delta(&repo, &ids[1]));
    assert_clean(&repo);
}

#[test]
fn repair_keeps_the_keyframe_when_only_it_reproduces_the_id() {
    let (_t, repo, ids) = build(128, 3);
    let last = &ids[2];
    let tree = crate::tree_store::tree_of(repo.conn(), last).unwrap();
    for (path, (hash, mode)) in tree {
        repo.conn()
            .execute(
                "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
                rusqlite::params![last, path, hash, mode],
            )
            .unwrap();
    }
    exec(
        &repo,
        "DELETE FROM tree_delta_rows WHERE snapshot_hash = ?",
        &[last],
    );
    let report = fsck::check(&repo).unwrap();
    assert_eq!(storage_mismatches(&report), 1);
    fsck::repair(&repo.write().unwrap()).unwrap();
    assert!(!is_delta(&repo, last));
    assert_clean(&repo);
}

#[test]
fn repair_leaves_a_storage_mismatch_when_neither_side_verifies() {
    let (_t, repo, ids) = build(128, 3);
    // Keyframe rows that are not the tree, on a delta whose own rows are wrong.
    exec(
        &repo,
        "INSERT INTO file_map (snapshot_hash, path, hash, mode)
         SELECT ?1, path, hash, mode FROM file_map WHERE snapshot_hash = ?2",
        &[&ids[2], &ids[0]],
    );
    exec(
        &repo,
        "DELETE FROM tree_delta_rows WHERE snapshot_hash = ?",
        &[&ids[2]],
    );
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    assert_eq!(storage_mismatches(&report), 1);
}

#[test]
fn orphan_delta_rows_are_cruft_that_repair_removes() {
    let (_t, repo, ids) = build(128, 3);
    exec(
        &repo,
        "INSERT INTO tree_delta_rows (snapshot_hash, path, hash, mode)
         VALUES ('orphan', 'x.txt', NULL, NULL)",
        &[],
    );
    // And a delta snapshot that is in no table and has no dependants.
    exec(
        &repo,
        "INSERT INTO tree_deltas (snapshot_hash, base_hash) VALUES ('orphan2', ?)",
        &[&ids[2]],
    );
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.cruft, vec![fsck::Cruft::OrphanDeltaRows(2)]);
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    assert!(report.cruft.is_empty());
    assert_eq!(report.repaired, vec![fsck::Cruft::OrphanDeltaRows(2)]);
    assert_clean(&repo);
}

#[test]
fn orphan_delta_with_a_dependant_survives_repair() {
    let (_t, repo, ids) = build(128, 3);
    exec(
        &repo,
        "INSERT INTO tree_deltas (snapshot_hash, base_hash) VALUES ('orphan2', ?)",
        &[&ids[2]],
    );
    exec(
        &repo,
        "INSERT INTO tree_deltas (snapshot_hash, base_hash) VALUES ('orphan3', 'orphan2')",
        &[],
    );
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    // Both go: the chain of orphans is removed from the tip down.
    assert!(report.cruft.is_empty(), "{:?}", report.cruft);
}

#[test]
fn overlong_chains_are_cruft_only() {
    let (_t, repo, _ids) = build(200, 150);
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    // Depths 128..=149 are over the 127 a default writer would leave.
    assert_eq!(report.cruft, vec![fsck::Cruft::OverlongChains(22)]);
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    assert!(report.problems.is_empty());
    assert_eq!(report.cruft, vec![fsck::Cruft::OverlongChains(22)]);
}
