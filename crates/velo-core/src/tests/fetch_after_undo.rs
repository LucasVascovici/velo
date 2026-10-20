//! Importing a pack never duplicates `file_map` rows, even for a snapshot that
//! `undo` moved to the trash and a fetch then brought back.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::{self, fsck, redo, switch, sync, undo};
use crate::transport::Spawn;
use crate::Repo;

/// The receiving repository's keyframe interval: the default, and a short chain
/// so imported snapshots really are deltas.
const RECEIVER_KS: [u32; 2] = [128, 2];

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn save(root: &Path, msg: &str) -> String {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot")
    .hash
    .into_string()
}

fn fetch(root: &Path) {
    with_write(root, |g| {
        sync::fetch(g, "origin", &Spawn::new("velo"), Default::default())
    })
    .unwrap();
}

fn duplicates(root: &Path) -> i64 {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row(
            "SELECT COUNT(*) FROM (SELECT snapshot_hash, path FROM file_map \
             GROUP BY 1,2 HAVING COUNT(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn healthy(root: &Path) -> bool {
    let repo = Repo::open_and_migrate(root).unwrap();
    fsck::check(&repo).unwrap().is_healthy()
}

/// Clone, switch to `br`, save and push it: (holder, origin, clone, pushed id).
fn pushed_branch() -> (TempDir, PathBuf, PathBuf, String) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    fs::write(origin.join("a.txt"), "shared\n").unwrap();
    save(&origin, "base");
    let copy = holder.path().join("copy");
    sync::clone(
        origin.to_str().unwrap(),
        &Spawn::new("velo"),
        sync::CloneOptions {
            dir: Some(copy.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
    with_write(&copy, |g| switch::run(g, "br", false)).unwrap();
    fs::write(copy.join("b.txt"), "on br\n").unwrap();
    let id = save(&copy, "on br");
    with_write(&copy, |g| {
        sync::push(g, "origin", None, &Spawn::new("velo"), Default::default())
    })
    .unwrap();
    (holder, origin, copy, id)
}

#[test]
fn fetch_after_undoing_a_pushed_snapshot_adds_no_duplicate_rows() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        fetch_after_undoing_a_pushed_snapshot_adds_no_duplicate_rows_at_k();
    }
}

fn fetch_after_undoing_a_pushed_snapshot_adds_no_duplicate_rows_at_k() {
    let (_h, _origin, copy, _id) = pushed_branch();
    with_write(&copy, undo::run).unwrap();
    fetch(&copy);
    assert_eq!(duplicates(&copy), 0);
    assert!(healthy(&copy));
}

#[test]
fn redo_after_that_fetch_restores_the_identical_snapshot() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        redo_after_that_fetch_restores_the_identical_snapshot_at_k();
    }
}

fn redo_after_that_fetch_restores_the_identical_snapshot_at_k() {
    let (_h, _origin, copy, id) = pushed_branch();
    with_write(&copy, undo::run).unwrap();
    fetch(&copy);
    let out = with_write(&copy, redo::run).unwrap();
    assert_eq!(out.snapshot, id);
    let repo = Repo::open_and_migrate(&copy).unwrap();
    assert_eq!(
        commands::branch_tip(repo.conn(), "br").as_deref(),
        Some(id.as_str())
    );
    drop(repo);
    assert_eq!(duplicates(&copy), 0);
    assert!(healthy(&copy));
}

#[test]
fn undo_then_a_different_save_then_fetch_adds_no_duplicate_rows() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        undo_then_a_different_save_then_fetch_adds_no_duplicate_rows_at_k();
    }
}

fn undo_then_a_different_save_then_fetch_adds_no_duplicate_rows_at_k() {
    let (_h, _origin, copy, _id) = pushed_branch();
    with_write(&copy, undo::run).unwrap();
    fs::write(copy.join("c.txt"), "other\n").unwrap();
    save(&copy, "something else");
    fetch(&copy);
    assert_eq!(duplicates(&copy), 0);
    assert!(healthy(&copy));
}

#[test]
fn bundle_apply_over_orphaned_rows_leaves_one_row_per_path() {
    for k in RECEIVER_KS {
        crate::repo::set_test_keyframe_interval(k);
        bundle_apply_over_orphaned_rows_leaves_one_row_per_path_at_k();
    }
}

fn bundle_apply_over_orphaned_rows_leaves_one_row_per_path_at_k() {
    let (_h, origin, _copy, _id) = pushed_branch();
    let file = origin.parent().unwrap().join("out.velobundle");
    let src = Repo::open_and_migrate(&origin).unwrap();
    commands::bundle::create(&src, &file, None).unwrap();

    let dest = origin.parent().unwrap().join("dest");
    fs::create_dir_all(&dest).unwrap();
    commands::init::run(&dest).unwrap();
    with_write(&dest, |g| commands::bundle::apply(g, &file)).unwrap();
    // Orphan the rows, and duplicate some for good measure.
    {
        let repo = Repo::open_and_migrate(&dest).unwrap();
        repo.conn()
            .execute_batch("INSERT INTO file_map SELECT * FROM file_map; DELETE FROM snapshots;")
            .unwrap();
    }
    assert!(duplicates(&dest) > 0);
    with_write(&dest, |g| commands::bundle::apply(g, &file)).unwrap();
    assert_eq!(duplicates(&dest), 0);
}
