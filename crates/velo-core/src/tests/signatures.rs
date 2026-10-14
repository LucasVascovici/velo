//! Signed snapshots (`sign`): FORMAT.md 11.1 storage, verification and fsck.

use ed25519_dalek::Signer;
use tempfile::TempDir;

use crate::commands::fsck::{self, Cruft, Problem};
use crate::error::RefKind;
use crate::sign::{self, PublicKey, SigningKey, TrustedKeys};
use crate::tree::{SaveTree, TreeEntry};
use crate::{Error, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo, SnapshotId) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let branch = "main".parse().unwrap();
    let id = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent: None,
            merge_parent: None,
            message: "one",
            entries: vec![TreeEntry::file("a.txt", b"a".to_vec())],
            meta: SnapshotMeta::default(),
            author: None,
            timestamp_ms: Some(1),
            renames: &[],
        })
        .unwrap();
    (tmp, repo, id)
}

fn key(n: u8) -> SigningKey {
    SigningKey::from_seed([n; 32])
}

fn sql(tmp: &TempDir, stmt: &str) {
    rusqlite::Connection::open(tmp.path().join(".velo").join("velo.db"))
        .unwrap()
        .execute_batch(stmt)
        .unwrap();
}

#[test]
fn round_trip_with_a_fixed_seed() {
    let (_t, repo, id) = setup();
    let k = key(7);
    let row = sign::sign(&repo.write().unwrap(), &id, &k).unwrap();
    assert_eq!(row.algorithm, "ed25519");
    assert_eq!(row.public_key, k.public_key().to_string());
    assert_eq!(sign::signatures(&repo, &id).unwrap(), vec![row]);

    let trusted: TrustedKeys = [k.public_key()].into_iter().collect();
    let v = sign::verify(&repo, &id, &trusted).unwrap();
    assert!(v.is_trusted());
    assert_eq!(v.trusted, vec![k.public_key().to_string()]);
}

#[test]
fn message_bytes_match_a_signature_made_directly() {
    let (_t, repo, id) = setup();
    let mut expected = b"velo-signature-v1\n".to_vec();
    expected.extend_from_slice(id.as_str().as_bytes());
    assert_eq!(sign::message(&id), expected);

    let row = sign::sign(&repo.write().unwrap(), &id, &key(3)).unwrap();
    let direct = ed25519_dalek::SigningKey::from_bytes(&[3; 32]).sign(&expected);
    let hex: String = direct
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(row.signature, hex);
}

#[test]
fn signing_twice_leaves_one_row_and_unknown_is_not_found() {
    let (_t, repo, id) = setup();
    let k = key(1);
    let a = sign::sign(&repo.write().unwrap(), &id, &k).unwrap();
    let b = sign::sign(&repo.write().unwrap(), &id, &k).unwrap();
    assert_eq!(a, b);
    assert_eq!(sign::signatures(&repo, &id).unwrap().len(), 1);

    let ghost: SnapshotId = "0".repeat(64).parse().unwrap();
    let err = sign::sign(&repo.write().unwrap(), &ghost, &k).unwrap_err();
    assert!(matches!(
        err,
        Error::NotFound {
            kind: RefKind::Snapshot,
            ..
        }
    ));
}

#[test]
fn an_untrusted_key_is_valid_but_not_trusted() {
    let (_t, repo, id) = setup();
    sign::sign(&repo.write().unwrap(), &id, &key(1)).unwrap();
    sign::sign(&repo.write().unwrap(), &id, &key(2)).unwrap();
    let only_one: TrustedKeys = [key(1).public_key()].into_iter().collect();
    let v = sign::verify(&repo, &id, &only_one).unwrap();
    assert_eq!(v.trusted, vec![key(1).public_key().to_string()]);
    assert_eq!(v.untrusted, vec![key(2).public_key().to_string()]);
    assert!(v.is_trusted());

    let v = sign::verify(&repo, &id, &TrustedKeys::new()).unwrap();
    assert!(!v.is_trusted());
    assert_eq!(v.untrusted.len(), 2);
}

#[test]
fn a_tampered_signature_is_invalid_and_fsck_reports_it() {
    let (tmp, repo, id) = setup();
    let k = key(5);
    sign::sign(&repo.write().unwrap(), &id, &k).unwrap();
    drop(repo);
    sql(
        &tmp,
        &format!("UPDATE signatures SET signature = '{}'", "ab".repeat(64)),
    );
    let repo = Repo::open(tmp.path()).unwrap();
    let trusted: TrustedKeys = [k.public_key()].into_iter().collect();
    let v = sign::verify(&repo, &id, &trusted).unwrap();
    assert_eq!(v.invalid, vec![k.public_key().to_string()]);
    assert!(!v.is_trusted());

    let report = fsck::check(&repo).unwrap();
    assert!(report.problems.contains(&Problem::BadSignature {
        snapshot: id.as_str().to_string(),
        public_key: k.public_key().to_string(),
    }));
    assert!(!report.is_healthy());
}

#[test]
fn an_unknown_algorithm_is_unverifiable_and_never_valid() {
    let (tmp, repo, id) = setup();
    let k = key(9);
    sign::sign(&repo.write().unwrap(), &id, &k).unwrap();
    drop(repo);
    sql(&tmp, "UPDATE signatures SET algorithm = 'ed448'");
    let repo = Repo::open(tmp.path()).unwrap();
    let trusted: TrustedKeys = [k.public_key()].into_iter().collect();
    let v = sign::verify(&repo, &id, &trusted).unwrap();
    assert_eq!(v.unverifiable, vec![k.public_key().to_string()]);
    assert!(v.trusted.is_empty() && v.untrusted.is_empty() && v.invalid.is_empty());
    assert!(!v.is_trusted());
    assert!(fsck::check(&repo).unwrap().is_healthy());
}

#[test]
fn an_orphan_signature_is_cruft_that_repair_removes() {
    let (tmp, repo, id) = setup();
    sign::sign(&repo.write().unwrap(), &id, &key(1)).unwrap();
    drop(repo);
    // An unknown algorithm keeps the orphan from also being a bad signature.
    sql(
        &tmp,
        &format!(
            "INSERT INTO signatures SELECT '{}', 'other', public_key, signature FROM signatures",
            "f".repeat(64)
        ),
    );
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let report = fsck::check(&repo).unwrap();
    assert!(report.is_healthy());
    assert_eq!(report.cruft, vec![Cruft::OrphanSignatures(1)]);
    let report = fsck::repair(&repo.write().unwrap()).unwrap();
    assert_eq!(report.repaired, vec![Cruft::OrphanSignatures(1)]);
    assert!(fsck::check(&repo).unwrap().cruft.is_empty());
    assert_eq!(sign::signatures(&repo, &id).unwrap().len(), 1);
}

#[test]
fn signed_snapshots_lists_signed_ids() {
    let (_t, repo, id) = setup();
    assert!(sign::signed_snapshots(&repo).unwrap().is_empty());
    sign::sign(&repo.write().unwrap(), &id, &key(1)).unwrap();
    let set = sign::signed_snapshots(&repo).unwrap();
    assert_eq!(set.len(), 1);
    assert!(set.contains(&id));
}

#[test]
fn a_missing_table_reads_as_empty_through_open() {
    let (tmp, repo, id) = setup();
    drop(repo);
    sql(&tmp, "DROP TABLE signatures;");
    let repo = Repo::open(tmp.path()).unwrap();
    assert!(sign::signatures(&repo, &id).unwrap().is_empty());
    assert!(sign::signed_snapshots(&repo).unwrap().is_empty());
    let v = sign::verify(&repo, &id, &TrustedKeys::new()).unwrap();
    assert!(!v.is_trusted());
    assert!(fsck::check(&repo).unwrap().is_healthy());
    // Signing recreates it.
    sign::sign(&repo.write().unwrap(), &id, &key(1)).unwrap();
    assert_eq!(sign::signatures(&repo, &id).unwrap().len(), 1);
}

#[test]
fn keys_parse_and_the_secret_never_prints() {
    let hex = "01".repeat(32);
    let k = SigningKey::from_hex(&hex).unwrap();
    assert!(!format!("{k:?}").contains(&hex));
    assert!(format!("{k:?}").contains(&k.public_key().to_string()));
    assert!(matches!(
        SigningKey::from_hex("abc"),
        Err(Error::InvalidInput { .. })
    ));
    let pk: PublicKey = k.public_key().to_string().parse().unwrap();
    assert_eq!(pk, k.public_key());
    assert!("zz".parse::<PublicKey>().is_err());
}
