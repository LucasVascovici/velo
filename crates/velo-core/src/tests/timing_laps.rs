//! The stage timers record their laps in the documented order.

use std::fs;

use tempfile::TempDir;

use crate::commands::retention::{self, Policy};
use crate::commands::{restore, save};
use crate::timing;
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotMeta};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
const NOW: i64 = 100 * DAY;

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn names() -> Vec<&'static str> {
    timing::drain_laps().into_iter().map(|l| l.name).collect()
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

fn tree(repo: &Repo, parent: Option<&crate::SnapshotId>, ts: i64) -> crate::SnapshotId {
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main_branch(),
            parent,
            merge_parent: None,
            message: "m",
            entries: vec![TreeEntry::file(
                "f.txt",
                format!("body {ts}\n").into_bytes(),
            )],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

#[test]
fn save_tree_records_its_stages() {
    let (_t, repo) = setup();
    timing::drain_laps();
    tree(&repo, None, 1);
    assert_eq!(
        names(),
        [
            "save_tree.validate",
            "save_tree.objects",
            "save_tree.snapshot_id",
            "save_tree.insert_file_map",
            "save_tree.insert_rest",
            "save_tree.commit",
        ]
    );
}

#[test]
fn save_changes_records_its_stages() {
    let (_t, repo) = setup();
    let parent = tree(&repo, None, 1);
    timing::drain_laps();
    let b = main_branch();
    repo.write()
        .unwrap()
        .save_changes(
            crate::tree::SaveChanges::new(&b, Some(&parent), "m")
                .timestamp_ms(2)
                .put(TreeEntry::file(
                    "g.txt",
                    b"g
"
                    .to_vec(),
                )),
        )
        .unwrap();
    assert_eq!(
        names(),
        [
            "save_changes.load_parent",
            "save_changes.apply",
            "save_tree.snapshot_id",
            "save_tree.insert_file_map",
            "save_tree.insert_rest",
            "save_tree.commit",
        ]
    );
}

#[test]
fn save_records_its_stages() {
    let (t, repo) = setup();
    fs::write(t.path().join("a.txt"), "one\n").unwrap();
    timing::drain_laps();
    let g = repo.write().unwrap();
    save::run(&g, Some("first"), save::Options::default()).unwrap();
    assert_eq!(
        names(),
        [
            "save.scan",
            "save.prepare",
            "save.hash_store",
            "save.parent_tree",
            "save.snapshot_id",
            "save.begin_tx",
            "save.insert_file_map",
            "save.insert_rest",
            "save.commit",
            "save.parent_file",
        ]
    );
}

#[test]
fn restore_records_its_stages() {
    let (t, repo) = setup();
    let first = {
        let g = repo.write().unwrap();
        fs::write(t.path().join("a.txt"), "one\n").unwrap();
        let crate::commands::save::Outcome::Saved(r) =
            save::run(&g, Some("first"), save::Options::default()).unwrap()
        else {
            panic!("expected a snapshot");
        };
        fs::write(t.path().join("a.txt"), "two\n").unwrap();
        save::run(&g, Some("second"), save::Options::default()).unwrap();
        r.hash
    };
    timing::drain_laps();
    let g = repo.write().unwrap();
    restore::run(&g, &first, Default::default()).unwrap();
    assert_eq!(
        names(),
        [
            "restore.scan",
            "restore.target_tree",
            "restore.diff",
            "restore.ghosts",
            "restore.validate",
            "restore.write_files",
            "restore.cache_seed",
            "restore.parent_file",
        ]
    );
}

#[test]
fn retention_records_its_stages_then_compactions() {
    let (_t, repo) = setup();
    let mut ids = Vec::new();
    for i in 0..8 {
        let id = tree(&repo, ids.last(), NOW - 10 * DAY + i * HOUR / 4);
        ids.push(id);
    }
    timing::drain_laps();
    let g = repo.write().unwrap();
    let report = retention::apply(
        &g,
        &main_branch(),
        &Policy::default(),
        retention::Options {
            now_ms: Some(NOW),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.removed > 0);
    assert_eq!(
        names(),
        [
            "retention.chain",
            "retention.dependants",
            "retention.classify",
            "compact.chain",
            "compact.eligibility",
            "compact.plan",
            "compact.delete",
            "compact.insert",
            "compact.refs",
            "compact.commit",
        ]
    );
}

#[test]
fn counters_accumulate_and_clear() {
    timing::take_counts();
    timing::count("t.a", 2);
    timing::count("t.a", 3);
    timing::count("t.b", 1);
    let got: Vec<_> = timing::take_counts()
        .into_iter()
        .filter(|(n, _)| n.starts_with("t."))
        .collect();
    assert_eq!(got, [("t.a", 5), ("t.b", 1)]);
    assert!(timing::take_counts()
        .iter()
        .all(|(n, _)| !n.starts_with("t.")));
}
