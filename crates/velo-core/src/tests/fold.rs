//! The fold primitive `tree_store::remove_trees` (ARCHITECTURE 20.6): removing
//! stored trees never changes a surviving tree, never lengthens a chain past
//! K, and never leaves a delta whose base is gone. Also the trash rule: undo,
//! save, gc and redo in every order.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;

use proptest::prelude::*;
use tempfile::TempDir;

use crate::commands;
use crate::sign::{self, SigningKey, TrustedKeys};
use crate::tree_store::{self, StoredTree};
use crate::Repo;

/// A repository at keyframe interval `k` with `n` real saves; each edits
/// `a.txt` and adds its own `f<i>.txt`, so a delta has two rows.
fn build(k: u32, n: usize) -> (TempDir, Repo, Vec<String>) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let mut repo = Repo::open_and_migrate(tmp.path()).unwrap();
    repo.set_keyframe_interval(k);
    let mut ids = Vec::new();
    for i in 0..n {
        save_one(&repo, tmp.path(), i);
        ids.push(tip(&repo));
    }
    (tmp, repo, ids)
}

fn save_one(repo: &Repo, root: &std::path::Path, i: usize) -> Option<String> {
    fs::write(root.join("a.txt"), format!("edit {i}\n")).unwrap();
    fs::write(root.join(format!("f{i}.txt")), format!("file {i}\n")).unwrap();
    let guard = repo.write().unwrap();
    commands::save::run(&guard, Some("s"), commands::save::Options::default())
        .ok()?
        .into_result()
        .map(|r| r.hash.into_string())
}

fn tip(repo: &Repo) -> String {
    repo.conn()
        .query_row(
            "SELECT hash FROM snapshots ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn depths(conn: &rusqlite::Connection) -> HashMap<String, u32> {
    let links: HashMap<String, String> =
        tree_store::delta_links(conn).unwrap().into_iter().collect();
    let mut out = HashMap::new();
    for s in links.keys() {
        let (mut cur, mut d) = (s.as_str(), 0);
        while let Some(b) = links.get(cur) {
            cur = b;
            d += 1;
            assert!(d < 10_000, "cycle at {s}");
        }
        out.insert(s.clone(), d);
    }
    out
}

fn assert_no_dangling(conn: &rusqlite::Connection) {
    for (s, base) in tree_store::delta_links(conn).unwrap() {
        assert!(
            tree_store::has_tree(conn, &base).unwrap(),
            "{s} is based on {base}, which has no tree"
        );
    }
}

// ---------------------------------------------------------------------------
// Property: random histories and removal sets.
// ---------------------------------------------------------------------------

const PATHS: usize = 6;

fn tree_strategy() -> impl Strategy<Value = Vec<Option<u8>>> {
    prop::collection::vec(prop::option::of(0u8..3), PATHS)
}

fn materialize(t: &[Option<u8>]) -> Vec<(String, String, i64)> {
    let mut rows: Vec<(String, String, i64)> = t
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|v| (format!("p{i}"), format!("h{v}"), 0)))
        .collect();
    // A tree with no entries has no stored rows at all, which is not a tree.
    if rows.is_empty() {
        rows.push(("p0".into(), "h0".into(), 0));
    }
    rows
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_fold_never_changes_a_surviving_tree_or_grows_a_chain(
        k in 2u32..=5,
        history in prop::collection::vec((0usize..100, tree_strategy()), 2..14),
        mask in prop::collection::vec(any::<bool>(), 14),
    ) {
        crate::repo::keyframes_only();
        let tmp = TempDir::new().unwrap();
        commands::init::run(tmp.path()).unwrap();
        let repo = Repo::open_and_migrate(tmp.path()).unwrap();
        let conn = repo.conn();
        let n = history.len();
        let id = |i: usize| format!("s{i:02}");
        // A node's first parent: a root now and then, a merge's second parent
        // never matters because deltas follow the first only.
        let parent: Vec<Option<usize>> = history
            .iter()
            .enumerate()
            .map(|(i, (sel, _))| if i == 0 || sel % 7 == 0 { None } else { Some(sel % i) })
            .collect();
        let tx = conn.unchecked_transaction().unwrap();
        // Readers insist that a base is a snapshot or trash row; trash rows
        // are the cheapest way to make the synthetic ids known.
        for i in 0..n {
            tx.execute(
                "INSERT INTO trash (hash, message, branch) VALUES (?, 'm', 'main')",
                [id(i)],
            )
            .unwrap();
        }
        for (i, (_, t)) in history.iter().enumerate() {
            let rows = materialize(t);
            let refs: Vec<(&str, &str, i64)> =
                rows.iter().map(|(p, h, m)| (p.as_str(), h.as_str(), *m)).collect();
            let p = parent[i].map(id);
            tree_store::write_tree(&tx, &id(i), p.as_deref(), None, &refs, k).unwrap();
        }
        let removed: Vec<usize> = (0..n).filter(|i| mask[*i]).collect();
        let gone: HashSet<usize> = removed.iter().copied().collect();
        let before: BTreeMap<usize, StoredTree> = (0..n)
            .filter(|i| !gone.contains(i))
            .map(|i| (i, tree_store::tree_of_unchecked(&tx, &id(i)).unwrap()))
            .collect();
        // Compaction's remapping: a removed snapshot's children go to its first
        // surviving ancestor.
        let remap = |s: &str| -> Option<String> {
            let mut cur = parent[s[1..].parse::<usize>().unwrap()];
            while let Some(c) = cur {
                if !gone.contains(&c) {
                    return Some(id(c));
                }
                cur = parent[c];
            }
            None
        };
        let ids: Vec<String> = removed.iter().map(|i| id(*i)).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        tree_store::remove_trees(&tx, &refs, &remap, k).unwrap();
        for (i, tree) in &before {
            prop_assert_eq!(&tree_store::tree_of_unchecked(&tx, &id(*i)).unwrap(), tree);
        }
        for (s, d) in depths(&tx) {
            prop_assert!(d < k, "{s} is {d} deep at K={k}");
        }
        for (s, base) in tree_store::delta_links(&tx).unwrap() {
            prop_assert!(!ids.contains(&base), "{s} is based on removed {base}");
        }
        for i in &ids {
            prop_assert!(!tree_store::has_tree(&tx, i).unwrap());
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic folds.
// ---------------------------------------------------------------------------

#[test]
fn a_fold_writes_a_delta_against_the_surviving_parent() {
    let (_t, repo, ids) = build(4, 4);
    let c = repo.conn();
    // s0 kf, s1 on s0, s2 on s1, s3 on s2.
    assert_eq!(depths(c)[&ids[3]], 3);
    let want = tree_store::tree_of(c, &ids[3]).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    let parent = ids[0].clone();
    tree_store::remove_trees(&tx, &[&ids[1], &ids[2]], &|_| Some(parent.clone()), 4).unwrap();
    tx.commit().unwrap();
    assert_eq!(tree_store::tree_of(c, &ids[3]).unwrap(), want);
    let links: HashMap<_, _> = tree_store::delta_links(c).unwrap().into_iter().collect();
    assert_eq!(links[&ids[3]], ids[0]);
    assert_no_dangling(c);
}

#[test]
fn a_fold_with_no_surviving_parent_writes_a_keyframe() {
    let (_t, repo, ids) = build(4, 3);
    let c = repo.conn();
    let want = tree_store::tree_of(c, &ids[1]).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    tree_store::remove_trees(&tx, &[&ids[0]], &|_| None, 4).unwrap();
    tx.commit().unwrap();
    assert_eq!(tree_store::tree_of(c, &ids[1]).unwrap(), want);
    assert!(!depths(c).contains_key(&ids[1]));
    assert_no_dangling(c);
}

#[test]
fn materialize_dependants_still_makes_a_set_deletable() {
    let (_t, repo, ids) = build(4, 3);
    let c = repo.conn();
    let want = tree_store::tree_of(c, &ids[1]).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    tree_store::materialize_dependants(&tx, &[&ids[0]]).unwrap();
    tree_store::delete_trees(&tx, &[&ids[0]]).unwrap();
    tx.commit().unwrap();
    assert_eq!(tree_store::tree_of(c, &ids[1]).unwrap(), want);
}

#[test]
fn gc_folds_an_orphan_base_instead_of_skipping_it() {
    let (_t, repo, ids) = build(3, 3);
    let c = repo.conn();
    let want = tree_store::tree_of(c, &ids[2]).unwrap();
    assert_eq!(depths(c)[&ids[2]], 2);
    // s1 stops being a snapshot (and is not in trash): an orphan tree that s2
    // is based on.
    c.execute("DELETE FROM snapshots WHERE hash = ?", [&ids[1]])
        .unwrap();
    let collected = commands::gc::run(
        &repo.write().unwrap(),
        commands::gc::Options {
            keep_days: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(collected.orphan_file_map > 0);
    assert!(!tree_store::has_tree(c, &ids[1]).unwrap());
    assert_eq!(tree_store::tree_of(c, &ids[2]).unwrap(), want);
    let links: HashMap<_, _> = tree_store::delta_links(c).unwrap().into_iter().collect();
    assert_eq!(links.get(&ids[2]), Some(&ids[0]));
    assert_eq!(tree_store::orphan_delta_snapshots(c).unwrap(), 0);
}

#[test]
fn gc_leaves_an_orphan_whose_own_base_is_missing_untouched() {
    let (_t, repo, ids) = build(3, 3);
    let c = repo.conn();
    // s0's rows vanish and s0 and s1 stop being snapshots: s1 is an orphan
    // delta whose base has nothing stored, and s2 is based on s1.
    c.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [&ids[0]])
        .unwrap();
    for i in [0, 1] {
        c.execute("DELETE FROM snapshots WHERE hash = ?", [&ids[i]])
            .unwrap();
    }
    let dump = |c: &rusqlite::Connection| -> Vec<(String, String, Option<String>)> {
        let mut s = c
            .prepare(
                "SELECT d.snapshot_hash, d.base_hash, r.path || '=' || COALESCE(r.hash, '')
                   FROM tree_deltas d LEFT JOIN tree_delta_rows r
                     ON r.snapshot_hash = d.snapshot_hash
                  ORDER BY 1, 3",
            )
            .unwrap();
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    let before = dump(c);
    let _ = commands::gc::run(
        &repo.write().unwrap(),
        commands::gc::Options {
            keep_days: 0,
            ..Default::default()
        },
    );
    assert_eq!(dump(c), before, "gc rewrote a tree it could not read");
}

#[test]
fn a_signed_snapshot_keeps_its_tree_and_signature_through_a_fold() {
    let (_t, repo, ids) = build(4, 4);
    let c = repo.conn();
    let key = SigningKey::from_seed([9; 32]);
    let signed = crate::SnapshotId::from_stored(&ids[3]);
    sign::sign(&repo.write().unwrap(), &signed, &key).unwrap();
    let want = tree_store::tree_of(c, &ids[3]).unwrap();
    let tx = c.unchecked_transaction().unwrap();
    let parent = ids[0].clone();
    tree_store::remove_trees(&tx, &[&ids[1], &ids[2]], &|_| Some(parent.clone()), 4).unwrap();
    tx.commit().unwrap();
    assert_eq!(tree_store::tree_of(c, &ids[3]).unwrap(), want);
    let trusted: TrustedKeys = [key.public_key()].into_iter().collect();
    assert!(sign::verify(&repo, &signed, &trusted).unwrap().is_trusted());
}

// ---------------------------------------------------------------------------
// The trash rule: undo, save, gc, redo in every order, at K = 2.
// ---------------------------------------------------------------------------

fn permutations(items: &mut Vec<u8>, from: usize, out: &mut Vec<Vec<u8>>) {
    if from == items.len() {
        out.push(items.clone());
        return;
    }
    for i in from..items.len() {
        items.swap(from, i);
        permutations(items, from + 1, out);
        items.swap(from, i);
    }
}

fn all_trees(conn: &rusqlite::Connection) -> BTreeMap<String, StoredTree> {
    let mut stmt = conn
        .prepare("SELECT hash FROM snapshots UNION SELECT hash FROM trash")
        .unwrap();
    let hashes: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    hashes
        .into_iter()
        .filter(|h| tree_store::has_tree(conn, h).unwrap())
        .map(|h| {
            let t = tree_store::tree_of(conn, &h).unwrap();
            (h, t)
        })
        .collect()
}

#[test]
fn undo_save_gc_redo_in_every_order_keeps_ids_and_trees() {
    let mut orders = Vec::new();
    permutations(&mut vec![0, 1, 2, 3], 0, &mut orders);
    assert_eq!(orders.len(), 24);
    for order in orders {
        // s0 keyframe, s1 a delta on it, s2 a keyframe again at K = 2.
        let (tmp, repo, _ids) = build(2, 3);
        let known = all_trees(repo.conn());
        let mut next = 3;
        // The hash undo trashed, its tree beforehand, and whether a keep_days=0
        // gc has run since (that expires the trash row by design).
        let mut trashed: Option<(String, StoredTree)> = None;
        let mut gc_since_undo = false;
        for step in &order {
            match step {
                0 => {
                    let o = commands::undo::run(&repo.write().unwrap())
                        .unwrap_or_else(|e| panic!("{order:?}: undo failed: {e}"));
                    let t = tree_store::tree_of(repo.conn(), &o.snapshot).unwrap();
                    trashed = Some((o.snapshot, t));
                    gc_since_undo = false;
                }
                1 => {
                    // A save on the branch starts new history and clears its
                    // trash, so a later redo has nothing to restore.
                    let saved = save_one(&repo, tmp.path(), next);
                    assert!(saved.is_some(), "{order:?}: save failed");
                    trashed = None;
                    next += 1;
                }
                2 => {
                    commands::gc::run(
                        &repo.write().unwrap(),
                        commands::gc::Options {
                            keep_days: 0,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    gc_since_undo = true;
                }
                _ => {
                    let res = commands::redo::run(&repo.write().unwrap());
                    match trashed.take() {
                        Some((hash, tree)) if !gc_since_undo => {
                            let o = res.unwrap_or_else(|e| panic!("{order:?}: redo failed: {e}"));
                            assert_eq!(o.snapshot, hash, "{order:?}: redo restored another id");
                            assert_eq!(
                                tree_store::tree_of(repo.conn(), &o.snapshot).unwrap(),
                                tree,
                                "{order:?}: redone tree differs"
                            );
                        }
                        _ => assert!(res.is_err(), "{order:?}: redo should have failed"),
                    }
                }
            }
            let c = repo.conn();
            assert_no_dangling(c);
            for (h, d) in depths(c) {
                assert!(d < 2, "{order:?}: {h} is {d} deep");
            }
            // Every original snapshot that still exists reads exactly as before.
            for (h, t) in all_trees(c) {
                if let Some(old) = known.get(&h) {
                    assert_eq!(&h_tree(&t), &h_tree(old), "{order:?}: {h} changed");
                }
            }
        }
    }
}

fn h_tree(t: &StoredTree) -> Vec<(String, String, i64)> {
    t.iter()
        .map(|(p, (h, m))| (p.clone(), h.clone(), *m))
        .collect()
}

// ---------------------------------------------------------------------------
// Compaction folds through the primitive.
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn compaction_keeps_every_tree_and_bounds_chains(
        k in 2u32..=3,
        n in 7usize..14,
        spans in prop::collection::vec((0usize..14, 1usize..4), 1..3),
    ) {
        let (_tmp, repo, ids) = build(k, n);
        let before: HashMap<String, StoredTree> = ids
            .iter()
            .map(|i| (i.clone(), tree_store::tree_of(repo.conn(), i).unwrap()))
            .collect();
        // Disjoint ranges of at least two snapshots, oldest first.
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut floor = 0;
        let mut sorted = spans.clone();
        sorted.sort();
        for (lo, len) in sorted {
            let lo = lo.max(floor) % (n - 1);
            let hi = (lo + len).min(n - 1);
            if lo < floor || hi <= lo {
                continue;
            }
            ranges.push((lo, hi));
            floor = hi + 1;
        }
        prop_assume!(!ranges.is_empty());
        let sids: Vec<crate::SnapshotId> = ids
            .iter()
            .map(|i| crate::SnapshotId::from_stored(i.clone()))
            .collect();
        let rs: Vec<commands::compact::Range> = ranges
            .iter()
            .map(|(lo, hi)| commands::compact::Range { oldest: &sids[*lo], newest: &sids[*hi] })
            .collect();
        let branch: crate::BranchName = "main".parse().unwrap();
        let done = {
            let guard = repo.write().unwrap();
            commands::compact::run(&guard, &branch, &rs, commands::compact::Options::default())
                .unwrap()
        };
        let conn = repo.conn();
        // Every snapshot that was neither squashed away nor re-minted is where
        // it was; every new id carries the tree of the one it replaces.
        let mut replaced: HashSet<String> = HashSet::new();
        for s in &done.squashed {
            let newest = s.removed.last().unwrap().as_str().to_string();
            for r in &s.removed {
                replaced.insert(r.as_str().to_string());
            }
            prop_assert_eq!(
                &tree_store::tree_of(conn, s.into.as_str()).unwrap(),
                &before[&newest]
            );
        }
        for (old, new) in &done.rewritten {
            replaced.insert(old.as_str().to_string());
            prop_assert_eq!(
                &tree_store::tree_of(conn, new.as_str()).unwrap(),
                &before[old.as_str()]
            );
        }
        for (i, tree) in &before {
            if !replaced.contains(i) {
                prop_assert_eq!(&tree_store::tree_of(conn, i).unwrap(), tree);
            }
        }
        for (s, d) in depths(conn) {
            prop_assert!(d < k, "{s} is {d} deep at K={k}");
        }
        assert_no_dangling(conn);
    }
}
