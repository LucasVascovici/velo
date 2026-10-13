//! `switch` to the branch you are on repositions a stale or empty working
//! position instead of answering "already on".

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::switch::{self, Outcome};
use crate::commands::{self, bundle, history, restore};
use crate::error::VeloError;
use crate::{Repo, SnapshotId};

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

fn parent(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn switch_main(root: &Path, force: bool) -> crate::error::Result<Outcome> {
    with_write(root, |g| switch::run(g, "main", force))
}

/// A source repo with two saves, bundled into a second, empty repo.
fn imported() -> (TempDir, TempDir, String, String) {
    let src = TempDir::new().unwrap();
    commands::init::run(src.path()).unwrap();
    fs::write(src.path().join("a.txt"), "one").unwrap();
    let first = save(src.path(), "first");
    fs::write(src.path().join("a.txt"), "two").unwrap();
    fs::write(src.path().join("b.txt"), "bee").unwrap();
    let second = save(src.path(), "second");
    let file = src.path().join("out.bundle");
    let repo = Repo::open_and_migrate(src.path()).unwrap();
    bundle::create(&repo, &file, None).unwrap();
    let dst = TempDir::new().unwrap();
    commands::init::run(dst.path()).unwrap();
    with_write(dst.path(), |g| bundle::apply(g, &file)).unwrap();
    (src, dst, first, second)
}

#[test]
fn bundle_import_then_switch_positions_on_the_tip() {
    let (_src, dst, _first, second) = imported();
    assert_eq!(parent(dst.path()), "");
    let out = switch_main(dst.path(), false).unwrap();
    assert_eq!(
        out,
        Outcome::Switched {
            branch: "main".into(),
            at: second.clone()
        }
    );
    assert_eq!(parent(dst.path()), second);
    assert_eq!(fs::read_to_string(dst.path().join("a.txt")).unwrap(), "two");
    assert_eq!(fs::read_to_string(dst.path().join("b.txt")).unwrap(), "bee");
    let repo = Repo::open_and_migrate(dst.path()).unwrap();
    let h = history::run(&repo, Default::default()).unwrap();
    assert_eq!(h.entries.len(), 2);
}

#[test]
fn already_on_when_position_is_the_tip() {
    let (_src, dst, _first, _second) = imported();
    switch_main(dst.path(), false).unwrap();
    fs::write(dst.path().join("a.txt"), "unsaved").unwrap();
    let out = switch_main(dst.path(), false).unwrap();
    assert_eq!(
        out,
        Outcome::AlreadyOn {
            branch: "main".into()
        }
    );
    assert_eq!(
        fs::read_to_string(dst.path().join("a.txt")).unwrap(),
        "unsaved"
    );
}

#[test]
fn stale_position_refuses_dirty_file_and_force_repositions() {
    let (_src, dst, first, second) = imported();
    switch_main(dst.path(), false).unwrap();
    with_write(dst.path(), |g| {
        restore::run(
            g,
            &SnapshotId::from_stored(first.as_str()),
            Default::default(),
        )
    })
    .unwrap();
    assert_eq!(parent(dst.path()), first);
    fs::write(dst.path().join("a.txt"), "edited").unwrap();
    match switch_main(dst.path(), false) {
        Err(VeloError::DirtyWorkingTree { paths }) => {
            assert_eq!(paths, vec![PathBuf::from("a.txt")]);
        }
        other => panic!("expected DirtyWorkingTree, got {other:?}"),
    }
    assert_eq!(parent(dst.path()), first);
    let out = switch_main(dst.path(), true).unwrap();
    assert!(matches!(out, Outcome::Switched { ref at, .. } if *at == second));
    assert_eq!(parent(dst.path()), second);
    assert_eq!(fs::read_to_string(dst.path().join("a.txt")).unwrap(), "two");
}

#[test]
fn empty_position_guards_untracked_collision() {
    let (_src, dst, _first, second) = imported();
    fs::write(dst.path().join("a.txt"), "mine").unwrap();
    match switch_main(dst.path(), false) {
        Err(VeloError::DirtyWorkingTree { paths }) => {
            assert_eq!(paths, vec![PathBuf::from("a.txt")]);
        }
        other => panic!("expected DirtyWorkingTree, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(dst.path().join("a.txt")).unwrap(),
        "mine"
    );
    switch_main(dst.path(), true).unwrap();
    assert_eq!(parent(dst.path()), second);
    assert_eq!(fs::read_to_string(dst.path().join("a.txt")).unwrap(), "two");
}
