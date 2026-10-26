//! One-step `sync`: fetch, then push, fast-forward or merge, stopping at a
//! conflict in the state `velody merge` leaves.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

use crate::commands::resolve::{self, OnEdits, TakeOption};
use crate::commands::show;
use crate::commands::sync::{self, BranchCreated, Pushed, SyncOptions, Synced};
use crate::commands::{self, merge};
use crate::error::VelodyError;
use crate::progress::{Observer, Phase};
use crate::transport::Spawn;
use crate::{Author, Repo, SnapshotId};

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn write(root: &Path, rel: &str, content: &str) {
    fs::write(root.join(rel), content).unwrap();
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

fn run_sync(root: &Path, options: SyncOptions<'_>) -> crate::Result<Synced> {
    with_write(root, |g| {
        sync::sync(g, "origin", None, &Spawn::new("velody"), options)
    })
}

fn parent(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap()
        .trim()
        .to_string()
}

fn origin_tip(origin: &Path) -> String {
    let repo = Repo::open_and_migrate(origin).unwrap();
    commands::branch_tip(repo.conn(), "main").unwrap()
}

fn remote_ref(root: &Path) -> Option<String> {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::tracking_ref(repo.conn(), "main").map(|(_, h)| h)
}

/// An origin with one snapshot and a clone of it: (holder, origin, clone).
fn origin_and_clone() -> (TempDir, PathBuf, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    write(&origin, "a.txt", "shared\n");
    save(&origin, "base");
    let copy = holder.path().join("copy");
    sync::clone(
        origin.to_str().unwrap(),
        &Spawn::new("velody"),
        sync::CloneOptions {
            dir: Some(copy.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
    (holder, origin, copy)
}

#[test]
fn equal_gives_nothing() {
    let (_h, _origin, copy) = origin_and_clone();
    assert_eq!(
        run_sync(&copy, SyncOptions::default()).unwrap(),
        Synced::Nothing {
            branch: "main".into(),
            remote: "origin".into()
        }
    );
}

#[test]
fn behind_fast_forwards_the_working_tree() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "b.txt", "new\n");
    let tip = save(&origin, "theirs");
    let out = run_sync(&copy, SyncOptions::default()).unwrap();
    assert!(matches!(&out, Synced::FastForwarded { to, .. } if to.as_str() == tip));
    assert_eq!(parent(&copy), tip);
    assert_eq!(fs::read_to_string(copy.join("b.txt")).unwrap(), "new\n");
}

#[test]
fn ahead_pushes() {
    let (_h, origin, copy) = origin_and_clone();
    write(&copy, "b.txt", "mine\n");
    let tip = save(&copy, "ours");
    let out = run_sync(&copy, SyncOptions::default()).unwrap();
    assert!(matches!(
        out,
        Synced::Pushed {
            report: sync::PushReport {
                pushed: Pushed::Sent { .. },
                ..
            }
        }
    ));
    assert_eq!(origin_tip(&origin), tip);
}

#[test]
fn a_missing_remote_branch_is_created() {
    let (_h, origin, copy) = origin_and_clone();
    with_write(&copy, |g| {
        commands::switch::run(g, "feature", true).unwrap();
    });
    write(&copy, "f.txt", "f\n");
    save(&copy, "feature work");
    let out = with_write(&copy, |g| {
        sync::sync(g, "origin", None, &Spawn::new("velody"), Default::default())
    })
    .unwrap();
    match out {
        Synced::Pushed { report } => match report.pushed {
            Pushed::Sent {
                created, branch, ..
            } => {
                assert_eq!(branch, "feature");
                assert!(matches!(
                    created,
                    Some(BranchCreated::BranchWasMissing | BranchCreated::RemoteWasEmpty)
                ));
            }
            other => panic!("expected Sent, got {other:?}"),
        },
        other => panic!("expected Pushed, got {other:?}"),
    }
    let repo = Repo::open_and_migrate(&origin).unwrap();
    assert!(commands::branch_tip(repo.conn(), "feature").is_some());
}

#[test]
fn diverged_clean_merges_and_pushes() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "theirs.txt", "t\n");
    let theirs = save(&origin, "theirs");
    write(&copy, "ours.txt", "o\n");
    save(&copy, "ours");

    let out = run_sync(&copy, SyncOptions::default()).unwrap();
    let Synced::Merged { merge: m, .. } = out else {
        panic!("expected Merged, got {out:?}");
    };
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let info = show::run(&repo, &m, &[]).unwrap();
    assert_eq!(
        info.merge_parent.as_ref().map(|s| s.as_str()),
        Some(theirs.as_str())
    );
    assert_eq!(info.message, "Merge origin/main into main");
    assert_eq!(origin_tip(&origin), m.as_str());
    assert!(!copy.join(".velo/MERGE_HEAD").exists());
}

#[test]
fn the_author_is_recorded_on_the_merge() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "theirs.txt", "t\n");
    save(&origin, "theirs");
    write(&copy, "ours.txt", "o\n");
    save(&copy, "ours");
    let author = Author::with_email("Ada", "ada@example.com").unwrap();
    let out = run_sync(
        &copy,
        SyncOptions::default().author(&author).message("joined"),
    )
    .unwrap();
    let Synced::Merged { merge: m, .. } = out else {
        panic!("expected Merged, got {out:?}");
    };
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let info = show::run(&repo, &m, &[]).unwrap();
    assert_eq!(info.message, "joined");
    let meta = repo.snapshot_meta(&m).unwrap();
    assert_eq!(meta.author(), Some(author));
}

#[test]
fn a_conflict_stops_then_resolve_save_sync_pushes() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "a.txt", "theirs\n");
    let theirs = save(&origin, "theirs");
    write(&copy, "a.txt", "ours\n");
    save(&copy, "ours");

    let out = run_sync(&copy, SyncOptions::default()).unwrap();
    assert_eq!(
        out,
        Synced::Conflicted {
            branch: "main".into(),
            remote: "origin".into(),
            paths: vec![PathBuf::from("a.txt")],
        }
    );
    assert_eq!(origin_tip(&origin), theirs, "nothing may be pushed");
    assert!(copy.join(".velo/MERGE_HEAD").exists());

    // Syncing mid-merge is refused rather than guessing.
    assert!(matches!(
        run_sync(&copy, SyncOptions::default()),
        Err(VelodyError::OperationInProgress { .. })
    ));

    let repo = Repo::open_and_migrate(&copy).unwrap();
    let row = resolve::get_conflict(&repo, "a.txt").unwrap();
    {
        let g = repo.write().unwrap();
        resolve::take_side(&g, &row, TakeOption::Ours, OnEdits::Refuse).unwrap();
    }
    drop(repo);
    let merged = save(&copy, "resolved");
    let out = run_sync(&copy, SyncOptions::default()).unwrap();
    assert!(matches!(out, Synced::Pushed { .. }), "got {out:?}");
    assert_eq!(origin_tip(&origin), merged);
}

#[test]
fn a_dirty_tree_is_refused_before_any_fetch() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "b.txt", "new\n");
    save(&origin, "theirs");
    write(&copy, "a.txt", "unsaved\n");
    let before = remote_ref(&copy);
    assert!(matches!(
        run_sync(&copy, SyncOptions::default()),
        Err(VelodyError::DirtyWorkingTree { .. })
    ));
    assert_eq!(remote_ref(&copy), before, "no fetch may have happened");
}

#[test]
fn a_merge_in_progress_is_refused() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "a.txt", "theirs\n");
    save(&origin, "theirs");
    write(&copy, "a.txt", "ours\n");
    save(&copy, "ours");
    with_write(&copy, |g| {
        sync::fetch(g, "origin", &Spawn::new("velody"), Default::default()).unwrap();
        merge::run(
            g,
            merge::Mode::Bring {
                source: "origin/main",
            },
        )
        .unwrap();
    });
    assert!(matches!(
        run_sync(&copy, SyncOptions::default()),
        Err(VelodyError::OperationInProgress { .. })
    ));
}

#[test]
fn naming_another_branch_is_invalid() {
    let (_h, _origin, copy) = origin_and_clone();
    let err = with_write(&copy, |g| {
        sync::sync(
            g,
            "origin",
            Some("other"),
            &Spawn::new("velody"),
            Default::default(),
        )
    })
    .unwrap_err();
    assert!(
        matches!(err, VelodyError::InvalidInput { .. }),
        "got {err:?}"
    );
}

/// Moves origin forward as the push phase begins, after our fetch.
struct RaceOnSecondTransfer<'a> {
    origin: &'a Path,
    transfers: AtomicUsize,
}

impl Observer for RaceOnSecondTransfer<'_> {
    fn begin(&self, phase: Phase, _total: Option<u64>) {
        if phase == Phase::Transferring && self.transfers.fetch_add(1, Ordering::SeqCst) == 1 {
            write(self.origin, "race.txt", "r\n");
            save(self.origin, "second writer");
        }
    }
}

#[test]
fn a_second_writer_between_fetch_and_push_keeps_the_local_merge() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "theirs.txt", "t\n");
    save(&origin, "theirs");
    write(&copy, "ours.txt", "o\n");
    save(&copy, "ours");

    let racer = RaceOnSecondTransfer {
        origin: &origin,
        transfers: AtomicUsize::new(0),
    };
    let err = run_sync(&copy, SyncOptions::default().observer(&racer)).unwrap_err();
    assert!(
        matches!(err, VelodyError::NotFastForward { .. }),
        "got {err:?}"
    );

    // The merge snapshot stays, with its second parent, and nothing is mid-merge.
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let info = show::run(&repo, &SnapshotId::from_stored(parent(&copy)), &[]).unwrap();
    assert!(info.merge_parent.is_some());
    assert!(!copy.join(".velo/MERGE_HEAD").exists());

    // Running sync again continues: it merges the racer's commit and pushes.
    let out = run_sync(&copy, SyncOptions::default()).unwrap();
    assert!(matches!(out, Synced::Merged { .. }), "got {out:?}");
    assert_eq!(origin_tip(&origin), parent(&copy));
}
