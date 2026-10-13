//! Stash fixes (15.1-d): real timestamps in `list`, and `pop` counting only the
//! files it actually wrote.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::{self, stash};
use crate::Repo;

fn setup() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    commands::init::run(&path).unwrap();
    (tmp, path)
}

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    f(&guard)
}

fn write(root: &Path, rel: &str, content: &str) {
    fs::write(root.join(rel), content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
}

fn save(root: &Path, msg: &str) {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap();
}

fn list(root: &Path) -> Vec<stash::Shelf> {
    let repo = Repo::open_and_migrate(root).unwrap();
    stash::list(&repo).unwrap()
}

fn push(root: &Path, name: &str) {
    with_write(root, |g| stash::push(g, Some(name.into()))).unwrap();
}

#[test]
fn a_fresh_shelf_lists_the_current_time() {
    let (_tmp, root) = setup();
    write(&root, "a.txt", "v1");
    save(&root, "base");
    write(&root, "a.txt", "v2");
    push(&root, "wip");

    let shelves = list(&root);
    let age = chrono::Utc::now() - shelves[0].created_at;
    assert!(age.num_seconds().abs() < 60, "age was {age}");
}

#[test]
fn a_legacy_zero_row_falls_back_to_the_shelf_snapshot_time() {
    let (_tmp, root) = setup();
    write(&root, "a.txt", "v1");
    save(&root, "base");
    write(&root, "a.txt", "v2");
    push(&root, "wip");

    let repo = Repo::open_and_migrate(&root).unwrap();
    repo.conn()
        .execute("UPDATE stash SET created_at_ms = 0", [])
        .unwrap();
    let shelves = stash::list(&repo).unwrap();
    assert!(shelves[0].created_at.timestamp_millis() > 1_577_836_800_000);
}

#[test]
fn pop_counts_only_files_it_wrote_and_contents_are_right() {
    let (_tmp, root) = setup();
    for i in 0..14 {
        write(&root, &format!("f{i}.txt"), &format!("v1-{i}"));
    }
    save(&root, "base");
    write(&root, "f3.txt", "edited-3");
    write(&root, "f9.txt", "edited-9");
    push(&root, "wip");

    let popped = with_write(&root, |g| stash::pop(g, None)).unwrap();
    assert_eq!(popped.restored, 2);
    for i in 0..14 {
        let want = match i {
            3 => "edited-3".to_string(),
            9 => "edited-9".to_string(),
            _ => format!("v1-{i}"),
        };
        assert_eq!(read(&root, &format!("f{i}.txt")), want);
    }
}

#[test]
fn pop_still_removes_a_shelved_deletion() {
    let (_tmp, root) = setup();
    write(&root, "keep.txt", "k");
    write(&root, "gone.txt", "g");
    save(&root, "base");
    fs::remove_file(root.join("gone.txt")).unwrap();
    push(&root, "wip");

    let popped = with_write(&root, |g| stash::pop(g, None)).unwrap();
    assert_eq!(popped.removed, 1);
    assert_eq!(popped.restored, 0);
    assert!(!root.join("gone.txt").exists());
    assert_eq!(read(&root, "keep.txt"), "k");
}
