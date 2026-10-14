//! `bundle apply` counts the tags it imports (17.6).

use tempfile::TempDir;

use crate::commands::{bundle, init, tag};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta, TagName};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn save(repo: &Repo) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "one",
            entries: vec![TreeEntry::file("f.txt", b"hello".to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap()
}

fn name(s: &str) -> TagName {
    s.parse().unwrap()
}

/// Bundle `from` into `dir` and apply it to `to`.
fn ship(from: &Repo, to: &Repo, dir: &TempDir, file: &str) -> bundle::Applied {
    let path = dir.path().join(file);
    bundle::create(from, &path, None).unwrap();
    bundle::apply(&to.write().unwrap(), &path).unwrap()
}

#[test]
fn tag_only_bundle_counts_the_tag() {
    let (_a, sender) = setup();
    let (_b, receiver) = setup();
    let scratch = TempDir::new().unwrap();
    let id = save(&sender);
    let first = ship(&sender, &receiver, &scratch, "1.bundle");
    assert!(!first.is_empty());

    tag::create(&sender.write().unwrap(), &name("v1"), Some(&id), false).unwrap();
    let second = ship(&sender, &receiver, &scratch, "2.bundle");
    assert_eq!((second.snapshots, second.objects, second.tags), (0, 0, 1));
    assert!(!second.is_empty());
    assert_eq!(tag::list(&receiver).unwrap().len(), 1);
}

#[test]
fn nothing_new_is_still_empty() {
    let (_a, sender) = setup();
    let (_b, receiver) = setup();
    let scratch = TempDir::new().unwrap();
    let id = save(&sender);
    tag::create(&sender.write().unwrap(), &name("v1"), Some(&id), false).unwrap();
    ship(&sender, &receiver, &scratch, "1.bundle");
    let again = ship(&sender, &receiver, &scratch, "2.bundle");
    assert_eq!(again.tags, 0);
    assert!(again.is_empty());
}

#[test]
fn a_tag_name_already_held_is_not_counted() {
    let (_a, sender) = setup();
    let (_b, receiver) = setup();
    let scratch = TempDir::new().unwrap();
    let id = save(&sender);
    ship(&sender, &receiver, &scratch, "1.bundle");
    tag::create(&sender.write().unwrap(), &name("v1"), Some(&id), false).unwrap();
    tag::create(&receiver.write().unwrap(), &name("v1"), Some(&id), false).unwrap();
    let applied = ship(&sender, &receiver, &scratch, "2.bundle");
    assert_eq!(applied.tags, 0);
    assert!(applied.is_empty());
}
