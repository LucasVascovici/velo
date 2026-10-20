//! Save reuses the dirty scan's hashes (15.7-d): one read per new object, and
//! none for an object that already exists.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands;
use crate::Repo;

fn setup_files() -> (TempDir, PathBuf) {
    crate::repo::keyframes_only();
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

fn setup_db() -> (TempDir, PathBuf) {
    crate::repo::keyframes_only();
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    commands::init::run_with(
        &path,
        crate::InitOptions::new().objects(crate::ObjectLocation::Database),
    )
    .unwrap();
    (tmp, path)
}

fn save(root: &Path, msg: &str) -> String {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    commands::save::run(&guard, Some(msg), commands::save::Options::default())
        .unwrap()
        .into_result()
        .unwrap()
        .hash
        .into_string()
}

fn tree_hashes(root: &Path, snapshot: &str) -> HashMap<String, String> {
    let repo = Repo::open_and_migrate(root).unwrap();
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")
        .unwrap();
    stmt.query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn read_count(path: &Path) -> usize {
    crate::storage::KNOWN_READS
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.as_path() == path)
        .count()
}

fn blake(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex().to_string()
}

fn healthy(root: &Path) -> bool {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::fsck::check(&repo).unwrap().is_healthy()
}

fn a_b_a(root: &Path) {
    let f = root.join("f.txt");
    fs::write(&f, "alpha\n").unwrap();
    save(root, "A");
    fs::write(&f, "bravo\n").unwrap();
    save(root, "B");
    // Back to content whose object already exists: the scan has its hash, so
    // the store must not read the file again.
    fs::write(&f, "alpha\n").unwrap();
    let before = read_count(&f);
    let id = save(root, "A again");
    assert_eq!(read_count(&f), before, "an existing object is not re-read");
    assert_eq!(tree_hashes(root, &id)["f.txt"], blake("alpha\n"));
    assert!(healthy(root));
}

#[test]
fn reverting_to_existing_content_does_not_read_the_file_files_backend() {
    let (_t, root) = setup_files();
    a_b_a(&root);
}

#[test]
fn reverting_to_existing_content_does_not_read_the_file_database_backend() {
    let (_t, root) = setup_db();
    a_b_a(&root);
}

#[test]
fn a_new_object_is_read_once() {
    let (_t, root) = setup_files();
    let f = root.join("new.txt");
    fs::write(&f, "fresh content\n").unwrap();
    let id = save(&root, "new");
    assert_eq!(read_count(&f), 1);
    assert_eq!(tree_hashes(&root, &id)["new.txt"], blake("fresh content\n"));
}

fn stale_cache(root: &Path) {
    let f = root.join("f.txt");
    fs::write(&f, "alpha\n").unwrap();
    save(root, "A");
    fs::write(&f, "bravo\n").unwrap();
    {
        // Warm the cache with the file's real mtime and size, then make its
        // hash a lie naming an object that does not exist.
        let repo = Repo::open_and_migrate(root).unwrap();
        assert!(!commands::get_dirty_files(&repo).is_empty());
        let fake = "ab".repeat(32);
        repo.conn()
            .execute(
                "UPDATE index_cache SET hash = ? WHERE path = 'f.txt'",
                [fake],
            )
            .unwrap();
    }
    let id = save(root, "B");
    assert_eq!(tree_hashes(root, &id)["f.txt"], blake("bravo\n"));
    assert!(healthy(root));
}

#[test]
fn a_stale_cached_hash_is_stored_under_the_real_hash_files_backend() {
    let (_t, root) = setup_files();
    stale_cache(&root);
}

#[test]
fn a_stale_cached_hash_is_stored_under_the_real_hash_database_backend() {
    let (_t, root) = setup_db();
    stale_cache(&root);
}

#[test]
fn dirty_scan_hashes_modified_files_and_leaves_new_ones_to_save() {
    let (_t, root) = setup_files();
    fs::write(root.join("keep.txt"), "keep\n").unwrap();
    fs::write(root.join("gone.txt"), "gone\n").unwrap();
    fs::write(root.join("edit.txt"), "one\n").unwrap();
    save(&root, "base");
    fs::write(root.join("edit.txt"), "two\n").unwrap();
    fs::write(root.join("add.txt"), "added\n").unwrap();
    fs::remove_file(root.join("gone.txt")).unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    let scan = commands::dirty_scan(&repo);
    assert_eq!(scan.status.len(), 3);
    // A new file is New whatever its content, so the scan does not read it;
    // save hashes it in the same read that stores it (19.6-a).
    assert_eq!(scan.hashes.len(), 1);
    assert!(scan.unhashed.contains_key("add.txt"));
    assert_eq!(scan.hashes["edit.txt"], blake("two\n"));
}
