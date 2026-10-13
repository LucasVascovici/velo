//! A tracking ref at a snapshot this repository does not hold is a typed error,
//! never a merge parent, and `undo` can find the refs that still name its tip.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::sync;
use crate::commands::{self, fsck, merge, undo};
use crate::error::{RefKind, VeloError};
use crate::transport::Spawn;
use crate::Repo;

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn save(root: &Path, file: &str, content: &str, msg: &str) -> String {
    fs::write(root.join(file), content).unwrap();
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot")
    .hash
    .into_string()
}

fn push(root: &Path) -> crate::error::Result<sync::Pushed> {
    with_write(root, |g| {
        sync::push(g, "origin", None, &Spawn::new("velo"), Default::default())
    })
}

fn fetch(root: &Path) {
    with_write(root, |g| {
        sync::fetch(g, "origin", &Spawn::new("velo"), Default::default())
    })
    .unwrap();
}

fn count_snapshots(root: &Path) -> i64 {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn origin_and_clone() -> (TempDir, PathBuf, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    save(&origin, "a.txt", "shared\n", "base");
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
    (holder, origin, copy)
}

/// A clone whose pushed tip was then undone: the tracking ref names a snapshot
/// the clone no longer holds. Returns (holder, clone, undone tip).
fn clone_with_undone_pushed_tip() -> (TempDir, PathBuf, String) {
    let (h, _origin, copy) = origin_and_clone();
    let tip = save(&copy, "pushed.txt", "pushed\n", "pushed work");
    push(&copy).unwrap();
    with_write(&copy, undo::run).unwrap();
    (h, copy, tip)
}

fn absent(e: &VeloError, tip: &str) -> bool {
    matches!(e, VeloError::RemoteRefAbsent { remote, branch, snapshot }
        if remote == "origin" && branch == "main" && snapshot.as_str() == tip)
}

#[test]
fn both_spellings_are_a_typed_error() {
    let (_h, copy, tip) = clone_with_undone_pushed_tip();
    let repo = Repo::open_and_migrate(&copy).unwrap();
    for spec in ["origin/main", "remotes/origin/main"] {
        let err = commands::resolve_snapshot_id(&repo, spec).unwrap_err();
        assert!(absent(&err, &tip), "{spec}: {err:?}");
    }
}

#[test]
fn merge_of_an_absent_ref_refuses_then_works_after_fetch() {
    let (_h, copy, tip) = clone_with_undone_pushed_tip();
    save(&copy, "other.txt", "other\n", "something else");
    assert!(matches!(push(&copy), Err(VeloError::NotFastForward { .. })));

    let err = with_write(&copy, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "origin/main",
            },
        )
    })
    .unwrap_err();
    assert!(absent(&err, &tip), "{err:?}");
    assert!(!copy.join(".velo/MERGE_HEAD").exists());

    fetch(&copy);
    with_write(&copy, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "origin/main",
            },
        )
    })
    .unwrap();
    assert!(copy.join(".velo/MERGE_HEAD").exists());
    save(&copy, "other.txt", "other\n", "merge");
    push(&copy).unwrap();

    let repo = Repo::open_and_migrate(&copy).unwrap();
    let report = fsck::check(&repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

#[test]
fn merge_of_remotes_spelling_refuses_too() {
    let (_h, copy, tip) = clone_with_undone_pushed_tip();
    save(&copy, "other.txt", "other\n", "something else");
    let err = with_write(&copy, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "remotes/origin/main",
            },
        )
    })
    .unwrap_err();
    assert!(absent(&err, &tip), "{err:?}");
    assert!(!copy.join(".velo/MERGE_HEAD").exists());
}

#[test]
fn save_finishing_a_merge_refuses_an_absent_source() {
    let (_h, copy, tip) = clone_with_undone_pushed_tip();
    let pre = fs::read_to_string(copy.join(".velo/PARENT")).unwrap();
    fs::write(
        copy.join(".velo/MERGE_HEAD"),
        format!("{}:origin/main", pre.trim()),
    )
    .unwrap();
    fs::write(copy.join("more.txt"), "more\n").unwrap();
    let before = count_snapshots(&copy);
    let err = with_write(&copy, |g| {
        commands::save::run(g, Some("merge"), commands::save::Options::default())
    })
    .map(|_| ())
    .unwrap_err();
    assert!(absent(&err, &tip), "{err:?}");
    assert_eq!(count_snapshots(&copy), before);
}

#[test]
fn plan_refuses_an_id_that_is_not_live() {
    let (_h, copy, tip) = clone_with_undone_pushed_tip();
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let ours = commands::resolve_snapshot_id(&repo, "main").unwrap();
    let dead: crate::SnapshotId = tip.parse().unwrap();
    for (o, t) in [(&ours, &dead), (&dead, &ours)] {
        let err = merge::plan(&repo, o, t).map(|_| ()).unwrap_err();
        assert!(
            matches!(
                err,
                VeloError::NotFound {
                    kind: RefKind::Snapshot,
                    ..
                }
            ),
            "{err:?}"
        );
    }
}

#[test]
fn tracking_refs_at_names_the_ref_after_undo() {
    let (_h, copy, tip) = clone_with_undone_pushed_tip();
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let id: crate::SnapshotId = tip.parse().unwrap();
    assert_eq!(
        sync::tracking_refs_at(&repo, &id).unwrap(),
        vec![("origin".to_string(), "main".to_string())]
    );
    let other = commands::resolve_snapshot_id(&repo, "main").unwrap();
    assert!(sync::tracking_refs_at(&repo, &other).unwrap().is_empty());
}
