//! Import writes deltas where it can (the first parent is stored here, and the
//! chain stays within K), keyframes otherwise, and never duplicates rows for a
//! hash it already has rows for.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

use crate::commands::bundle::{self, Bundle};
use crate::commands::{self, redo, sync, undo};
use crate::transport::Spawn;
use crate::tree_store;
use crate::Repo;

/// A repository with its working directory, at keyframe interval `k`.
struct Side {
    _holder: TempDir,
    root: PathBuf,
    repo: Repo,
}

fn side(k: u32) -> Side {
    let holder = TempDir::new().unwrap();
    let root = holder.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    commands::init::run(&root).unwrap();
    let mut repo = Repo::open_and_migrate(&root).unwrap();
    repo.set_keyframe_interval(k);
    Side {
        _holder: holder,
        root,
        repo,
    }
}

/// A clone of `src` whose receiving side runs at K = `k`: its working tree
/// matches its tip, so `undo` is allowed.
fn cloned(src: &Side, k: u32) -> Side {
    crate::repo::set_test_keyframe_interval(k);
    let holder = TempDir::new().unwrap();
    let root = holder.path().join("copy");
    sync::clone(
        src.root.to_str().unwrap(),
        &Spawn::new("velody"),
        sync::CloneOptions {
            dir: Some(root.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut repo = Repo::open_and_migrate(&root).unwrap();
    repo.set_keyframe_interval(k);
    Side {
        _holder: holder,
        root,
        repo,
    }
}

fn save(s: &Side, file: &str, content: &str, msg: &str) -> String {
    fs::write(s.root.join(file), content).unwrap();
    let guard = s.repo.write().unwrap();
    commands::save::run(&guard, Some(msg), commands::save::Options::default())
        .unwrap()
        .into_result()
        .expect("snapshot")
        .hash
        .into_string()
}

/// A chain of `n` snapshots, each changing a different file and one shared one.
fn chain(s: &Side, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            fs::write(s.root.join("shared.txt"), format!("v{i}\n")).unwrap();
            save(
                s,
                &format!("f{i}.txt"),
                &format!("file {i}\n"),
                &format!("c{i}"),
            )
        })
        .collect()
}

fn pack(s: &Side, ids: &[&String]) -> Bundle {
    let set: HashSet<String> = ids.iter().map(|i| (*i).clone()).collect();
    bundle::build_pack(s.repo.conn(), &s.repo.objects(), &set).unwrap()
}

fn import(s: &Side, b: &Bundle) {
    let guard = s.repo.write().unwrap();
    bundle::import(&guard, b).unwrap();
}

fn base_of(s: &Side, id: &str) -> Option<String> {
    s.repo
        .conn()
        .query_row(
            "SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?",
            [id],
            |r| r.get(0),
        )
        .ok()
}

fn counts(s: &Side) -> (i64, i64, i64) {
    let n = |t: &str| -> i64 {
        s.repo
            .conn()
            .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
            .unwrap()
    };
    (n("file_map"), n("tree_deltas"), n("tree_delta_rows"))
}

fn is_keyframe(s: &Side, id: &str) -> bool {
    base_of(s, id).is_none() && tree_store::has_tree(s.repo.conn(), id).unwrap()
}

#[test]
fn a_bundle_whose_first_parent_is_local_becomes_a_delta() {
    let src = side(128);
    let ids = chain(&src, 2);
    let dst = side(128);
    import(&dst, &pack(&src, &[&ids[0]]));
    assert!(is_keyframe(&dst, &ids[0]));
    import(&dst, &pack(&src, &[&ids[1]]));
    assert_eq!(base_of(&dst, &ids[1]).as_deref(), Some(ids[0].as_str()));
    assert_eq!(
        tree_store::tree_of(dst.repo.conn(), &ids[1]).unwrap(),
        tree_store::tree_of(src.repo.conn(), &ids[1]).unwrap()
    );
}

#[test]
fn a_missing_first_parent_gives_a_keyframe() {
    let src = side(128);
    let ids = chain(&src, 2);
    let dst = side(128);
    import(&dst, &pack(&src, &[&ids[1]]));
    assert!(is_keyframe(&dst, &ids[1]));
    assert_eq!(counts(&dst).1, 0);
}

fn chain_respects_k(reverse: bool) {
    let src = side(1);
    let ids = chain(&src, 4);
    let dst = side(3);
    let mut b = pack(&src, &ids.iter().collect::<Vec<_>>());
    if reverse {
        b.snapshots.reverse();
    }
    import(&dst, &b);
    assert!(is_keyframe(&dst, &ids[0]));
    assert_eq!(base_of(&dst, &ids[1]).as_deref(), Some(ids[0].as_str()));
    assert_eq!(base_of(&dst, &ids[2]).as_deref(), Some(ids[1].as_str()));
    assert!(is_keyframe(&dst, &ids[3]));
    for id in &ids {
        assert_eq!(
            tree_store::tree_of(dst.repo.conn(), id).unwrap(),
            tree_store::tree_of(src.repo.conn(), id).unwrap()
        );
    }
}

#[test]
fn a_chain_in_one_bundle_respects_k() {
    chain_respects_k(false);
}

#[test]
fn a_chain_listed_child_before_parent_is_imported_the_same() {
    chain_respects_k(true);
}

fn duplicates(s: &Side) -> usize {
    tree_store::duplicate_rows(s.repo.conn()).unwrap()
}

#[test]
fn a_locally_trashed_hash_is_replaced_without_duplicates_and_redo_restores_it() {
    for k in [128, 2] {
        let src = side(128);
        let ids = chain(&src, 2);
        let dst = cloned(&src, k);
        {
            let g = dst.repo.write().unwrap();
            undo::run(&g).unwrap();
        }
        assert!(tree_store::has_tree(dst.repo.conn(), &ids[1]).unwrap());
        import(&dst, &pack(&src, &[&ids[0], &ids[1]]));
        assert_eq!(duplicates(&dst), 0, "K={k}");
        let out = {
            let g = dst.repo.write().unwrap();
            redo::run(&g).unwrap()
        };
        assert_eq!(out.snapshot, ids[1]);
        assert_eq!(
            tree_store::tree_of(dst.repo.conn(), &ids[1]).unwrap(),
            tree_store::tree_of(src.repo.conn(), &ids[1]).unwrap()
        );
        assert_eq!(duplicates(&dst), 0, "K={k}");
    }
}

#[test]
fn replacing_a_trashed_hash_with_a_dependant_delta_does_not_fail() {
    let src = side(128);
    let ids = chain(&src, 3);
    let dst = cloned(&src, 128);
    assert_eq!(base_of(&dst, &ids[2]).as_deref(), Some(ids[1].as_str()));
    {
        let g = dst.repo.write().unwrap();
        undo::run(&g).unwrap();
        undo::run(&g).unwrap();
    }
    // ids[1] is trashed with its rows, and ids[2] (trashed too) is a delta on it.
    import(&dst, &pack(&src, &[&ids[0], &ids[1]]));
    assert_eq!(duplicates(&dst), 0);
    let mut restored = Vec::new();
    for _ in 0..2 {
        let g = dst.repo.write().unwrap();
        restored.push(redo::run(&g).unwrap().snapshot.to_string());
    }
    restored.sort();
    let mut want = vec![ids[1].clone(), ids[2].clone()];
    want.sort();
    assert_eq!(restored, want);
    for id in &ids {
        assert_eq!(
            tree_store::tree_of(dst.repo.conn(), id).unwrap(),
            tree_store::tree_of(src.repo.conn(), id).unwrap()
        );
    }
    assert_eq!(duplicates(&dst), 0);
}

#[test]
fn importing_the_same_bundle_twice_writes_no_rows() {
    let src = side(128);
    let ids = chain(&src, 4);
    let dst = side(3);
    let b = pack(&src, &ids.iter().collect::<Vec<_>>());
    import(&dst, &b);
    let before = counts(&dst);
    assert!(before.1 > 0, "some snapshots are deltas");
    import(&dst, &b);
    assert_eq!(counts(&dst), before);
    assert_eq!(duplicates(&dst), 0);
}

/// Export lists rows, snapshots and objects in no particular order (and objects
/// come from a hash set), so two exports are compared after sorting; the bytes
/// of the sorted bundle must then match exactly.
fn canonical(mut b: Bundle) -> Vec<u8> {
    b.snapshots.sort_by(|x, y| x.hash.cmp(&y.hash));
    b.file_map
        .sort_by(|x, y| (&x.snapshot_hash, &x.path).cmp(&(&y.snapshot_hash, &y.path)));
    b.meta.sort_by(|x, y| {
        (&x.snapshot_id, &x.namespace, &x.key).cmp(&(&y.snapshot_id, &y.namespace, &y.key))
    });
    b.tags.sort();
    b.objects.sort_by(|x, y| x.0.cmp(&y.0));
    b.renames.sort_by(|x, y| {
        (&x.snapshot_hash, &x.from_path, &x.to_path).cmp(&(
            &y.snapshot_hash,
            &y.from_path,
            &y.to_path,
        ))
    });
    bundle::encode(&b)
}

#[test]
fn a_bundle_from_delta_storage_encodes_identically_to_one_from_keyframes() {
    for k in [2, 3] {
        let delta = side(k);
        let ids = chain(&delta, 6);
        let refs = ids.iter().collect::<Vec<_>>();
        assert!(counts(&delta).1 > 0, "K={k} stores deltas");
        let from_delta = canonical(pack(&delta, &refs));

        // The same history stored whole: import it into a K=1 repository.
        let whole = side(1);
        import(&whole, &pack(&delta, &refs));
        assert_eq!(counts(&whole).1, 0);
        let from_whole = canonical(pack(&whole, &refs));
        assert_eq!(from_delta, from_whole, "K={k}");
    }
}
