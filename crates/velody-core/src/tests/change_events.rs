//! Change events (`Repo::listening`): what was committed, delivered after commit.

use std::sync::mpsc::{self, Receiver};
use tempfile::TempDir;

use crate::commands::merge::{self, MergeCommit, Resolution};
use crate::commands::{branches, bundle, save, tag};
use crate::events::{Event, Ref};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Error, Repo, SnapshotId, SnapshotMeta, TagName};

fn listened() -> (TempDir, Repo, Receiver<Event>) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let (tx, rx) = mpsc::channel();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap().listening(tx);
    (tmp, repo, rx)
}

fn branch(name: &str) -> BranchName {
    name.parse().unwrap()
}

fn put(
    g: &crate::WriteGuard,
    b: &BranchName,
    parent: Option<&SnapshotId>,
    merge_parent: Option<&SnapshotId>,
    body: &str,
    ts: i64,
) -> crate::Result<SnapshotId> {
    g.save_tree(SaveTree {
        branch: b,
        parent,
        merge_parent,
        message: "t",
        entries: vec![TreeEntry::file("f.txt", body.as_bytes().to_vec())],
        meta: SnapshotMeta::new(),
        author: None,
        timestamp_ms: Some(ts),
        renames: &[],
    })
}

fn drain(rx: &Receiver<Event>) -> Vec<Event> {
    rx.try_iter().collect()
}

#[test]
fn save_tree_emits_one_saved() {
    let (_tmp, repo, rx) = listened();
    let main = branch("main");
    let g = repo.write().unwrap();
    let first = put(&g, &main, None, None, "a\n", 1).unwrap();
    let second = put(&g, &main, Some(&first), None, "b\n", 2).unwrap();
    assert_eq!(
        drain(&rx),
        vec![
            Event::Saved {
                snapshot: first.clone(),
                branch: main.clone(),
                parent: None,
                merge_parent: None
            },
            Event::Saved {
                snapshot: second,
                branch: main,
                parent: Some(first),
                merge_parent: None
            },
        ]
    );
}

#[test]
fn an_identical_resave_emits_nothing() {
    let (_tmp, repo, rx) = listened();
    let main = branch("main");
    let g = repo.write().unwrap();
    put(&g, &main, None, None, "a\n", 1).unwrap();
    assert_eq!(drain(&rx).len(), 1);
    put(&g, &main, None, None, "a\n", 1).unwrap();
    assert!(drain(&rx).is_empty());
}

#[test]
fn a_merge_parent_yields_saved_then_merged() {
    let (_tmp, repo, rx) = listened();
    let main = branch("main");
    let side = branch("side");
    let g = repo.write().unwrap();
    let a = put(&g, &main, None, None, "a\n", 1).unwrap();
    let b = put(&g, &side, Some(&a), None, "b\n", 2).unwrap();
    drain(&rx);
    let m = put(&g, &main, Some(&a), Some(&b), "m\n", 3).unwrap();
    let events = drain(&rx);
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], Event::Saved { snapshot, .. } if *snapshot == m));
    assert_eq!(
        events[1],
        Event::Merged {
            snapshot: m,
            into: main,
            ours: a,
            theirs: b
        }
    );
}

#[test]
fn merge_commit_emits_saved_and_merged_once() {
    let (_tmp, repo, rx) = listened();
    let g = repo.write().unwrap();
    let base = put(&g, &branch("base"), None, None, "1\n2\n3\n4\n5\n6\n7\n", 1).unwrap();
    let ours = put(
        &g,
        &branch("ours"),
        Some(&base),
        None,
        "ONE\n2\n3\n4\n5\n6\n7\n",
        2,
    )
    .unwrap();
    let theirs = put(
        &g,
        &branch("theirs"),
        Some(&base),
        None,
        "1\n2\n3\n4\n5\n6\nSEVEN\n",
        3,
    )
    .unwrap();
    drain(&rx);
    let target = branch("ours");
    let none: [(String, Resolution); 0] = [];
    let id = merge::commit(
        &g,
        MergeCommit {
            branch: &target,
            ours: &ours,
            theirs: &theirs,
            resolutions: &none,
            message: "merge",
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(10),
        },
    )
    .unwrap();
    let events = drain(&rx);
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(&events[0], Event::Saved { snapshot, .. } if *snapshot == id));
    assert!(matches!(&events[1], Event::Merged { snapshot, .. } if *snapshot == id));
}

#[test]
fn branch_refs_report_from_and_to() {
    let (_tmp, repo, rx) = listened();
    let main = branch("main");
    let g = repo.write().unwrap();
    let a = put(&g, &main, None, None, "a\n", 1).unwrap();
    let b = put(&g, &main, Some(&a), None, "b\n", 2).unwrap();
    drain(&rx);

    let topic = branch("topic");
    branches::create(&g, &topic, Some(&a)).unwrap();
    branches::set_tip(&g, &topic, &b).unwrap();
    branches::delete(&g, &topic).unwrap();
    let r = Ref::Branch(topic);
    assert_eq!(
        drain(&rx),
        vec![
            Event::RefMoved {
                reference: r.clone(),
                from: None,
                to: Some(a.clone())
            },
            Event::RefMoved {
                reference: r.clone(),
                from: Some(a),
                to: Some(b.clone())
            },
            Event::RefMoved {
                reference: r,
                from: Some(b),
                to: None
            },
        ]
    );
}

#[test]
fn tag_create_and_delete_emit_ref_moved() {
    let (_tmp, repo, rx) = listened();
    let g = repo.write().unwrap();
    let a = put(&g, &branch("main"), None, None, "a\n", 1).unwrap();
    drain(&rx);
    let name: TagName = "v1".parse().unwrap();
    tag::create(&g, &name, Some(&a), false).unwrap();
    tag::delete(&g, &name).unwrap();
    let r = Ref::Tag(name);
    assert_eq!(
        drain(&rx),
        vec![
            Event::RefMoved {
                reference: r.clone(),
                from: None,
                to: Some(a.clone())
            },
            Event::RefMoved {
                reference: r,
                from: Some(a),
                to: None
            },
        ]
    );
}

#[test]
fn bundle_apply_emits_one_imported() {
    let src_tmp = TempDir::new().unwrap();
    crate::commands::init::run(src_tmp.path()).unwrap();
    let src = Repo::open_and_migrate(src_tmp.path()).unwrap();
    {
        let g = src.write().unwrap();
        let a = put(&g, &branch("main"), None, None, "a\n", 1).unwrap();
        put(&g, &branch("main"), Some(&a), None, "b\n", 2).unwrap();
    }
    let file = src_tmp.path().join("out.velobundle");
    bundle::create(&src, &file, None).unwrap();

    let (_tmp2, repo, rx) = listened();
    let g = repo.write().unwrap();
    bundle::apply(&g, &file).unwrap();
    assert_eq!(drain(&rx), vec![Event::Imported { snapshots: 2 }]);
    // Idempotent re-apply imports nothing, so says nothing.
    bundle::apply(&g, &file).unwrap();
    assert!(drain(&rx).is_empty());
}

#[test]
fn a_failing_save_emits_nothing() {
    let (_tmp, repo, rx) = listened();
    let g = repo.write().unwrap();
    let ghost = SnapshotId::from_stored("0".repeat(64));
    let err = put(&g, &branch("main"), Some(&ghost), None, "a\n", 1).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "{err:?}");
    assert!(drain(&rx).is_empty());
}

#[test]
fn another_handle_is_not_heard() {
    let (tmp, repo, rx) = listened();
    let _first = &repo;
    let other = Repo::open_and_migrate(tmp.path()).unwrap();
    let g = other.write().unwrap();
    put(&g, &branch("main"), None, None, "a\n", 1).unwrap();
    assert!(drain(&rx).is_empty());
}

#[test]
fn save_run_emits_saved() {
    let (tmp, repo, rx) = listened();
    std::fs::write(tmp.path().join("a.txt"), "hello\n").unwrap();
    let g = repo.write().unwrap();
    let result = save::run(&g, Some("first"), save::Options::default())
        .unwrap()
        .into_result()
        .unwrap();
    let events = drain(&rx);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        matches!(&events[0], Event::Saved { snapshot, parent: None, .. } if *snapshot == result.hash)
    );
}

#[test]
fn repo_stays_send() {
    fn is_send<T: Send>() {}
    is_send::<Repo>();
}
