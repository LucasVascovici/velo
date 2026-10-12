//! Restore seeds `index_cache` for the files it writes (15.5-a).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands;
use crate::Repo;

fn setup() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(p, content).unwrap();
}

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    f(&guard)
}

fn save(root: &Path, msg: &str) -> String {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .unwrap()
    .hash
    .into_string()
}

fn dirty(root: &Path) -> HashMap<String, commands::FileStatus> {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::get_dirty_files(&repo)
}

fn cache_rows(root: &Path) -> HashMap<String, (i64, i64, String)> {
    let repo = Repo::open_and_migrate(root).unwrap();
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT path, mtime_ns, size, hash FROM index_cache")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn tree(root: &Path, snapshot: &str) -> HashMap<String, String> {
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

/// Every path of `snapshot` has a cache row matching both the tree and the disk.
fn assert_seeded(root: &Path, snapshot: &str) {
    let rows = cache_rows(root);
    let files = tree(root, snapshot);
    assert!(!files.is_empty());
    for (path, hash) in files {
        let (mtime, size, cached) = rows
            .get(&path)
            .unwrap_or_else(|| panic!("no cache row for {path}"));
        assert_eq!(cached, &hash, "hash for {path}");
        let meta = fs::symlink_metadata(root.join(&path)).unwrap();
        assert_eq!(*size, meta.len() as i64, "size for {path}");
        let on_disk = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        assert_eq!(*mtime, on_disk, "mtime for {path}");
    }
}

fn two_saves(root: &Path) -> (String, String) {
    write(root, "a.txt", "one");
    write(root, "dir/b.txt", "bee");
    let s1 = save(root, "s1");
    write(root, "a.txt", "two two");
    write(root, "dir/b.txt", "bee!");
    let s2 = save(root, "s2");
    (s1, s2)
}

#[test]
fn undo_seeds_cache_and_status_is_clean() {
    let (_tmp, root) = setup();
    let (s1, _) = two_saves(&root);
    with_write(&root, commands::undo::run).unwrap();
    assert_seeded(&root, &s1);
    assert!(dirty(&root).is_empty());
}

#[test]
fn redo_seeds_cache() {
    let (_tmp, root) = setup();
    let (_, s2) = two_saves(&root);
    with_write(&root, commands::undo::run).unwrap();
    with_write(&root, commands::redo::run).unwrap();
    assert_seeded(&root, &s2);
    assert!(dirty(&root).is_empty());
}

#[test]
fn switch_seeds_cache() {
    let (_tmp, root) = setup();
    let (_, s2) = two_saves(&root);
    with_write(&root, |g| commands::switch::run(g, "other", false)).unwrap();
    write(&root, "c.txt", "see");
    save(&root, "s3");
    with_write(&root, |g| commands::switch::run(g, "main", false)).unwrap();
    assert_seeded(&root, &s2);
    assert!(dirty(&root).is_empty());
}

#[test]
fn modified_after_restore_is_still_detected() {
    let (_tmp, root) = setup();
    two_saves(&root);
    with_write(&root, commands::undo::run).unwrap();
    write(&root, "a.txt", "a much longer body than before");
    let d = dirty(&root);
    assert!(d.contains_key("a.txt"), "{d:?}");
    assert!(!d.contains_key("dir/b.txt"));
}

#[test]
fn failed_restore_leaves_no_stale_row_for_the_failed_path() {
    let (_tmp, root) = setup();
    let (s1, _) = two_saves(&root);
    // Warm the cache for the current (s2) content.
    assert!(dirty(&root).is_empty());
    // Replace a.txt with a directory so writing the s1 content over it fails.
    fs::remove_file(root.join("a.txt")).unwrap();
    fs::create_dir(root.join("a.txt")).unwrap();
    fs::write(root.join("a.txt").join("x"), "x").unwrap();
    let r = with_write(&root, |g| {
        commands::restore::run(
            g,
            &crate::SnapshotId::from_stored(s1.clone()),
            commands::restore::Options {
                force: true,
                ..Default::default()
            },
        )
    });
    assert!(r.is_err(), "restore should fail on the obstructed path");
    let rows = cache_rows(&root);
    assert!(!rows.contains_key("a.txt"), "stale row for failed path");
    // The file that did land is seeded with the s1 hash.
    assert_eq!(
        rows.get("dir/b.txt").map(|r| &r.2),
        tree(&root, &s1).get("dir/b.txt")
    );
}
