//! Read limits: no bytes from another machine allocate past a ceiling.
//!
//! Each case here is something a hostile bundle, wire stream or git stream can
//! say in a handful of bytes, and each must end as a typed `LimitExceeded` (or
//! the truncated error for a lying count), never as an allocation.

use std::time::{Duration, Instant};
use tempfile::TempDir;

use crate::commands::bundle::{self, Bundle, FileMapRow, MetaRow, SnapshotRow};
use crate::commands::{import, snapshot_id_of};
use crate::limits::Limit;
use crate::{Error, Limits, Repo, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn empty() -> Bundle {
    Bundle {
        snapshots: vec![],
        file_map: vec![],
        meta: vec![],
        tags: vec![],
        objects: vec![],
        renames: vec![],
    }
}

fn assert_limit<T: std::fmt::Debug>(result: crate::Result<T>, want: Limit) -> (u64, Option<u64>) {
    match result {
        Err(Error::LimitExceeded { limit, max, found }) => {
            assert_eq!(limit, want);
            (max, found)
        }
        other => panic!("expected LimitExceeded {want:?}, got {other:?}"),
    }
}

/// A one-snapshot bundle with a valid id, carrying `meta_value` as metadata.
fn one_snapshot(source: &Repo, meta_value: &str) -> Bundle {
    let object = source.objects().put(b"payload\n").unwrap();
    let frame = source.objects().compressed(&object).unwrap();
    let mut meta = SnapshotMeta::default();
    if !meta_value.is_empty() {
        meta.insert_stored("ns".into(), "k".into(), meta_value.into());
    }
    let id = snapshot_id_of([("a.txt", object.as_str(), 0)], "", "", "m", 1, &meta);
    Bundle {
        snapshots: vec![SnapshotRow {
            hash: id.clone(),
            message: "m".into(),
            branch: "main".into(),
            parent_hash: String::new(),
            merge_parent: String::new(),
            created_at_ms: 1,
        }],
        file_map: vec![FileMapRow {
            snapshot_hash: id.clone(),
            path: "a.txt".into(),
            hash: object.clone(),
            mode: 0,
        }],
        meta: if meta_value.is_empty() {
            vec![]
        } else {
            vec![MetaRow {
                snapshot_id: id,
                namespace: "ns".into(),
                key: "k".into(),
                value: meta_value.into(),
            }]
        },
        tags: vec![],
        objects: vec![(object, frame)],
        renames: vec![],
    }
}

fn write_bundle(dir: &TempDir, bytes: &[u8]) -> std::path::PathBuf {
    let file = dir.path().join("b.velobundle");
    std::fs::write(&file, bytes).unwrap();
    file
}

#[test]
fn a_zstd_bomb_is_refused_and_imports_nothing() {
    let zeros = vec![0u8; 64 << 20];
    let hash = blake3::hash(&zeros).to_hex().to_string();
    let frame = zstd::encode_all(&zeros[..], 1).unwrap();
    assert!(frame.len() < 64 << 10, "64 MiB of zeros is a tiny frame");
    let mut b = empty();
    b.objects.push((hash, frame));
    let dir = TempDir::new().unwrap();
    let file = write_bundle(&dir, &bundle::encode(&b));

    let (_tmp, repo) = setup();
    let repo = repo.limited(Limits::default().with_max_object_bytes(1 << 20));
    let result = {
        let guard = repo.write().unwrap();
        bundle::apply(&guard, &file)
    };
    let (max, _) = assert_limit(result, Limit::ObjectBytes);
    assert_eq!(max, 1 << 20);
    assert_eq!(repo.objects().list().unwrap().len(), 0);
}

#[test]
fn bounded_decoding_catches_a_frame_that_hides_its_size() {
    // A streamed frame states no content size, so only the read cap can stop it.
    let zeros = vec![0u8; 4 << 20];
    let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
    std::io::Write::write_all(&mut enc, &zeros).unwrap();
    let frame = enc.finish().unwrap();
    assert_limit(
        crate::storage::decode_bounded(&frame, 1 << 20),
        Limit::ObjectBytes,
    );
    assert_eq!(
        crate::storage::decode_bounded(&frame, 4 << 20)
            .unwrap()
            .len(),
        4 << 20
    );
}

#[test]
fn a_lying_section_count_fails_fast_without_allocating() {
    let mut bytes = bundle::encode(&empty());
    bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes.truncate(16);
    bytes.extend_from_slice(&[0u8; 10]);
    let started = Instant::now();
    let result = bundle::decode(&bytes, &Limits::default()).map(|_| ());
    assert!(
        matches!(result, Err(Error::InvalidInput { .. })),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_huge_string_length_in_a_bundle_is_a_limit_not_an_allocation() {
    let (_tmp, source) = setup();
    let mut bytes = bundle::encode(&one_snapshot(&source, ""));
    // Past the magic, version and snapshot count: the first string's prefix.
    bytes[16..20].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    let (max, found) = assert_limit(
        bundle::decode(&bytes, &Limits::default()).map(|_| ()),
        Limit::StringBytes,
    );
    assert_eq!(max, 16 << 20);
    assert_eq!(found, Some(0xFFFF_FFFF));
}

#[test]
fn an_oversized_meta_value_is_refused_under_default_limits() {
    let (_tmp, source) = setup();
    let big = "x".repeat(20 << 20);
    let bytes = bundle::encode(&one_snapshot(&source, &big));
    assert_limit(
        bundle::decode(&bytes, &Limits::default()).map(|_| ()),
        Limit::StringBytes,
    );
    // The ceiling is the handle's to move.
    let raised = Limits::default().with_max_string_bytes(32 << 20);
    assert!(bundle::decode(&bytes, &raised).is_ok());
}

#[test]
fn a_bundle_file_over_the_pack_limit_is_refused_before_it_is_read() {
    let (_tmp, source) = setup();
    let bytes = bundle::encode(&one_snapshot(&source, ""));
    let dir = TempDir::new().unwrap();
    let file = write_bundle(&dir, &bytes);

    let (_tmp, dest) = setup();
    let dest = dest.limited(Limits::default().with_max_pack_bytes(10));
    let result = {
        let guard = dest.write().unwrap();
        bundle::apply(&guard, &file)
    };
    let (max, found) = assert_limit(result, Limit::PackBytes);
    assert_eq!((max, found), (10, Some(bytes.len() as u64)));
}

#[test]
fn the_wire_reader_refuses_a_huge_length_prefix() {
    let wire = 0xFFFF_FFFFu32.to_le_bytes();
    let result = crate::transport::read_string_opt(&mut std::io::Cursor::new(&wire[..]), 16 << 20);
    assert_limit(result, Limit::StringBytes);
}

#[test]
fn git_import_refuses_a_data_length_past_the_object_limit() {
    let (_tmp, repo) = setup();
    let stream = "blob\nmark :1\ndata 99999999999\n";
    let guard = repo.write().unwrap();
    let result = import::git_fast_export(&guard, &mut stream.as_bytes(), Default::default());
    let (_, found) = assert_limit(result, Limit::ObjectBytes);
    assert_eq!(found, Some(99_999_999_999));
}

#[test]
fn git_import_bounds_an_unterminated_line() {
    let (_tmp, repo) = setup();
    let repo = repo.limited(Limits::default().with_max_string_bytes(64));
    let stream = "x".repeat(10_000);
    let guard = repo.write().unwrap();
    let result = import::git_fast_export(&guard, &mut stream.as_bytes(), Default::default());
    assert_limit(result, Limit::StringBytes);
}

#[test]
fn a_normal_bundle_still_applies_under_default_limits() {
    let (_tmp, source) = setup();
    let dir = TempDir::new().unwrap();
    let file = write_bundle(&dir, &bundle::encode(&one_snapshot(&source, "small")));

    let (_tmp, dest) = setup();
    let applied = {
        let guard = dest.write().unwrap();
        bundle::apply(&guard, &file).unwrap()
    };
    assert_eq!(applied.snapshots, 1);
    assert_eq!(dest.limits(), &Limits::default());
}
