//! The stat cache does not vouch for a file whose mtime is too fresh to tell
//! two writes apart (racy-clean). Every mtime here is set explicitly, so the
//! tests do not depend on the filesystem's timestamp granularity or on timing.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

use crate::commands;
use crate::Repo;

fn setup() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

fn write_at(root: &Path, rel: &str, content: &str, mtime: SystemTime) {
    let p = root.join(rel);
    fs::write(&p, content).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
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

fn cached(root: &Path, path: &str) -> bool {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row(
            "SELECT COUNT(*) FROM index_cache WHERE path = ?",
            [path],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

/// Two same-size writes carrying one mtime, the second after the first save:
/// the second save must store the second content.
#[test]
fn same_size_rewrite_with_the_same_fresh_mtime_is_saved() {
    let (_tmp, root) = setup();
    let fresh = SystemTime::now();
    write_at(&root, "a.txt", "a 4", fresh);
    let s1 = save(&root, "one");
    write_at(&root, "a.txt", "a 5", fresh);
    let s2 = save(&root, "two");
    assert_ne!(
        tree(&root, &s1).get("a.txt"),
        tree(&root, &s2).get("a.txt"),
        "second save kept the first content"
    );
    let repo = Repo::open_and_migrate(&root).unwrap();
    assert!(commands::get_dirty_files(&repo).is_empty());
}

/// The same through a scan that finds the file already tracked and cached.
#[test]
fn a_racy_file_gets_no_cache_row_and_an_old_one_is_dropped() {
    let (_tmp, root) = setup();
    let old = SystemTime::now() - Duration::from_secs(3600);
    write_at(&root, "a.txt", "a 4", old);
    save(&root, "one");
    assert!(cached(&root, "a.txt"), "an old file is cached");

    let fresh = SystemTime::now();
    write_at(&root, "a.txt", "a 5", fresh);
    let repo = Repo::open_and_migrate(&root).unwrap();
    assert!(commands::get_dirty_files(&repo).contains_key("a.txt"));
    assert!(!cached(&root, "a.txt"), "a racy file must not be cached");
}

#[test]
fn racy_mtime_boundaries() {
    let now = 10_000_000_000_i64;
    assert!(commands::racy_mtime(now, now));
    assert!(commands::racy_mtime(now + 1, now), "a future mtime is racy");
    assert!(commands::racy_mtime(now - 1_999_999_999, now));
    assert!(!commands::racy_mtime(now - 2_000_000_000, now));
    assert!(!commands::racy_mtime(0, now));
}

/// Seeding (what restore and a first save do) keeps a row for an old file and
/// drops the row for a fresh one.
#[test]
fn seeding_keeps_old_rows_and_skips_racy_ones() {
    let (_tmp, root) = setup();
    let repo = Repo::open_and_migrate(&root).unwrap();
    let ns = |t: SystemTime| t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as i64;
    let old = ns(SystemTime::now() - Duration::from_secs(3600));
    let fresh = ns(SystemTime::now());
    repo.conn()
        .execute(
            "INSERT INTO index_cache (path, mtime_ns, size, hash) VALUES ('fresh', 1, 1, 'stale')",
            [],
        )
        .unwrap();
    commands::seed_cache_entries(
        &repo,
        &[
            ("old".into(), old, 3, "h1".into()),
            ("fresh".into(), fresh, 3, "h2".into()),
        ],
    );
    drop(repo);
    assert!(cached(&root, "old"));
    assert!(!cached(&root, "fresh"), "the stale row must be dropped too");
}
