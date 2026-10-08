//! Chunked large objects and repository format v3 (14.7-b).

use crate::commands;
use crate::tree::{SaveTree, TreeEntry};
use crate::{db, BranchName, Error, ObjectHash, Repo, SnapshotId, SnapshotMeta};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fresh() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    Repo::init(&path).unwrap();
    (tmp, path)
}

/// Deterministic incompressible content from a seeded xorshift.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn save(repo: &Repo, parent: Option<&SnapshotId>, path: &str, content: Vec<u8>) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: "m",
            entries: vec![TreeEntry::file(path, content)],
            meta: SnapshotMeta::new(),
            timestamp_ms: Some(1),
            author: None,
            renames: &[],
        })
        .unwrap()
}

fn object_of(content: &[u8]) -> (String, ObjectHash) {
    let hex = blake3::hash(content).to_hex().to_string();
    (hex.clone(), hex.parse().unwrap())
}

fn count(dir: &Path) -> usize {
    fs::read_dir(dir).unwrap().count()
}

#[test]
fn a_large_object_round_trips_through_chunks_under_its_content_hash() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 7);
    let snap = save(&repo, None, "big.bin", content.clone());

    let (hex, obj) = object_of(&content);
    let raw = fs::read(root.join(".velo/objects").join(&hex)).unwrap();
    assert!(raw.starts_with(b"VELOCHK1"), "stored as a manifest");
    assert!(count(&root.join(".velo/chunks")) > 4);
    assert_eq!(repo.read_object(&obj).unwrap(), content);
    assert_eq!(repo.read_file_at(&snap, "big.bin").unwrap(), content);
    assert!(commands::fsck::check(&repo).unwrap().is_healthy());
}

#[test]
fn near_identical_large_objects_share_their_chunks() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let mut content = noise(2 * 1024 * 1024, 11);
    let s1 = save(&repo, None, "big.bin", content.clone());
    let first = count(&root.join(".velo/chunks"));
    content[1_000_000] ^= 0xff;
    content[1_000_001] ^= 0xff;
    let s2 = save(&repo, Some(&s1), "big.bin", content.clone());
    let total = count(&root.join(".velo/chunks"));
    assert!(
        total < first + first / 4,
        "second save added too many chunks: {first} -> {total}"
    );
    assert_eq!(repo.read_file_at(&s2, "big.bin").unwrap(), content);
}

#[test]
fn a_small_object_is_not_chunked() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(1024 * 1024 - 1, 3);
    save(&repo, None, "f.bin", content.clone());
    let (hex, _) = object_of(&content);
    let raw = fs::read(root.join(".velo/objects").join(hex)).unwrap();
    assert!(!raw.starts_with(b"VELOCHK1"));
    assert_eq!(count(&root.join(".velo/chunks")), 0);
}

#[test]
fn a_bundle_of_chunked_objects_applies_into_a_fresh_repository() {
    let (_ta, a) = fresh();
    let repo = Repo::open(&a).unwrap();
    let content = noise(2 * 1024 * 1024, 5);
    let snap = save(&repo, None, "big.bin", content.clone());

    let bd = TempDir::new().unwrap();
    let bundle = bd.path().join("out.velo");
    commands::bundle::create(&repo, &bundle, None).unwrap();

    let (_tb, b) = fresh();
    let other = Repo::open(&b).unwrap();
    commands::bundle::apply(&other.write().unwrap(), &bundle).unwrap();
    assert_eq!(other.read_file_at(&snap, "big.bin").unwrap(), content);
    let (hex, _) = object_of(&content);
    assert!(fs::read(b.join(".velo/objects").join(hex))
        .unwrap()
        .starts_with(b"VELOCHK1"));
    assert!(commands::fsck::check(&other).unwrap().is_healthy());
}

#[test]
fn a_deleted_chunk_is_an_error_not_a_panic() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 9);
    save(&repo, None, "big.bin", content.clone());
    let victim = fs::read_dir(root.join(".velo/chunks"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(victim).unwrap();
    let (_, obj) = object_of(&content);
    assert!(repo.read_object(&obj).is_err());
}

#[test]
fn a_truncated_or_garbled_manifest_is_corrupt() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 13);
    save(&repo, None, "big.bin", content.clone());
    let (hex, obj) = object_of(&content);
    let path = root.join(".velo/objects").join(&hex);
    let full = fs::read(&path).unwrap();
    for cut in [8, 12, 30, full.len() - 1] {
        fs::write(&path, &full[..cut]).unwrap();
        assert!(
            matches!(repo.read_object(&obj), Err(Error::Corrupt { .. })),
            "cut at {cut}"
        );
    }
    // A count claiming far more chunks than the file holds must not panic.
    let mut bad = full.clone();
    bad[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&path, bad).unwrap();
    assert!(matches!(repo.read_object(&obj), Err(Error::Corrupt { .. })));
}

#[test]
fn reordered_chunks_are_a_mismatch_in_verify_and_corrupt_in_fsck() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 17);
    save(&repo, None, "big.bin", content.clone());
    let (hex, obj) = object_of(&content);
    let path = root.join(".velo/objects").join(&hex);
    let mut raw = fs::read(&path).unwrap();
    // Entries start after magic (8) + version (4) + total (8) + count (4).
    let a: Vec<u8> = raw[24..60].to_vec();
    let b: Vec<u8> = raw[60..96].to_vec();
    assert_ne!(a, b);
    raw[24..60].copy_from_slice(&b);
    raw[60..96].copy_from_slice(&a);
    fs::write(&path, raw).unwrap();

    assert!(matches!(repo.read_object(&obj), Err(Error::Corrupt { .. })));
    let report = commands::fsck::check(&repo).unwrap();
    assert!(
        report.problems.iter().any(|p| matches!(
            p,
            commands::fsck::Problem::CorruptObject { hash, .. } if *hash == hex
        )),
        "fsck should report CorruptObject, got {:?}",
        report.problems
    );
}

#[test]
fn a_v2_repository_needs_migration_and_gains_the_chunks_directory() {
    let (_t, root) = fresh();
    {
        let conn = db::connect(&root.join(".velo/velo.db")).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
    }
    fs::remove_dir(root.join(".velo/chunks")).unwrap();
    assert!(matches!(
        Repo::open(&root),
        Err(Error::MigrationRequired {
            found: 2,
            supported: 3
        })
    ));
    let repo = Repo::open_and_migrate(&root).unwrap();
    assert!(root.join(".velo/chunks").is_dir());
    assert_eq!(repo.format_version().unwrap(), 3);
    assert_eq!(crate::FORMAT_VERSION, 3);
}

// ── gc and fsck over chunks (14.7-c) ─────────────────────────────────────

/// Remove every snapshot using `hash`, as `undo` followed by an expired trash
/// would, leaving the object unreferenced.
fn unreference(root: &Path, hash: &str) {
    let conn = db::connect(&root.join(".velo/velo.db")).unwrap();
    // The snapshots go too, or fsck would rightly find their ids unproven.
    conn.execute(
        "DELETE FROM snapshots WHERE hash IN
         (SELECT snapshot_hash FROM file_map WHERE hash = ?1)",
        [hash],
    )
    .unwrap();
    conn.execute("DELETE FROM file_map WHERE hash = ?", [hash])
        .unwrap();
}

fn gc_now(repo: &Repo) -> commands::gc::Collected {
    commands::gc::run(
        &repo.write().unwrap(),
        commands::gc::Options {
            keep_days: 0,
            ..Default::default()
        },
    )
    .unwrap()
}

fn chunk_files(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root.join(".velo/chunks"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

#[test]
fn gc_reclaims_the_chunks_of_an_unreachable_object() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 21);
    save(&repo, None, "big.bin", content.clone());
    let (hex, _) = object_of(&content);
    let before = chunk_files(&root).len();
    assert!(before > 4);

    unreference(&root, &hex);
    let collected = gc_now(&repo);
    assert_eq!(collected.objects, 1);
    assert_eq!(collected.chunks, before);
    assert!(!collected.is_empty());
    assert!(collected.bytes_freed > 1024 * 1024, "chunk bytes counted");
    assert!(chunk_files(&root).is_empty());
    assert!(commands::fsck::check(&repo).unwrap().is_healthy());
}

#[test]
fn gc_keeps_chunks_shared_with_a_surviving_object() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let mut content = noise(2 * 1024 * 1024, 23);
    let s1 = save(&repo, None, "big.bin", content.clone());
    let first = chunk_files(&root).len();
    let original = content.clone();
    content[1_000_000] ^= 0xff;
    content[1_000_001] ^= 0xff;
    save(&repo, Some(&s1), "other.bin", content.clone());
    let both = chunk_files(&root).len();
    assert!(both > first, "the edit must add at least one chunk");

    let (hex, _) = object_of(&content);
    unreference(&root, &hex);
    let collected = gc_now(&repo);
    assert_eq!(collected.objects, 1);
    assert_eq!(collected.chunks, both - first, "only the unique chunks go");
    assert_eq!(chunk_files(&root).len(), first);
    assert_eq!(repo.read_file_at(&s1, "big.bin").unwrap(), original);
    assert!(commands::fsck::check(&repo).unwrap().is_healthy());
}

#[test]
fn fsck_names_a_deleted_chunk() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 25);
    save(&repo, None, "big.bin", content.clone());
    let (hex, _) = object_of(&content);
    let victim = chunk_files(&root).remove(0);
    let name = victim.file_name().unwrap().to_string_lossy().to_string();
    fs::remove_file(victim).unwrap();

    let report = commands::fsck::check(&repo).unwrap();
    assert!(!report.is_healthy());
    assert!(
        report
            .problems
            .contains(&commands::fsck::Problem::MissingChunk {
                object: hex,
                chunk: name
            }),
        "got {:?}",
        report.problems
    );
}

#[test]
fn fsck_names_a_tampered_chunk() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    save(&repo, None, "big.bin", noise(2 * 1024 * 1024, 27));
    let victim = chunk_files(&root).remove(0);
    let name = victim.file_name().unwrap().to_string_lossy().to_string();
    let other = zstd::encode_all(&b"not the chunk"[..], 1).unwrap();
    fs::write(&victim, other).unwrap();

    let report = commands::fsck::check(&repo).unwrap();
    assert!(
        report.problems.iter().any(|p| matches!(
            p,
            commands::fsck::Problem::CorruptChunk { chunk, .. } if *chunk == name
        )),
        "got {:?}",
        report.problems
    );
    assert!(report
        .problems
        .iter()
        .all(|p| !matches!(p, commands::fsck::Problem::CorruptObject { .. })));
}

#[test]
fn an_orphan_chunk_is_cruft_that_repair_removes() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    save(&repo, None, "big.bin", noise(2 * 1024 * 1024, 29));
    let orphan = blake3::hash(b"orphan").to_hex().to_string();
    let frame = zstd::encode_all(&b"orphan"[..], 1).unwrap();
    fs::write(root.join(".velo/chunks").join(&orphan), frame).unwrap();

    let report = commands::fsck::check(&repo).unwrap();
    assert!(report.is_healthy(), "cruft is not corruption");
    assert_eq!(
        report.cruft,
        vec![commands::fsck::Cruft::UnreferencedChunks(1)]
    );

    let repaired = commands::fsck::repair(&repo.write().unwrap()).unwrap();
    assert_eq!(
        repaired.repaired,
        vec![commands::fsck::Cruft::UnreferencedChunks(1)]
    );
    assert!(!root.join(".velo/chunks").join(&orphan).exists());
    let after = commands::fsck::check(&repo).unwrap();
    assert!(after.cruft.is_empty() && after.is_healthy());
}

#[test]
fn a_cancelled_gc_leaves_fsck_healthy() {
    let (_t, root) = fresh();
    let repo = Repo::open(&root).unwrap();
    let content = noise(2 * 1024 * 1024, 31);
    save(&repo, None, "big.bin", content.clone());
    let (hex, _) = object_of(&content);
    unreference(&root, &hex);

    let cancel = crate::progress::Cancel::new();
    cancel.cancel();
    let err = commands::gc::run(
        &repo.write().unwrap(),
        commands::gc::Options {
            keep_days: 0,
            cancel: Some(&cancel),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled));
    assert!(commands::fsck::check(&repo).unwrap().is_healthy());
}
