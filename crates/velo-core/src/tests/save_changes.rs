//! `WriteGuard::save_changes`: a parent plus changes, the same snapshot
//! `save_tree` gives for the resulting tree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc;

use proptest::prelude::*;
use tempfile::TempDir;

use crate::commands::{paths, show};
use crate::events::Event;
use crate::tree::{SaveChanges, SaveTree, TreeEntry};
use crate::{BranchName, Error, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

fn base(repo: &Repo, files: &[(&str, &str)]) -> SnapshotId {
    let entries = files
        .iter()
        .map(|(p, c)| TreeEntry::file(*p, c.as_bytes().to_vec()))
        .collect();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main_branch(),
            parent: None,
            merge_parent: None,
            message: "base",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap()
}

fn snapshot_count(repo: &Repo) -> i64 {
    repo.conn()
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn invalid_naming(result: crate::Result<crate::tree::Saved>, path: &str) {
    match result {
        Err(Error::InvalidInput { detail }) => {
            assert!(detail.contains(path), "{detail} should name {path}")
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn delete_of_an_absent_path_is_refused_and_writes_nothing() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "a\n")]);
    let before = snapshot_count(&repo);
    let b = main_branch();
    let g = repo.write().unwrap();
    let spec = SaveChanges::new(&b, Some(&parent), "m").delete("nope.txt");
    invalid_naming(g.save_changes(spec), "nope.txt");
    assert_eq!(snapshot_count(&repo), before);
}

#[test]
fn rename_from_outside_the_parent_is_refused() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "a\n")]);
    let b = main_branch();
    let g = repo.write().unwrap();
    let spec = SaveChanges::new(&b, Some(&parent), "m")
        .put(TreeEntry::file("new.txt", b"n\n".to_vec()))
        .rename("new.txt", "other.txt");
    invalid_naming(g.save_changes(spec), "new.txt");
    // Moved twice: the second `from` is gone.
    let spec = SaveChanges::new(&b, Some(&parent), "m")
        .rename("a.txt", "b.txt")
        .rename("a.txt", "c.txt");
    invalid_naming(g.save_changes(spec), "a.txt");
    // Deleted first.
    let spec = SaveChanges::new(&b, Some(&parent), "m")
        .delete("a.txt")
        .rename("a.txt", "c.txt");
    invalid_naming(g.save_changes(spec), "a.txt");
    assert_eq!(snapshot_count(&repo), 1);
}

#[test]
fn rename_onto_an_existing_path_is_refused() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "a\n"), ("b.txt", "b\n")]);
    let b = main_branch();
    let g = repo.write().unwrap();
    let spec = SaveChanges::new(&b, Some(&parent), "m").rename("a.txt", "b.txt");
    invalid_naming(g.save_changes(spec), "b.txt");
    assert_eq!(snapshot_count(&repo), 1);
}

#[test]
fn an_unsafe_path_is_refused_before_any_object_is_written() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "a\n")]);
    let b = main_branch();
    let marker = b"never stored, unsafe path\n".to_vec();

    // The id of the marker object, learned in a scratch repository.
    let (_s, scratch) = setup();
    let hash = scratch
        .write()
        .unwrap()
        .save_changes(
            SaveChanges::new(&b, None, "m").put(TreeEntry::file("ok.txt", marker.clone())),
        )
        .unwrap()
        .written[0]
        .1
        .clone();

    let g = repo.write().unwrap();
    let spec = SaveChanges::new(&b, Some(&parent), "m")
        .put(TreeEntry::file("ok.txt", marker))
        .put(TreeEntry::file("../x", b"x\n".to_vec()));
    assert!(matches!(
        g.save_changes(spec),
        Err(Error::UnsafePath { .. })
    ));
    assert!(!repo.objects().contains(hash.as_str()));
    assert_eq!(snapshot_count(&repo), 1);
    let spec = SaveChanges::new(&b, Some(&parent), "m").rename("a.txt", "../y");
    assert!(matches!(
        g.save_changes(spec),
        Err(Error::UnsafePath { .. })
    ));
}

#[test]
fn a_missing_or_compacted_parent_is_distinguished() {
    let (_t, repo) = setup();
    let live = base(&repo, &[("a.txt", "a\n")]);
    let ghost: SnapshotId = "a".repeat(64).parse().unwrap();
    let b = main_branch();
    let g = repo.write().unwrap();
    assert!(matches!(
        g.save_changes(SaveChanges::new(&b, Some(&ghost), "m")),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        g.save_changes(SaveChanges::new(&b, Some(&live), "m").merge_parent(&ghost)),
        Err(Error::NotFound { .. })
    ));
    repo.conn()
        .execute(
            "INSERT INTO compactions (old_hash, new_hash, compacted_at_ms) VALUES (?, ?, 0)",
            [ghost.as_str(), live.as_str()],
        )
        .unwrap();
    assert!(matches!(
        g.save_changes(SaveChanges::new(&b, Some(&ghost), "m")),
        Err(Error::Compacted { .. })
    ));
}

#[test]
fn a_missing_stored_object_is_refused() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "a\n")]);
    let b = main_branch();
    let g = repo.write().unwrap();
    let absent: crate::ObjectHash = "b".repeat(64).parse().unwrap();
    let spec = SaveChanges::new(&b, Some(&parent), "m").put(TreeEntry::stored(
        "x.txt",
        absent,
        crate::tree::FileKind::Regular,
    ));
    assert!(matches!(
        g.save_changes(spec),
        Err(Error::MissingObject { .. })
    ));
}

#[test]
fn a_put_after_a_rename_replaces_the_content() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "old\n")]);
    let b = main_branch();
    let g = repo.write().unwrap();
    let saved = g
        .save_changes(
            SaveChanges::new(&b, Some(&parent), "m")
                .timestamp_ms(2)
                .rename("a.txt", "b.txt")
                .put(TreeEntry::file("b.txt", b"new\n".to_vec())),
        )
        .unwrap();
    assert_eq!(
        repo.read_file_at(&saved.snapshot, "b.txt").unwrap(),
        b"new\n"
    );
    assert!(repo.read_file_at(&saved.snapshot, "a.txt").is_err());
    assert_eq!(saved.written.len(), 1);
    assert_eq!(saved.written[0].0, "b.txt");
    let edges = paths::recorded_by(&repo, &saved.snapshot).unwrap();
    assert_eq!(edges, [(PathBuf::from("a.txt"), PathBuf::from("b.txt"))]);
}

#[test]
fn rename_edges_are_visible_and_keep_the_object() {
    let (_t, repo) = setup();
    let parent = base(&repo, &[("a.txt", "a\n")]);
    let before = repo.tree_at(&parent).unwrap();
    let b = main_branch();
    let g = repo.write().unwrap();
    let saved = g
        .save_changes(SaveChanges::new(&b, Some(&parent), "move").rename("a.txt", "d/b.txt"))
        .unwrap();
    assert!(saved.written.is_empty());
    let after = repo.tree_at(&saved.snapshot).unwrap();
    assert_eq!(after[0].path, "d/b.txt");
    assert_eq!(after[0].object, before[0].object);
    let edges = paths::recorded_by(&repo, &saved.snapshot).unwrap();
    assert_eq!(edges, [(PathBuf::from("a.txt"), PathBuf::from("d/b.txt"))]);
    let detail = show::run(&repo, &saved.snapshot, &[]).unwrap();
    assert!(format!("{detail:?}").contains("d/b.txt"));
}

#[test]
fn the_merge_parent_is_recorded() {
    let (_t, repo) = setup();
    let first = base(&repo, &[("a.txt", "a\n")]);
    let other = {
        let b: BranchName = "side".parse().unwrap();
        repo.write()
            .unwrap()
            .save_changes(
                SaveChanges::new(&b, Some(&first), "side")
                    .timestamp_ms(2)
                    .put(TreeEntry::file("s.txt", b"s\n".to_vec())),
            )
            .unwrap()
            .snapshot
    };
    let b = main_branch();
    let saved = repo
        .write()
        .unwrap()
        .save_changes(
            SaveChanges::new(&b, Some(&first), "merge")
                .merge_parent(&other)
                .timestamp_ms(3),
        )
        .unwrap();
    let stored: String = repo
        .conn()
        .query_row(
            "SELECT merge_parent FROM snapshots WHERE hash = ?",
            [&saved.snapshot],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, other.as_str());
}

#[test]
fn crlf_in_a_put_is_normalised_like_save_tree() {
    let (_t, a) = setup();
    let (_u, c) = setup();
    let b = main_branch();
    let saved = a
        .write()
        .unwrap()
        .save_changes(
            SaveChanges::new(&b, None, "m")
                .timestamp_ms(5)
                .put(TreeEntry::file("f.txt", b"x\r\ny\r\n".to_vec())),
        )
        .unwrap();
    let tree_id = c
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &b,
            parent: None,
            merge_parent: None,
            message: "m",
            entries: vec![TreeEntry::file("f.txt", b"x\r\ny\r\n".to_vec())],
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(5),
            renames: &[],
        })
        .unwrap();
    assert_eq!(saved.snapshot, tree_id);
    assert_eq!(a.read_file_at(&saved.snapshot, "f.txt").unwrap(), b"x\ny\n");
}

#[test]
fn one_saved_event_per_real_save_and_a_resave_is_the_same_id() {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let (tx, rx) = mpsc::channel();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap().listening(tx);
    let b = main_branch();
    let g = repo.write().unwrap();
    let spec = || {
        SaveChanges::new(&b, None, "m")
            .timestamp_ms(9)
            .put(TreeEntry::file("f.txt", b"f\n".to_vec()))
    };
    let first = g.save_changes(spec()).unwrap();
    let again = g.save_changes(spec()).unwrap();
    assert_eq!(first, again);
    let events: Vec<Event> = rx.try_iter().collect();
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], Event::Saved { snapshot, .. } if *snapshot == first.snapshot));
}

// ---- parity with save_tree ---------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Put(usize, Vec<u8>),
    PutStored(usize, Vec<u8>),
    Delete(usize),
    Rename(usize, usize),
}

fn pool_path(i: usize) -> String {
    // Nested and top-level names that can never collide as file and directory.
    let (dir, file) = (i / 4 % 4, i % 4);
    if dir == 0 {
        format!("top{file}.txt")
    } else {
        format!("d{dir}/e{}/f{file}.txt", dir % 2)
    }
}

fn content() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop::sample::select(vec![b'a', b'b', b'\n', b' ']), 0..12)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..16, content()).prop_map(|(i, c)| Op::Put(i, c)),
        (0usize..16, content()).prop_map(|(i, c)| Op::PutStored(i, c)),
        (0usize..16).prop_map(Op::Delete),
        (0usize..16, 0usize..16).prop_map(|(a, b)| Op::Rename(a, b)),
    ]
}

fn tree_save(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    tree: &BTreeMap<String, Vec<u8>>,
    renames: &[(PathBuf, PathBuf)],
    ts: i64,
) -> SnapshotId {
    let entries = tree
        .iter()
        .map(|(p, c)| TreeEntry::file(p.clone(), c.clone()))
        .collect();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main_branch(),
            parent,
            merge_parent: None,
            message: "m",
            entries,
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames,
        })
        .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn save_changes_matches_save_tree(
        seed in prop::collection::btree_map(0usize..16, content(), 1..40),
        ops in prop::collection::vec(op(), 0..14),
    ) {
        let (_a, ra) = setup();
        let (_b, rb) = setup();
        let mut tree: BTreeMap<String, Vec<u8>> =
            seed.iter().map(|(i, c)| (pool_path(*i), c.clone())).collect();
        let parent_paths: BTreeSet<String> = tree.keys().cloned().collect();
        let pa = tree_save(&ra, None, &tree, &[], 1);
        let pb = tree_save(&rb, None, &tree, &[], 1);
        prop_assert_eq!(&pa, &pb);

        let branch = main_branch();
        let mut spec = SaveChanges::new(&branch, Some(&pa), "m").timestamp_ms(2);
        let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut puts: BTreeSet<String> = BTreeSet::new();
        for op in ops {
            match op {
                Op::Put(i, c) => {
                    let p = pool_path(i);
                    spec = spec.put(TreeEntry::file(p.clone(), c.clone()));
                    tree.insert(p.clone(), c);
                    puts.insert(p);
                }
                Op::PutStored(i, c) => {
                    let p = pool_path(i);
                    let object = ra.objects().put(&crate::storage::normalise_crlf(c.clone())).unwrap();
                    spec = spec.put(TreeEntry::stored(
                        p.clone(),
                        crate::ObjectHash::from_stored(object),
                        crate::tree::FileKind::Regular,
                    ));
                    tree.insert(p.clone(), c);
                    puts.insert(p);
                }
                Op::Delete(i) => {
                    let p = pool_path(i);
                    if tree.remove(&p).is_some() {
                        puts.remove(&p);
                        spec = spec.delete(p);
                    }
                }
                Op::Rename(i, j) => {
                    let (from, to) = (pool_path(i), pool_path(j));
                    if parent_paths.contains(&from)
                        && tree.contains_key(&from)
                        && !tree.contains_key(&to)
                    {
                        let c = tree.remove(&from).unwrap();
                        tree.insert(to.clone(), c);
                        if puts.remove(&from) {
                            puts.insert(to.clone());
                        }
                        renames.push((PathBuf::from(&from), PathBuf::from(&to)));
                        spec = spec.rename(from, to);
                    }
                }
            }
        }
        // An edge whose target was deleted later is refused on both paths.
        prop_assume!(renames.iter().all(|(_, to)| tree.contains_key(to.to_str().unwrap())));

        let saved = ra.write().unwrap().save_changes(spec).unwrap();
        // The stored objects of the stored puts exist in repo B too once the
        // tree is saved there, so the comparison is on whole-tree ids.
        let want = tree_save(&rb, Some(&pb), &tree, &renames, 2);
        prop_assert_eq!(&saved.snapshot, &want);
        let ta = ra.tree_at(&saved.snapshot).unwrap();
        prop_assert_eq!(&ta, &rb.tree_at(&want).unwrap());

        let listed: Vec<&str> = saved.written.iter().map(|(p, _)| p.as_str()).collect();
        let expected: Vec<&str> = puts.iter().map(String::as_str).collect();
        prop_assert_eq!(listed, expected);
        for (path, object) in &saved.written {
            let file = ta.iter().find(|f| &f.path == path).unwrap();
            prop_assert_eq!(&file.object, object);
        }
    }
}
