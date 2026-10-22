//! `MERGE_HEAD` records the snapshot a merge combined, so a fetch between the
//! merge and the save cannot change the saved merge's second parent.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::sync;
use crate::commands::{self, merge};
use crate::transport::Spawn;
use crate::Repo;

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn save(root: &Path, file: &str, content: &str, msg: &str) -> String {
    fs::write(root.join(file), content).unwrap();
    commit(root, msg)
}

fn commit(root: &Path, msg: &str) -> String {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot")
    .hash
    .into_string()
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

fn merge_origin(root: &Path) -> merge::Outcome {
    with_write(root, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "origin/main",
            },
        )
    })
    .unwrap()
}

fn clone_of(holder: &Path, origin: &Path, name: &str) -> PathBuf {
    let copy = holder.join(name);
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
    copy
}

/// Remote, clone A and clone B, all at the base snapshot.
fn setup() -> (TempDir, PathBuf, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    save(&origin, "a.txt", "shared\n", "base");
    let a = clone_of(holder.path(), &origin, "a");
    let b = clone_of(holder.path(), &origin, "b");
    (holder, a, b)
}

fn merge_parent_of(root: &Path, hash: &str) -> String {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row(
            "SELECT merge_parent FROM snapshots WHERE hash = ?1",
            [hash],
            |r| r.get(0),
        )
        .unwrap()
}

fn origin_tip(root: &Path) -> String {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::resolve_snapshot_id(&repo, "origin/main")
        .unwrap()
        .into_string()
}

#[test]
fn a_fetch_between_merge_and_save_does_not_move_the_merge_parent() {
    let (_h, a, b) = setup();
    save(&a, "a_only.txt", "a\n", "a1");
    let b1 = save(&b, "b1.txt", "one\n", "b1");
    push(&b);
    fetch(&a);
    assert!(matches!(merge_origin(&a), merge::Outcome::Merged(_)));

    save(&b, "b2.txt", "two\n", "b2");
    push(&b);
    fetch(&a);
    assert_ne!(origin_tip(&a), b1, "the fetch must move origin/main");

    let merged = commit(&a, "merge");
    assert_eq!(merge_parent_of(&a, &merged), b1);

    // b2 was never merged, so a second merge still has work to do.
    let again = merge_origin(&a);
    assert!(
        !matches!(again, merge::Outcome::AlreadyUpToDate { .. }),
        "b2 was lost"
    );
    assert!(a.join("b2.txt").exists());
}

#[test]
fn the_old_two_field_form_is_resolved_again_at_save() {
    let (_h, a, b) = setup();
    save(&a, "a_only.txt", "a\n", "a1");
    save(&b, "b1.txt", "one\n", "b1");
    push(&b);
    fetch(&a);
    merge_origin(&a);
    let head = fs::read_to_string(a.join(".velo/MERGE_HEAD")).unwrap();
    let (pre, _) = head.split_once(':').unwrap();
    fs::write(a.join(".velo/MERGE_HEAD"), format!("{pre}:origin/main")).unwrap();

    let b2 = save(&b, "b2.txt", "two\n", "b2");
    push(&b);
    fetch(&a);
    let merged = commit(&a, "merge");
    assert_eq!(merge_parent_of(&a, &merged), b2);
}

#[test]
fn abort_after_a_fetch_removes_exactly_what_the_merge_added() {
    let (_h, a, b) = setup();
    save(&a, "a_only.txt", "a\n", "a1");
    save(&b, "b1.txt", "one\n", "b1");
    push(&b);
    fetch(&a);
    merge_origin(&a);
    assert!(a.join("b1.txt").exists());

    // The source moves on and no longer has b1.txt at all.
    fs::remove_file(b.join("b1.txt")).unwrap();
    commit(&b, "drop b1");
    push(&b);
    fetch(&a);

    with_write(&a, |g| merge::run(g, merge::Mode::Abort)).unwrap();
    assert!(!a.join("b1.txt").exists(), "merge-added file left behind");
    assert!(a.join("a_only.txt").exists());
    assert!(!a.join(".velo/MERGE_HEAD").exists());
}
