pub mod branches;
pub mod cherry_pick;
pub mod diff;
pub mod gc;
pub mod init;
pub mod logs;
pub mod merge;
pub mod redo;
pub mod resolve;
pub mod restore;
pub mod save;
pub mod show;
pub mod stash;
pub mod status;
pub mod switch;
pub mod tag;
pub mod undo;

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ignore::{WalkBuilder, WalkState};
use parking_lot::Mutex;
use rayon::prelude::*;

use crate::error::{Result, VeloError};

/// Number of hex characters used for snapshot hashes (48 bits of entropy).
pub const SNAP_HASH_LEN: usize = 12;

// ─── File status ─────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Clone)]
pub enum FileStatus {
    New,
    Modified,
    Deleted,
}

// ─── Cache entry (from index_cache table) ─────────────────────────────────────

struct CacheEntry {
    mtime_ns: i64,
    size: i64,
    hash: String,
}

// ─── Repository discovery ─────────────────────────────────────────────────────

/// Walk upward from `start` until `.velo/` is found.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".velo").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve a user-supplied snapshot identifier (tag name or hash prefix).
pub fn resolve_snapshot_id(root: &Path, input: &str) -> Result<String> {
    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // 1. Try as tag name
    if let Ok(h) = conn.query_row(
        "SELECT snapshot_hash FROM tags WHERE name = ?",
        [input],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(h);
    }

    // 2. Try as exact or prefix hash
    let rows: Vec<String> = {
        let mut stmt = conn.prepare("SELECT hash FROM snapshots WHERE hash LIKE ? || '%'")?;
        let collected: Vec<String> = stmt
            .query_map([input], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    match rows.len() {
        0 => Err(VeloError::InvalidInput(format!(
            "No snapshot or tag found matching '{}'.",
            input
        ))),
        1 => Ok(rows.into_iter().next().unwrap()),
        n => Err(VeloError::InvalidInput(format!(
            "Ambiguous prefix '{}' matches {} snapshots. Use more characters.",
            input, n
        ))),
    }
}

// ─── Filesystem enumeration ───────────────────────────────────────────────────

/// Collected file entry from the parallel walk.
struct WalkEntry {
    path: PathBuf,
    mtime_ns: i64,
    size: i64,
}

/// Build a `WalkBuilder` with the standard ignore rules applied.
fn make_walker(root: &Path) -> WalkBuilder {
    let mut b = WalkBuilder::new(root);
    b.hidden(false)
        .add_custom_ignore_filename(".veloignore")
        .add_custom_ignore_filename(".gitignore");
    b.filter_entry(|e| {
        let n = e.file_name().to_str().unwrap_or("");
        n != ".velo" && n != ".git" && n != "target"
    });
    b
}

/// Return all tracked file paths under `root` using the parallel walker.
pub fn get_tracked_files(root: &Path) -> Vec<PathBuf> {
    let acc: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    make_walker(root).build_parallel().run(|| {
        Box::new(|res| {
            if let Ok(e) = res {
                if e.path().is_file() {
                    acc.lock().push(e.into_path());
                }
            }
            WalkState::Continue
        })
    });
    acc.into_inner()
}

/// Parallel walk that collects both the path and its filesystem metadata.
fn walk_with_meta(root: &Path) -> Vec<WalkEntry> {
    let acc: Mutex<Vec<WalkEntry>> = Mutex::new(Vec::new());
    make_walker(root).build_parallel().run(|| {
        Box::new(|res| {
            if let Ok(entry) = res {
                let path = entry.into_path();
                if let Ok(meta) = path.metadata() {
                    if meta.is_file() {
                        let mtime_ns = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos() as i64)
                            .unwrap_or(0);
                        let size = meta.len() as i64;
                        acc.lock().push(WalkEntry {
                            path,
                            mtime_ns,
                            size,
                        });
                    }
                }
            }
            WalkState::Continue
        })
    });
    acc.into_inner()
}

// ─── Dirty-file detection (the hot path) ─────────────────────────────────────

/// Return every file that differs from the current snapshot.
///
/// ### Performance strategy
///
/// 1. **One DB round-trip** – load the snapshot's file map into a `HashMap`.
/// 2. **One DB round-trip** – load the full `index_cache` into a `HashMap`.
/// 3. **Parallel filesystem walk** – enumerate files + read metadata in
///    parallel using `ignore`'s built-in parallel walker.
/// 4. **Parallel hash phase** – rayon processes all walk entries; files whose
///    `(mtime_ns, size)` match the cache skip the disk read entirely.
/// 5. **Batch cache write** – newly computed hashes are written back in one
///    transaction so the next call is even faster.
///
/// On a clean working tree with a warm cache this is essentially:
///   N × stat()  +  1 DB read  (instead of N × read + N × hash)
pub fn get_dirty_files(root: &Path) -> HashMap<String, FileStatus> {
    let mut dirty = HashMap::new();
    let db_path = root.join(".velo/velo.db");
    if !db_path.exists() {
        return dirty;
    }

    let conn = match crate::db::get_conn_at_path(&db_path) {
        Ok(c) => c,
        Err(_) => return dirty,
    };
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // ── 1. Load snapshot's file map ───────────────────────────────────────────
    let mut db_files: HashMap<String, String> = {
        let mut stmt = conn
            .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")
            .unwrap();
        let collected: HashMap<String, String> = stmt
            .query_map([parent_hash.trim()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    // ── 2. Load index cache ───────────────────────────────────────────────────
    let index: HashMap<String, CacheEntry> = {
        let mut stmt = conn
            .prepare("SELECT path, mtime_ns, size, hash FROM index_cache")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|(p, m, s, h)| {
            (
                p,
                CacheEntry {
                    mtime_ns: m,
                    size: s,
                    hash: h,
                },
            )
        })
        .collect()
    };

    // ── 3. Parallel walk ──────────────────────────────────────────────────────
    let entries = walk_with_meta(root);

    // ── 4. Parallel hash (cache-aware) ────────────────────────────────────────
    // Returns (rel_path, current_hash, mtime_ns, size, was_cache_miss)
    let results: Vec<(String, String, i64, i64, bool)> = entries
        .into_par_iter()
        .map(|e| {
            let rel = crate::db::normalise(e.path.strip_prefix(root).unwrap().to_str().unwrap());
            let (hash, miss) = if let Some(cached) = index.get(&rel) {
                if cached.mtime_ns == e.mtime_ns && cached.size == e.size {
                    (cached.hash.clone(), false) // cache hit — no disk read
                } else {
                    (crate::storage::fast_hash(&e.path), true)
                }
            } else {
                (crate::storage::fast_hash(&e.path), true)
            };
            (rel, hash, e.mtime_ns, e.size, miss)
        })
        .collect();

    // ── 5. Batch-write cache misses back to DB ────────────────────────────────
    let misses: Vec<_> = results.iter().filter(|(_, _, _, _, miss)| *miss).collect();

    if !misses.is_empty() {
        if let Ok(tx) = conn.unchecked_transaction() {
            if let Ok(mut stmt) = tx.prepare(
                "INSERT OR REPLACE INTO index_cache (path, mtime_ns, size, hash)
                 VALUES (?, ?, ?, ?)",
            ) {
                for (rel, hash, mtime, size, _) in &misses {
                    let _ = stmt.execute(rusqlite::params![rel, mtime, size, hash]);
                }
            }
            let _ = tx.commit();
        }
    }

    // ── 6. Compare hashes against snapshot ───────────────────────────────────
    for (rel, hash, _, _, _) in results {
        if let Some(snap_hash) = db_files.remove(&rel) {
            if snap_hash != hash {
                dirty.insert(rel, FileStatus::Modified);
            }
        } else {
            dirty.insert(rel, FileStatus::New);
        }
    }

    // Anything left in db_files was deleted from disk
    for rel in db_files.into_keys() {
        dirty.insert(rel, FileStatus::Deleted);
    }

    dirty
}

/// Invalidate index_cache entries for a set of paths.
/// Called after `restore` writes files: the mtime will have changed so the
/// old entries would cause spurious cache hits on the next dirty check.
pub fn invalidate_cache_entries(root: &Path, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let db_path = root.join(".velo/velo.db");
    if let Ok(conn) = crate::db::get_conn_at_path(&db_path) {
        if let Ok(tx) = conn.unchecked_transaction() {
            if let Ok(mut stmt) = tx.prepare("DELETE FROM index_cache WHERE path = ?") {
                for p in paths {
                    let _ = stmt.execute([p]);
                }
            }
            let _ = tx.commit();
        }
    }
}

/// Return the list of files with active merge conflicts (reads from DB).
pub fn get_conflict_files(root: &Path) -> Vec<String> {
    let db_path = root.join(".velo/velo.db");
    if !db_path.exists() {
        return vec![];
    }
    let conn = match crate::db::get_conn_at_path(&db_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut stmt = match conn.prepare("SELECT path FROM conflict_files ORDER BY path") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

#[allow(dead_code)]
fn walkdir_all(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let n = entry.file_name();
        let name = n.to_str().unwrap_or("");
        if name == ".velo" || name == ".git" {
            continue;
        }
        if path.is_dir() {
            if let Ok(mut sub) = walkdir_all(&path) {
                out.append(&mut sub);
            }
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

// ─── Hashing (public for other modules) ──────────────────────────────────────

/// Streaming content hash — delegates to `storage::fast_hash`.
#[allow(dead_code)]
pub fn stream_hash(path: &Path) -> String {
    crate::storage::fast_hash(path)
}

/// Return `true` if the file likely contains binary data.
pub fn is_binary(path: &Path) -> bool {
    if let Ok(mut file) = fs::File::open(path) {
        let mut buf = [0u8; 1024];
        if let Ok(n) = file.read(&mut buf) {
            return buf[..n].contains(&0);
        }
    }
    false
}

// ─── Filesystem helpers ───────────────────────────────────────────────────────

/// Remove `dir` and all empty ancestors up to (but not including) `root`.
pub fn remove_empty_parents(dir: &Path, root: &Path) {
    let mut current = dir.to_path_buf();
    loop {
        if current == root {
            break;
        }
        match fs::remove_dir(&current) {
            Ok(_) => {}
            Err(_) => break,
        }
        if !current.pop() {
            break;
        }
    }
}
