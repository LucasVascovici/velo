//! Historical diffs classify binary files from the snapshot objects, not from
//! whatever the working tree holds at that path now.

use std::fs;
use tempfile::TempDir;

use crate::commands::diff::FileChange;
use crate::commands::{save, show};
use crate::{Repo, SnapshotId};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn save_all(repo: &Repo, msg: &str) -> SnapshotId {
    let guard = repo.write().unwrap();
    save::run(&guard, Some(msg), save::Options::default())
        .unwrap()
        .into_result()
        .expect("expected a snapshot")
        .hash
}

fn change_for(repo: &Repo, id: &SnapshotId, path: &str) -> FileChange {
    let detail = show::run(repo, id, &[]).unwrap();
    detail
        .diff
        .files
        .into_iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not in diff"))
        .change
}

#[test]
fn added_binary_stays_binary_when_worktree_is_now_text() {
    let (tmp, repo) = setup();
    fs::write(tmp.path().join("a.dat"), b"\x00\x01\x02bin").unwrap();
    let id = save_all(&repo, "add binary");
    fs::write(tmp.path().join("a.dat"), "plain text now\n").unwrap();
    assert!(matches!(
        change_for(&repo, &id, "a.dat"),
        FileChange::BinaryChanged { added: true }
    ));
}

#[test]
fn added_text_stays_text_when_worktree_is_now_binary() {
    let (tmp, repo) = setup();
    fs::write(tmp.path().join("a.txt"), "one\ntwo\n").unwrap();
    let id = save_all(&repo, "add text");
    fs::write(tmp.path().join("a.txt"), b"\x00\x00bin").unwrap();
    match change_for(&repo, &id, "a.txt") {
        FileChange::Added { lines } => assert_eq!(lines, vec!["one", "two"]),
        other => panic!("expected Added, got {other:?}"),
    }
}

#[test]
fn modified_with_binary_historical_side_is_binary() {
    let (tmp, repo) = setup();
    fs::write(tmp.path().join("m.dat"), "text\n").unwrap();
    save_all(&repo, "text");
    fs::write(tmp.path().join("m.dat"), b"\x00bin").unwrap();
    let id = save_all(&repo, "binary");
    fs::write(tmp.path().join("m.dat"), "text again\n").unwrap();
    assert!(matches!(
        change_for(&repo, &id, "m.dat"),
        FileChange::BinaryChanged { added: false }
    ));
}
