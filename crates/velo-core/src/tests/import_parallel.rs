//! `bundle::import_pack`: parallel verification, exists-before-decode, and
//! borrowed-row snapshot ids, on both object backends.

use std::fs;
use tempfile::TempDir;

use crate::commands::bundle::{self, Bundle};
use crate::commands::{fsck, init, save};
use crate::{Error, InitOptions, ObjectLocation, Repo};

fn setup(location: ObjectLocation) -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    init::run_with(tmp.path(), InitOptions::new().objects(location)).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// 100 files across 20 snapshots, a few files changing in each.
fn history(repo: &Repo, root: &std::path::Path) {
    for i in 0..100 {
        fs::write(root.join(format!("f{i:03}.txt")), format!("file {i}\n")).unwrap();
    }
    for s in 0..20 {
        for i in (s * 5)..(s * 5 + 5) {
            fs::write(
                root.join(format!("f{i:03}.txt")),
                format!("file {i} rev {s}\n"),
            )
            .unwrap();
        }
        let g = repo.write().unwrap();
        save::run(&g, Some(&format!("snap {s}")), save::Options::default()).unwrap();
    }
}

fn source_bundle(location: ObjectLocation) -> (TempDir, Bundle) {
    let (tmp, repo) = setup(location);
    history(&repo, tmp.path());
    let file = tmp.path().join("out.velobundle");
    bundle::create(&repo, &file, None).unwrap();
    let b = bundle::decode(&fs::read(&file).unwrap(), &crate::Limits::default()).unwrap();
    (tmp, b)
}

fn snapshot_count(repo: &Repo) -> i64 {
    repo.conn()
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn both() -> [ObjectLocation; 2] {
    [ObjectLocation::Files, ObjectLocation::Database]
}

#[test]
fn import_into_fresh_repo_is_clean_and_idempotent() {
    for loc in both() {
        let (_src, b) = source_bundle(loc);
        let (_tmp, dest) = setup(loc);
        let g = dest.write().unwrap();
        let (snaps, objs) = bundle::import_pack(&g, &b).unwrap();
        assert_eq!(snaps, 20);
        assert!(objs > 100);
        let (snaps2, objs2) = bundle::import_pack(&g, &b).unwrap();
        assert_eq!((snaps2, objs2), (0, 0));
        drop(g);
        assert!(fsck::check(&dest).unwrap().problems.is_empty());
    }
}

#[test]
fn tampered_object_is_rejected_and_imports_no_snapshots() {
    for loc in both() {
        let (_src, mut b) = source_bundle(loc);
        let frame = zstd::encode_all(&b"not what the name says"[..], 1).unwrap();
        b.objects[50].1 = frame;
        let (_tmp, dest) = setup(loc);
        let g = dest.write().unwrap();
        let err = bundle::import_pack(&g, &b).unwrap_err();
        assert!(matches!(err, Error::Corrupt { .. }), "{err:?}");
        assert_eq!(snapshot_count(&dest), 0);
    }
}

#[test]
fn first_error_in_input_order_wins() {
    for loc in both() {
        let (_src, mut b) = source_bundle(loc);
        let bad = zstd::encode_all(&b"x"[..], 1).unwrap();
        let first = b.objects[10].0.clone();
        b.objects[10].1 = bad.clone();
        b.objects[80].1 = bad;
        let (_tmp, dest) = setup(loc);
        let g = dest.write().unwrap();
        let err = bundle::import_pack(&g, &b).unwrap_err();
        assert!(err.to_string().contains(&first), "{err}");
    }
}

#[test]
fn held_objects_are_not_decoded_again() {
    for loc in both() {
        let (_src, mut b) = source_bundle(loc);
        let (_tmp, dest) = setup(loc);
        let g = dest.write().unwrap();
        bundle::import_pack(&g, &b).unwrap();
        // Garbage under a name the store already holds is skipped, never
        // decoded; an object it lacked would have been rejected.
        for o in b.objects.iter_mut() {
            o.1 = vec![0xde, 0xad];
        }
        assert_eq!(bundle::import_pack(&g, &b).unwrap(), (0, 0));
    }
}

#[test]
fn mismatched_snapshot_id_is_still_corrupt() {
    for loc in both() {
        let (_src, mut b) = source_bundle(loc);
        b.snapshots[7].message.push_str(" (edited)");
        let (_tmp, dest) = setup(loc);
        let g = dest.write().unwrap();
        let err = bundle::import_pack(&g, &b).unwrap_err();
        assert!(matches!(err, Error::Corrupt { .. }), "{err:?}");
        assert_eq!(snapshot_count(&dest), 0);
    }
}

/// Every `file_map` row as a sorted set, plus the snapshot ids, so two
/// repositories can be compared.
type Row = (String, String, String, u32);

fn dump(repo: &Repo) -> (Vec<Row>, Vec<String>) {
    let conn = repo.conn();
    let rows = conn
        .prepare("SELECT snapshot_hash, path, hash, mode FROM file_map ORDER BY 1, 2, 3, 4")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let ids = conn
        .prepare("SELECT hash FROM snapshots ORDER BY hash")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    (rows, ids)
}

/// Objects are verified in parallel but written one at a time (17.8-a); a
/// multi-snapshot pack must still land exactly the rows, ids and objects it
/// carried, and importing it twice must change nothing.
#[test]
fn multi_snapshot_pack_yields_the_same_rows_ids_and_objects() {
    for loc in both() {
        let (src_tmp, src) = setup(loc);
        history(&src, src_tmp.path());
        let file = src_tmp.path().join("out.velobundle");
        bundle::create(&src, &file, None).unwrap();
        let b = bundle::decode(&fs::read(&file).unwrap(), &crate::Limits::default()).unwrap();

        let (_tmp, dest) = setup(loc);
        for _ in 0..2 {
            let g = dest.write().unwrap();
            bundle::import_pack(&g, &b).unwrap();
        }

        assert_eq!(dump(&src), dump(&dest));
        for (name, _) in &b.objects {
            assert!(dest.objects().contains(name), "object {name} missing");
        }
        assert!(fsck::check(&dest).unwrap().problems.is_empty());
    }
}
