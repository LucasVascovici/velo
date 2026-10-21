//! An unreachable delta chain whose base is gone is cruft that `repair`
//! clears; a live or trashed snapshot with a missing base is still a problem.

use std::fs;
use tempfile::TempDir;

use crate::commands::{self, fsck};
use crate::Repo;

fn setup() -> (TempDir, Repo, Vec<String>) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let mut repo = Repo::open_and_migrate(tmp.path()).unwrap();
    repo.set_keyframe_interval(128);
    let mut ids = Vec::new();
    for i in 0..3 {
        fs::write(tmp.path().join("a.txt"), format!("edit {i}\n")).unwrap();
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

fn add_link(repo: &Repo, snap: &str, base: &str) {
    exec(
        repo,
        "INSERT INTO tree_deltas (snapshot_hash, base_hash) VALUES (?, ?)",
        &[snap, base],
    );
}

#[test]
fn unreachable_chain_with_missing_base_is_cruft() {
    let (_t, repo, _ids) = setup();
    add_link(&repo, "orphan1", "gone");
    add_link(&repo, "orphan2", "orphan1");
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.cruft, vec![fsck::Cruft::OrphanDeltaRows(2)]);
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    assert!(report.cruft.is_empty(), "{:?}", report.cruft);
    assert!(report.problems.is_empty());
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.is_empty() && report.cruft.is_empty());
}

#[test]
fn live_snapshot_with_missing_base_is_still_a_problem() {
    let (_t, repo, ids) = setup();
    exec(
        &repo,
        "DELETE FROM tree_deltas WHERE snapshot_hash = ?",
        &[&ids[2]],
    );
    add_link(&repo, &ids[2], "gone");
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.iter().any(
        |p| matches!(p, fsck::Problem::DeltaBaseMissing { snapshot, .. } if *snapshot == ids[2])
    ));
}

#[test]
fn trashed_snapshot_with_missing_base_is_still_a_problem() {
    let (_t, repo, _ids) = setup();
    exec(
        &repo,
        "INSERT INTO trash (hash, message, branch) VALUES ('trashed', 'm', 'main')",
        &[],
    );
    add_link(&repo, "trashed", "gone");
    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.iter().any(
        |p| matches!(p, fsck::Problem::DeltaBaseMissing { snapshot, .. } if snapshot == "trashed")
    ));
}
