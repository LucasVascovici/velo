//! `remotes/<r>/<b>` resolves through `remote_refs`, like `<r>/<b>`, and
//! history decorates remote-tracking tips from the same table.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::history;
use crate::commands::merge;
use crate::commands::sync;
use crate::commands::{self};
use crate::ids::SnapshotId;
use crate::transport::Spawn;
use crate::Repo;

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

fn write(root: &Path, rel: &str, content: &str) {
    fs::write(root.join(rel), content).unwrap();
}

fn push(root: &Path) {
    with_write(root, |g| {
        sync::push(g, "origin", None, &Spawn::new("velo"), Default::default())
    })
    .unwrap();
}

fn fetch(root: &Path) {
    with_write(root, |g| {
        sync::fetch(g, "origin", &Spawn::new("velo"), Default::default())
    })
    .unwrap();
}

fn clone_of(origin: &Path, holder: &Path, name: &str) -> PathBuf {
    let dir = holder.join(name);
    sync::clone(
        origin.to_str().unwrap(),
        &Spawn::new("velo"),
        sync::CloneOptions {
            dir: Some(dir.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
    dir
}

/// The two-clone repro: b pushes, c merges and pushes, then fetches.
/// Returns (holder, c, b's snapshot, c's pushed merge).
fn repro() -> (TempDir, PathBuf, String, String) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    write(&origin, "a.txt", "base\n");
    save(&origin, "base");
    let b = clone_of(&origin, holder.path(), "b");
    let c = clone_of(&origin, holder.path(), "c");
    write(&b, "b.txt", "b\n");
    let b_snap = save(&b, "b work");
    push(&b);
    write(&c, "c.txt", "c\n");
    save(&c, "c work");
    fetch(&c);
    with_write(&c, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "remotes/origin/main",
            },
        )
    })
    .unwrap();
    let merged = save(&c, "merge from b");
    push(&c);
    fetch(&c);
    (holder, c, b_snap, merged)
}

#[test]
fn both_spellings_resolve_to_the_pushed_merge() {
    let (_h, c, _b, merged) = repro();
    let repo = Repo::open_and_migrate(&c).unwrap();
    let short = commands::resolve_snapshot_id(&repo, "origin/main").unwrap();
    let long = commands::resolve_snapshot_id(&repo, "remotes/origin/main").unwrap();
    assert_eq!(short, long);
    assert_eq!(short.as_str(), merged);
    let tip = repo
        .branch_tip(&"remotes/origin/main".parse().unwrap())
        .unwrap();
    assert_eq!(tip.as_ref().map(|t| t.as_str()), Some(merged.as_str()));
}

#[test]
fn merging_through_the_long_spelling_records_the_merge_parent() {
    // `repro` merges `remotes/origin/main` while it is b's snapshot; the save
    // that follows must have recorded it as the merge parent.
    let (_h, c, b_snap, _merged) = repro();
    let repo = Repo::open_and_migrate(&c).unwrap();
    let h = history::run(
        &repo,
        history::Options {
            all: true,
            ..Default::default()
        },
    )
    .unwrap();
    let entry = h
        .entries
        .iter()
        .find(|e| e.message == "merge from b")
        .unwrap();
    assert!(
        entry.merge_parent.as_ref().map(|p| p.as_str()) == Some(b_snap.as_str()),
        "{entry:?}"
    );
}

#[test]
fn history_refs_come_from_remote_refs() {
    let (_h, c, b_snap, merged) = repro();
    let repo = Repo::open_and_migrate(&c).unwrap();
    let h = history::run(
        &repo,
        history::Options {
            all: true,
            ..Default::default()
        },
    )
    .unwrap();
    let has = |id: &str| {
        h.refs_at(&SnapshotId::from_stored(id.to_string()))
            .iter()
            .any(|r| r.name.as_str() == "remotes/origin/main")
    };
    assert!(has(&merged));
    assert!(!has(&b_snap));
}
