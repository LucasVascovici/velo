//! `undo` refuses to shelve a snapshot that another live snapshot builds on.

use tempfile::TempDir;

use crate::commands::{fsck, merge, save, stash, switch, undo};
use crate::error::VeloError;
use crate::Repo;

fn setup() -> TempDir {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    tmp
}

fn save_file(root: &std::path::Path, name: &str, content: &str, msg: &str) {
    std::fs::write(root.join(name), content).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    save::run(&guard, Some(msg), save::Options::default()).unwrap();
}

fn go(root: &std::path::Path, branch: &str) {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    switch::run(&guard, branch, false).unwrap();
}

fn try_undo(root: &std::path::Path) -> crate::Result<undo::Outcome> {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    undo::run(&guard)
}

fn count(root: &std::path::Path, sql: &str) -> i64 {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn healthy(root: &std::path::Path) -> bool {
    let repo = Repo::open_and_migrate(root).unwrap();
    fsck::check(&repo).unwrap().is_healthy()
}

/// base on main; A on wip; B on side (on top of A); back on wip.
fn repro() -> TempDir {
    let tmp = setup();
    let p = tmp.path();
    save_file(p, "f.txt", "base", "base");
    go(p, "wip");
    save_file(p, "f.txt", "a", "A");
    go(p, "side");
    save_file(p, "f.txt", "b", "B");
    go(p, "wip");
    tmp
}

fn expect_dependents(err: VeloError) -> (Vec<(String, usize)>, usize) {
    match err {
        VeloError::SnapshotHasDependents {
            branches,
            dependents,
            ..
        } => (
            branches
                .into_iter()
                .map(|(b, n)| (b.as_str().to_string(), n))
                .collect(),
            dependents,
        ),
        other => panic!("expected SnapshotHasDependents, got {other}"),
    }
}

#[test]
fn undo_refuses_when_another_branch_builds_on_the_tip() {
    let tmp = repro();
    let p = tmp.path();
    let parent_before = std::fs::read_to_string(p.join(".velo/PARENT")).unwrap();
    let (branches, n) = expect_dependents(try_undo(p).unwrap_err());
    assert_eq!(branches, vec![("side".to_string(), 1)]);
    assert_eq!(n, 1);

    assert!(healthy(p));
    assert_eq!(
        count(p, "SELECT COUNT(*) FROM snapshots WHERE message='A'"),
        1
    );
    assert_eq!(count(p, "SELECT COUNT(*) FROM trash"), 0);
    assert_eq!(
        count(p, "SELECT COUNT(*) FROM snapshots WHERE branch='side'"),
        1
    );
    assert_eq!(
        std::fs::read_to_string(p.join(".velo/PARENT")).unwrap(),
        parent_before
    );
    assert_eq!(std::fs::read_to_string(p.join("f.txt")).unwrap(), "a");
}

#[test]
fn undo_refuses_when_only_a_merge_parent_references_the_tip() {
    let tmp = setup();
    let p = tmp.path();
    save_file(p, "f.txt", "base", "base");
    go(p, "wip");
    save_file(p, "w.txt", "w", "A");
    go(p, "main");
    save_file(p, "m.txt", "m", "M1");
    {
        let repo = Repo::open_and_migrate(p).unwrap();
        let guard = repo.write().unwrap();
        merge::run(&guard, merge::Mode::Bring { source: "wip" }).unwrap();
        // The merge is recorded by the save that follows it.
        save::run(&guard, Some("merge"), save::Options::default()).unwrap();
    }
    assert_eq!(
        count(
            p,
            "SELECT COUNT(*) FROM snapshots WHERE COALESCE(merge_parent,'') <> ''"
        ),
        1
    );
    go(p, "wip");
    let (branches, n) = expect_dependents(try_undo(p).unwrap_err());
    assert_eq!(branches, vec![("main".to_string(), 1)]);
    assert_eq!(n, 1);
    assert!(healthy(p));
}

#[test]
fn undo_refuses_when_a_stash_sits_on_the_tip() {
    let tmp = setup();
    let p = tmp.path();
    save_file(p, "f.txt", "one", "one");
    save_file(p, "f.txt", "two", "two");
    std::fs::write(p.join("f.txt"), "dirty").unwrap();
    {
        let repo = Repo::open_and_migrate(p).unwrap();
        let guard = repo.write().unwrap();
        stash::push(&guard, None).unwrap();
    }
    let (branches, n) = expect_dependents(try_undo(p).unwrap_err());
    assert_eq!(branches, vec![("_stash".to_string(), 1)]);
    assert_eq!(n, 1);
    assert!(healthy(p));
}

#[test]
fn undo_of_an_unshared_tip_still_works() {
    let tmp = repro();
    let p = tmp.path();
    go(p, "side");
    let out = try_undo(p).unwrap();
    assert_eq!(out.message, "B");
    assert!(healthy(p));
}

#[test]
fn deep_chain_counts_every_descendant() {
    let tmp = repro();
    let p = tmp.path();
    go(p, "side");
    save_file(p, "f.txt", "c", "C");
    go(p, "wip");
    let (branches, n) = expect_dependents(try_undo(p).unwrap_err());
    assert_eq!(branches, vec![("side".to_string(), 2)]);
    assert_eq!(n, 2);
}
