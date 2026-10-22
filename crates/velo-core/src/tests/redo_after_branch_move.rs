//! `redo` after the branch moved: it refuses with `RedoBranchMoved` instead of
//! bringing back a snapshot that is no longer the tip's parent.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::sync::{self, SyncOptions};
use crate::commands::{self, merge, redo, status, undo};
use crate::error::VeloError;
use crate::transport::Spawn;
use crate::Repo;

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

fn parent(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap()
        .trim()
        .to_string()
}

fn tip(root: &Path) -> String {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::branch_tip(repo.conn(), "main").unwrap()
}

fn undo_once(root: &Path) {
    with_write(root, undo::run).unwrap();
}

fn redo_once(root: &Path) -> crate::Result<redo::Outcome> {
    with_write(root, redo::run)
}

fn peek(root: &Path) -> Option<redo::Outcome> {
    let repo = Repo::open_and_migrate(root).unwrap();
    redo::peek(&repo).unwrap()
}

fn fetch(root: &Path) {
    with_write(root, |g| {
        sync::fetch(g, "origin", &Spawn::new("velo"), Default::default()).unwrap();
    });
}

fn bring_origin(root: &Path) -> crate::Result<merge::Outcome> {
    with_write(root, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "origin/main",
            },
        )
    })
}

/// An origin with two snapshots and a clone of it: (holder, origin, clone).
fn origin_and_clone() -> (TempDir, PathBuf, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    write(&origin, "a.txt", "one\n");
    save(&origin, "one");
    write(&origin, "a.txt", "two\n");
    save(&origin, "two");
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

#[test]
fn undo_then_redo_with_no_move_restores_the_same_snapshot() {
    let (_h, _origin, copy) = origin_and_clone();
    let before = tip(&copy);
    undo_once(&copy);
    assert!(peek(&copy).is_some());
    let out = redo_once(&copy).unwrap();
    assert_eq!(out.snapshot, before);
    assert_eq!(parent(&copy), before);
}

#[test]
fn a_fast_forward_pull_after_two_undos_makes_redo_refuse() {
    let (_h, origin, copy) = origin_and_clone();
    undo_once(&copy);
    undo_once(&copy);
    write(&origin, "a.txt", "three\n");
    let newer = save(&origin, "three");
    fetch(&copy);
    bring_origin(&copy).unwrap();
    assert_eq!(tip(&copy), newer);
    assert_eq!(parent(&copy), newer);
    let tree = fs::read_to_string(copy.join("a.txt")).unwrap();

    assert!(peek(&copy).is_none(), "peek must agree with run");
    match redo_once(&copy).unwrap_err() {
        VeloError::RedoBranchMoved {
            branch, tip: moved, ..
        } => {
            assert_eq!(branch, "main");
            assert_eq!(moved.map(|t| t.into_string()), Some(newer.clone()));
        }
        other => panic!("expected RedoBranchMoved, got {other:?}"),
    }
    // Nothing was written: tree, PARENT and the reported position all still
    // sit on the tip.
    assert_eq!(parent(&copy), newer);
    assert_eq!(fs::read_to_string(copy.join("a.txt")).unwrap(), tree);
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let st = status::run(&repo, &[]).unwrap();
    assert_eq!(st.position.as_deref(), Some(newer.as_str()));
}

#[test]
fn a_one_step_sync_fast_forward_makes_redo_refuse() {
    let (_h, origin, copy) = origin_and_clone();
    undo_once(&copy);
    write(&origin, "b.txt", "new\n");
    save(&origin, "three");
    with_write(&copy, |g| {
        sync::sync(
            g,
            "origin",
            None,
            &Spawn::new("velo"),
            SyncOptions::default(),
        )
    })
    .unwrap();
    assert!(matches!(
        redo_once(&copy),
        Err(VeloError::RedoBranchMoved { .. })
    ));
    assert!(peek(&copy).is_none());
}

#[test]
fn a_three_way_merge_then_save_never_redoes_over_it() {
    let (_h, origin, copy) = origin_and_clone();
    undo_once(&copy);
    write(&origin, "b.txt", "theirs\n");
    save(&origin, "theirs");
    fetch(&copy);
    bring_origin(&copy).unwrap();
    // Whether the merge committed or is waiting for a save, redo must not
    // bring the undone snapshot back over it.
    assert!(redo_once(&copy).is_err());
    if copy.join(".velo/MERGE_HEAD").exists() {
        save(&copy, "merge");
    }
    assert!(peek(&copy).is_none());
    assert!(redo_once(&copy).is_err());
}

#[test]
fn a_save_after_undo_clears_the_redo_stack() {
    let (_h, _origin, copy) = origin_and_clone();
    undo_once(&copy);
    write(&copy, "c.txt", "mine\n");
    save(&copy, "mine");
    assert!(peek(&copy).is_none());
    assert!(redo_once(&copy).is_err());
}

#[test]
fn switching_away_and_back_keeps_redo_working() {
    let (_h, _origin, copy) = origin_and_clone();
    let before = tip(&copy);
    undo_once(&copy);
    with_write(&copy, |g| commands::switch::run(g, "feature", false)).unwrap();
    with_write(&copy, |g| commands::switch::run(g, "main", false)).unwrap();
    assert!(peek(&copy).is_some());
    let out = redo_once(&copy).unwrap();
    assert_eq!(out.snapshot, before);
    assert_eq!(parent(&copy), before);
}
