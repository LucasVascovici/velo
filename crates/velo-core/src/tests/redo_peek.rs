//! `redo::peek`: reports what `redo` would restore, without touching the trash.

use tempfile::TempDir;

use crate::commands::{redo, save, undo};
use crate::Repo;

fn setup() -> TempDir {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    tmp
}

fn save_file(root: &std::path::Path, content: &str, msg: &str) {
    std::fs::write(root.join("f.txt"), content).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    save::run(&guard, Some(msg), save::Options::default()).unwrap();
}

fn trash_rows(root: &std::path::Path) -> i64 {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row("SELECT COUNT(*) FROM trash", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn peek_is_none_before_any_undo() {
    let tmp = setup();
    save_file(tmp.path(), "v1", "one");
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    assert!(redo::peek(&repo).unwrap().is_none());
}

#[test]
fn peek_names_the_undone_snapshot_and_leaves_the_trash_alone() {
    let tmp = setup();
    save_file(tmp.path(), "v1", "one");
    save_file(tmp.path(), "v2", "two");
    {
        let repo = Repo::open_and_migrate(tmp.path()).unwrap();
        let guard = repo.write().unwrap();
        undo::run(&guard).unwrap();
    }
    assert_eq!(trash_rows(tmp.path()), 1);

    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let peeked = redo::peek(&repo).unwrap().expect("something to redo");
    assert_eq!(peeked.message, "two");
    assert_eq!(trash_rows(tmp.path()), 1, "peek must not consume the trash");

    // And it matches what redo really restores.
    drop(repo);
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let guard = repo.write().unwrap();
    let redone = redo::run(&guard).unwrap();
    assert_eq!(redone, peeked);
}
