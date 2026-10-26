//! Signatures on the wire: FORMAT.md 9 (bundle version 4) and 11.1.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::commands::bundle::{self, SignatureRow};
use crate::commands::sync;
use crate::sign::{self, SigningKey, TrustedKeys};
use crate::transport::Spawn;
use crate::tree::{SaveTree, TreeEntry};
use crate::{Error, Repo, SnapshotId, SnapshotMeta};

fn key(n: u8) -> SigningKey {
    SigningKey::from_seed([n; 32])
}

fn trusting(k: &SigningKey) -> TrustedKeys {
    [k.public_key()].into_iter().collect()
}

fn init(path: &Path) -> Repo {
    fs::create_dir_all(path).unwrap();
    crate::commands::init::run(path).unwrap();
    Repo::open_and_migrate(path).unwrap()
}

fn save(repo: &Repo, parent: Option<&SnapshotId>, msg: &str, t: i64) -> SnapshotId {
    let branch = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: msg,
            entries: vec![TreeEntry::file("a.txt", msg.as_bytes().to_vec())],
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(t),
            renames: &[],
        })
        .unwrap()
}

/// A repository with two snapshots, both signed by `key(1)`.
fn signed_source(dir: &Path) -> (Repo, SnapshotId, SnapshotId) {
    let repo = init(dir);
    let a = save(&repo, None, "one", 1);
    let b = save(&repo, Some(&a), "two", 2);
    let k = key(1);
    let g = repo.write().unwrap();
    sign::sign(&g, &a, &k).unwrap();
    sign::sign(&g, &b, &k).unwrap();
    drop(g);
    (repo, a, b)
}

fn version_of(file: &Path) -> u32 {
    let raw = fs::read(file).unwrap();
    u32::from_le_bytes(raw[8..12].try_into().unwrap())
}

fn snapshot_count(repo: &Repo) -> i64 {
    repo.conn()
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn import(repo: &Repo, b: &bundle::Bundle) -> crate::Result<bundle::Imported> {
    bundle::import(&repo.write().unwrap(), b)
}

fn decoded(repo: &Repo, file: &Path) -> bundle::Bundle {
    bundle::decode(&fs::read(file).unwrap(), repo.limits()).unwrap()
}

fn bundle_of(src: &Repo, dir: &Path) -> bundle::Bundle {
    let file = dir.join("x.bundle");
    bundle::create(src, &file, None).unwrap();
    decoded(src, &file)
}

#[test]
fn an_unsigned_bundle_is_version_3_and_a_signed_one_is_4() {
    let tmp = TempDir::new().unwrap();
    let plain = init(&tmp.path().join("plain"));
    save(&plain, None, "one", 1);
    let f3 = tmp.path().join("plain.bundle");
    bundle::create(&plain, &f3, None).unwrap();
    assert_eq!(version_of(&f3), 3);

    let (signed, ..) = signed_source(&tmp.path().join("signed"));
    let f4 = tmp.path().join("signed.bundle");
    bundle::create(&signed, &f4, None).unwrap();
    assert_eq!(version_of(&f4), 4);
    assert_eq!(decoded(&signed, &f4).signatures.len(), 2);
}

#[test]
fn a_signed_history_survives_bundle_create_and_apply() {
    let tmp = TempDir::new().unwrap();
    let (src, a, b) = signed_source(&tmp.path().join("src"));
    let file = tmp.path().join("x.bundle");
    bundle::create(&src, &file, None).unwrap();

    let dst = init(&tmp.path().join("dst"));
    bundle::apply(&dst.write().unwrap(), &file).unwrap();
    let k = key(1);
    for id in [&a, &b] {
        assert!(sign::verify(&dst, id, &trusting(&k)).unwrap().is_trusted());
    }
}

#[test]
fn a_signed_history_survives_clone() {
    let tmp = TempDir::new().unwrap();
    let (_src, a, b) = signed_source(&tmp.path().join("src"));
    let copy = tmp.path().join("copy");
    sync::clone(
        tmp.path().join("src").to_str().unwrap(),
        &Spawn::new("velody"),
        sync::CloneOptions {
            dir: Some(copy.as_path()),
            ..Default::default()
        },
    )
    .unwrap();
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let k = key(1);
    for id in [&a, &b] {
        assert!(sign::verify(&repo, id, &trusting(&k)).unwrap().is_trusted());
    }
}

#[test]
fn fetch_over_a_local_remote_carries_signatures() {
    let tmp = TempDir::new().unwrap();
    let (_src, a, b) = signed_source(&tmp.path().join("src"));
    let dst = init(&tmp.path().join("dst"));
    {
        let g = dst.write().unwrap();
        crate::commands::remote::add(&g, "origin", tmp.path().join("src").to_str().unwrap())
            .unwrap();
        sync::fetch(&g, "origin", &Spawn::new("velody"), Default::default()).unwrap();
    }
    let k = key(1);
    for id in [&a, &b] {
        assert!(sign::verify(&dst, id, &trusting(&k)).unwrap().is_trusted());
    }
}

#[test]
fn one_bad_signature_rejects_the_whole_import() {
    let tmp = TempDir::new().unwrap();
    let (src, ..) = signed_source(&tmp.path().join("src"));
    let mut b = bundle_of(&src, tmp.path());
    let sig = &mut b.signatures[1].signature;
    let flipped = if sig.starts_with('0') { "1" } else { "0" };
    sig.replace_range(0..1, flipped);

    let dst = init(&tmp.path().join("dst"));
    let err = import(&dst, &b).err().expect("must refuse");
    assert!(matches!(err, Error::UntrustedData { .. }), "{err:?}");
    assert_eq!(snapshot_count(&dst), 0);
    for (hash, _) in &b.objects {
        assert!(!dst.objects().contains(hash), "object imported");
    }
}

#[test]
fn a_tampered_tree_is_still_refused_by_the_id_check() {
    let tmp = TempDir::new().unwrap();
    let (src, ..) = signed_source(&tmp.path().join("src"));
    let mut b = bundle_of(&src, tmp.path());
    b.file_map[0].mode += 1;

    let dst = init(&tmp.path().join("dst"));
    let err = import(&dst, &b).err().expect("must refuse");
    assert!(!matches!(err, Error::UntrustedData { .. }), "{err:?}");
    assert_eq!(snapshot_count(&dst), 0);
}

#[test]
fn dropping_the_section_imports_but_leaves_nothing_trusted() {
    let tmp = TempDir::new().unwrap();
    let (src, a, _b) = signed_source(&tmp.path().join("src"));
    let mut b = bundle_of(&src, tmp.path());
    b.signatures.clear();
    let v3 = bundle::encode(&b);
    assert_eq!(u32::from_le_bytes(v3[8..12].try_into().unwrap()), 3);
    let reread = bundle::decode(&v3, src.limits()).unwrap();

    let dst = init(&tmp.path().join("dst"));
    import(&dst, &reread).unwrap();
    assert_eq!(snapshot_count(&dst), 2);
    let v = sign::verify(&dst, &a, &trusting(&key(1))).unwrap();
    assert!(!v.is_trusted());
    assert!(v.trusted.is_empty());
}

#[test]
fn an_unknown_algorithm_is_stored_and_reported_unverifiable() {
    let tmp = TempDir::new().unwrap();
    let (src, a, _b) = signed_source(&tmp.path().join("src"));
    let mut b = bundle_of(&src, tmp.path());
    b.signatures.push(SignatureRow {
        snapshot_hash: a.to_string(),
        algorithm: "frobnicate-9".into(),
        public_key: "abcd".into(),
        signature: "1234".into(),
    });
    // It survives the wire encoding.
    let b = bundle::decode(&bundle::encode(&b), src.limits()).unwrap();

    let dst = init(&tmp.path().join("dst"));
    import(&dst, &b).unwrap();
    let v = sign::verify(&dst, &a, &trusting(&key(1))).unwrap();
    assert!(v.is_trusted());
    assert_eq!(v.unverifiable, vec!["abcd".to_string()]);
}

#[test]
fn rows_for_snapshots_the_receiver_lacks_are_dropped() {
    let tmp = TempDir::new().unwrap();
    let (src, a, b_id) = signed_source(&tmp.path().join("src"));
    let mut b = bundle_of(&src, tmp.path());
    // Leave the second snapshot out but keep its (valid) signature row.
    b.snapshots.retain(|s| s.hash != b_id.as_str());
    b.file_map.retain(|r| r.snapshot_hash != b_id.as_str());
    assert!(b
        .signatures
        .iter()
        .any(|s| s.snapshot_hash == b_id.as_str()));

    let dst = init(&tmp.path().join("dst"));
    import(&dst, &b).unwrap();
    assert_eq!(snapshot_count(&dst), 1);
    let signed = sign::signed_snapshots(&dst).unwrap();
    assert!(signed.contains(&a));
    assert!(!signed.contains(&b_id));
}
