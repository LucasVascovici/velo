//! The snapshot id check runs on the pool, but the report it produces does not
//! depend on which thread finishes first: every `IdMismatch` is named, in the
//! order the `snapshots` table lists them.

use std::fs;
use tempfile::TempDir;

use crate::commands::{self, fsck};
use crate::Repo;

/// More snapshots than one batch handed to the pool, so several batches run.
const SNAPSHOTS: usize = 150;

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    for i in 0..SNAPSHOTS {
        fs::write(tmp.path().join("a.txt"), format!("version {i}\n")).unwrap();
        fs::write(tmp.path().join(format!("f{}.txt", i % 7)), format!("{i}\n")).unwrap();
        let guard = repo.write().unwrap();
        commands::save::run(
            &guard,
            Some(&format!("save {i}")),
            commands::save::Options::default(),
        )
        .unwrap();
    }
    (tmp, repo)
}

fn mismatches(report: &fsck::Report) -> Vec<String> {
    report
        .problems
        .iter()
        .filter_map(|p| match p {
            fsck::Problem::IdMismatch { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_corrupted_snapshot_is_reported_in_table_order() {
    let (_t, repo) = setup();
    assert!(fsck::check(&repo).unwrap().is_healthy());

    let all: Vec<String> = {
        let mut stmt = repo
            .conn()
            .prepare(
                "SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms
             FROM snapshots",
            )
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(all.len(), SNAPSHOTS);
    // Spread across the table so they land in different batches. Half change
    // the message, half drop a tree row.
    let damaged: Vec<&String> = all.iter().step_by(11).collect();
    assert!(damaged.len() > 10);
    for (n, hash) in damaged.iter().enumerate() {
        if n % 2 == 0 {
            repo.conn()
                .execute(
                    "UPDATE snapshots SET message = 'tampered' WHERE hash = ?",
                    [hash],
                )
                .unwrap();
        } else {
            repo.conn()
                .execute(
                    "DELETE FROM file_map WHERE snapshot_hash = ? AND path = 'a.txt'",
                    [hash],
                )
                .unwrap();
        }
    }

    // The order fsck lists snapshots in, which the report must follow.
    let expected: Vec<String> = all
        .iter()
        .filter(|h| damaged.contains(h))
        .cloned()
        .collect();
    let first = fsck::check(&repo).unwrap();
    assert_eq!(mismatches(&first), expected);
    for _ in 0..3 {
        let again = fsck::check(&repo).unwrap();
        assert_eq!(again.problems, first.problems);
        assert_eq!(again.sections, first.sections);
    }
}
