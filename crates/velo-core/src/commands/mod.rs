pub mod apply;
pub mod blame;
pub mod branches;
pub mod bundle;
pub mod cherry_pick;
pub mod diff;
pub mod fsck;
pub mod gc;
pub mod grep;
pub mod history;
pub mod init;
pub mod merge;
pub mod paths;
pub mod rebase;
pub mod redo;
pub mod remote;
pub mod resolve;
pub mod restore;
pub mod save;
pub mod show;
pub mod squash;
pub mod stash;
pub mod status;
pub mod switch;
pub mod sync;
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
use rusqlite::params;

use chrono::{DateTime, Utc};

use crate::error::{RefKind, Result, VeloError};
use crate::{Repo, SnapshotId, SnapshotMeta};

/// Hex characters of a snapshot id shown in output.
///
/// **Display only.** Ids are stored, compared, keyed and transmitted at full
/// width — see `docs/FORMAT.md` §8. In v1 this was also the stored width, and
/// that truncation *was* the primary key; storing 64 bits of a hash in a store
/// several applications write to put a ~50% collision risk around 5·10⁹
/// snapshots, which is why v2 stores the whole thing.
pub const SNAP_HASH_LEN: usize = 16;

/// Hex characters of a stored snapshot id: a whole BLAKE3 digest.
pub const SNAP_ID_LEN: usize = 64;

/// Everything a snapshot's identity is derived from.
///
/// A struct rather than six positional arguments because `parent`,
/// `merge_parent` and `message` are all `&str`: swapping two of them would
/// compile, produce a plausible-looking id, and be found much later by a peer
/// whose recomputation disagrees.
pub struct SnapshotIdentity<'a> {
    /// Every `(path, object-hash, mode)` in the snapshot. Sorted internally, so
    /// the caller's order does not matter.
    pub tree: &'a [(String, String, i64)],
    /// The parent id, or `""` to start a history.
    pub parent: &'a str,
    /// The second parent of a merge, or `""` when this is not one.
    pub merge_parent: &'a str,
    /// The commit message.
    pub message: &'a str,
    /// Creation time as epoch milliseconds (UTC).
    pub timestamp_ms: i64,
    /// App-namespaced metadata, which is part of the identity (decision D1).
    pub meta: &'a SnapshotMeta,
}

/// Compute a **content-addressed** snapshot id — the v2 recipe.
///
/// The id commits to the full tree (every `path → object-hash` pair, sorted),
/// the parents, the message, the timestamp and the metadata. Because it commits
/// to the tree, a snapshot can be *verified* against its own contents (see
/// `velo fsck`), and identical work yields an identical id — the property sync
/// depends on.
///
/// Deliberately excludes the branch: a snapshot's identity must not change when
/// a branch label is renamed or soft-deleted (which rewrites the `branch`
/// column), and — as in Git — the same commit reachable from two branches should
/// have one id.
///
/// # What changed from v1
///
/// The domain separator is `velo-snapshot-v2\n`, so a v1 and a v2 snapshot can
/// never collide even given identical inputs. The timestamp is hashed as decimal
/// epoch milliseconds rather than as formatted text, so no locale or precision
/// change can shift an id. Metadata is hashed. And the result is returned at
/// full width instead of truncated to 16 characters.
pub fn snapshot_id(id: SnapshotIdentity<'_>) -> String {
    let mut entries: Vec<&(String, String, i64)> = id.tree.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut h = blake3::Hasher::new();
    h.update(b"velo-snapshot-v2\n");
    for (path, hash, mode) in entries {
        h.update(path.as_bytes());
        h.update(b"\0");
        h.update(hash.as_bytes());
        h.update(b"\0");
        h.update(mode.to_string().as_bytes());
        h.update(b"\n");
    }
    h.update(b"parent\0");
    h.update(id.parent.as_bytes());
    h.update(b"\nmerge\0");
    h.update(id.merge_parent.as_bytes());
    h.update(b"\nmessage\0");
    h.update(id.message.as_bytes());
    h.update(b"\ntime\0");
    h.update(id.timestamp_ms.to_string().as_bytes());
    // The marker is emitted even when there is no metadata, so "none" and
    // "absent" are the same thing rather than two different ids.
    h.update(b"\nmeta\0");
    for (namespace, key, value) in id.meta.iter() {
        h.update(namespace.as_bytes());
        h.update(b"\0");
        h.update(key.as_bytes());
        h.update(b"\0");
        h.update(value.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

/// Now, as epoch milliseconds (UTC) — a snapshot's `created_at_ms` and an input
/// to [`snapshot_id`].
///
/// An integer rather than v1's formatted text, so a formatting change cannot
/// alter a snapshot id. Millisecond resolution matches v1's `%.3f`, and every
/// query that orders by it also tie-breaks on `rowid`, because two snapshots
/// inside one millisecond is possible and ordering must still be total.
pub fn snapshot_timestamp_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Render a stored `created_at_ms` as a UTC datetime.
///
/// Out-of-range values cannot come from [`snapshot_timestamp_ms`], but a corrupt
/// or hand-edited row could carry one; those read as the epoch rather than
/// panicking, and `fsck` is what reports them.
pub fn timestamp_from_ms(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or(DateTime::UNIX_EPOCH)
}

/// The metadata attached to `snapshot`, in canonical order.
///
/// Reads are unvalidated (see [`SnapshotMeta::insert_stored`]): a row is whatever
/// it is, and a bad one shows up as an id mismatch in `fsck` rather than as an
/// error in the middle of an unrelated read.
pub(crate) fn load_snapshot_meta(
    conn: &rusqlite::Connection,
    snapshot: &str,
) -> rusqlite::Result<SnapshotMeta> {
    let mut stmt = conn.prepare(
        "SELECT namespace, key, value FROM snapshot_meta WHERE snapshot_id = ?
         ORDER BY namespace, key",
    )?;
    let rows = stmt.query_map([snapshot], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut meta = SnapshotMeta::new();
    for row in rows {
        let (namespace, key, value) = row?;
        meta.insert_stored(namespace, key, value);
    }
    Ok(meta)
}

/// How far an ancestry walk will go before giving up.
///
/// A guard against a cycle, which the format makes impossible — an id commits to
/// its parents, so a snapshot cannot be its own ancestor — but which a corrupt or
/// hand-edited row could still produce.
pub(crate) const MAX_ANCESTRY_DEPTH: i64 = 10_000;

/// Every snapshot reachable from `start`, mapped to its shortest distance.
///
/// **Follows both parents.** A merge absorbs the branch it merged, so that
/// branch's history is part of this snapshot's ancestry; walking `parent_hash`
/// alone made the absorbed side invisible, which showed up three ways — a
/// merge-base too old (so the next merge re-raised settled conflicts), a
/// `velo history` missing the commits it merged in, and the same for any
/// consumer walking ancestry.
///
/// `merge_parent` is `TEXT NOT NULL DEFAULT ''`, so the empty string is excluded
/// rather than looked up as a hash.
///
/// `UNION` rather than `UNION ALL`: with two parents a node is reachable by
/// several routes, and without deduplication the walk re-expands every one of
/// them. The outer `MIN` then collapses a node reached at different depths to its
/// shortest.
pub(crate) fn ancestors(
    conn: &rusqlite::Connection,
    start: &str,
) -> rusqlite::Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE anc(hash, parent_hash, merge_parent, depth) AS (
             SELECT hash, parent_hash, merge_parent, 0
               FROM snapshots WHERE hash = ?1
             UNION
             SELECT s.hash, s.parent_hash, s.merge_parent, a.depth + 1
               FROM snapshots s JOIN anc a
                 ON s.hash = a.parent_hash
                 OR (a.merge_parent <> '' AND s.hash = a.merge_parent)
              WHERE a.depth < ?2
         )
         SELECT hash, MIN(depth) FROM anc GROUP BY hash",
    )?;
    let rows = stmt.query_map(params![start, MAX_ANCESTRY_DEPTH], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

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

/// Resolve a user-supplied *spec* — a tag, a hash or unique prefix, a branch
/// name, or a remote-tracking ref like `origin/main` — into a snapshot id.
///
/// `input` stays `&str` because any string is a plausible attempt at a spec;
/// the return is typed because everything downstream needs a resolved id. See
/// [`crate::ids`].
pub fn resolve_snapshot_id(repo: &Repo, input: &str) -> Result<SnapshotId> {
    let conn = repo.conn();

    // 1. Try as tag name
    if let Ok(h) = conn.query_row(
        "SELECT snapshot_hash FROM tags WHERE name = ?",
        [input],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(SnapshotId::from_stored(h));
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
        1 => return Ok(SnapshotId::from_stored(rows.into_iter().next().unwrap())),
        n if n > 1 => {
            return Err(VeloError::invalid(format!(
                "Ambiguous prefix '{}' matches {} snapshots. Use more characters.",
                input, n
            )))
        }
        _ => {}
    }

    // 3. Try as a branch name — resolve to wherever that branch points
    //    (its own latest snapshot, or the position it was created at).
    if let Some(h) = branch_tip(conn, input) {
        return Ok(SnapshotId::from_stored(h));
    }

    // 4. Try as a remote-tracking ref "<remote>/<branch>" (e.g. origin/main).
    if let Some((remote, branch)) = input.split_once('/') {
        if let Ok(h) = conn.query_row(
            "SELECT hash FROM remote_refs WHERE remote = ? AND branch = ?",
            [remote, branch],
            |r| r.get::<_, String>(0),
        ) {
            return Ok(SnapshotId::from_stored(h));
        }
    }

    // `NotFound`, not `InvalidInput`: the spec was well-formed, it simply does not
    // name anything. A consumer needs to tell "no such ref" (often expected — a
    // branch with no snapshots yet) from "you asked me something malformed", and
    // `RefKind::Any` exists for exactly this case: a ref that could have been a
    // snapshot, a tag or a branch.
    Err(VeloError::not_found(RefKind::Any, input))
}

/// How far `local` is ahead of / behind `remote`, counted in snapshots.
/// Both tips must already exist locally (fetch first); ancestry is walked over
/// local history, so this never touches the network — same as Git.
pub(crate) fn ahead_behind(
    conn: &rusqlite::Connection,
    local: &str,
    remote: &str,
) -> (usize, usize) {
    let l = bundle::reachable_ancestry(conn, local);
    let r = bundle::reachable_ancestry(conn, remote);
    let ahead = l.difference(&r).count();
    let behind = r.difference(&l).count();
    (ahead, behind)
}

/// The remote-tracking ref for `branch`, if any: `(remote, tip_hash)`.
/// Prefers `origin` when several remotes track the same branch.
pub(crate) fn tracking_ref(conn: &rusqlite::Connection, branch: &str) -> Option<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT remote, hash FROM remote_refs WHERE branch = ?
             ORDER BY (remote <> 'origin'), remote LIMIT 1",
        )
        .ok()?;
    stmt.query_row([branch], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .ok()
}

/// Where `branch` currently points, if anywhere.
///
/// A branch's own most recent snapshot wins. If it has none — a branch created
/// by `switch` that hasn't been committed to yet, including `main` in a fresh
/// repository — fall back to the tip recorded in the `branches` table, which is
/// the position the branch was created at.
pub(crate) fn branch_tip(conn: &rusqlite::Connection, branch: &str) -> Option<String> {
    let derived = conn
        .query_row(
            "SELECT hash FROM snapshots WHERE branch = ? ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
            [branch],
            |r| r.get::<_, String>(0),
        )
        .ok();
    if derived.is_some() {
        return derived;
    }
    conn.query_row("SELECT tip FROM branches WHERE name = ?", [branch], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .filter(|t| !t.is_empty())
}

/// Every branch that exists — those with snapshots plus those merely recorded by
/// `switch` — excluding the internal `_stash`, remote-tracking `remotes/*`, and
/// soft-deleted `_deleted_*` branches.
pub(crate) fn all_branch_names(conn: &rusqlite::Connection) -> Vec<String> {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut collect = |sql: &str| {
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                for n in rows.flatten() {
                    names.insert(n);
                }
            }
        }
    };
    collect(
        "SELECT DISTINCT branch FROM snapshots
         WHERE branch <> '_stash' AND branch NOT LIKE 'remotes/%'
           AND branch NOT LIKE '_deleted_%'",
    );
    collect(
        "SELECT name FROM branches
         WHERE name <> '_stash' AND name NOT LIKE 'remotes/%'
           AND name NOT LIKE '_deleted_%'",
    );
    names.into_iter().collect()
}

/// All real branches paired with their current tip (branches that point nowhere
/// yet are omitted).
pub(crate) fn all_branch_tips(conn: &rusqlite::Connection) -> Vec<(String, String)> {
    all_branch_names(conn)
        .into_iter()
        .filter_map(|b| branch_tip(conn, &b).map(|t| (b, t)))
        .collect()
}

/// Record that `branch` exists, pointing at `tip` (empty = "exists, unborn").
/// Never moves a branch that is already recorded — use [`set_branch_tip`] for
/// that.
pub(crate) fn register_branch(conn: &rusqlite::Connection, branch: &str, tip: &str) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO branches (name, tip) VALUES (?, ?)",
        [branch, tip],
    )?;
    Ok(())
}

/// Move `branch` to point at `tip`, creating the ref if needed.
///
/// Only ever called for an explicit, user-initiated move (a merge that
/// fast-forwards an unborn branch). Simply visiting a branch never moves it.
pub(crate) fn set_branch_tip(conn: &rusqlite::Connection, branch: &str, tip: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO branches (name, tip) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET tip = ?2",
        [branch, tip],
    )?;
    Ok(())
}

/// Does `branch` exist as a ref (even if it has no commits yet)?
pub(crate) fn branch_exists(conn: &rusqlite::Connection, branch: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM branches WHERE name = ?)",
        [branch],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
        || branch_tip(conn, branch).is_some()
}

// ─── Filesystem enumeration ───────────────────────────────────────────────────

/// Collected file entry from the parallel walk.
struct WalkEntry {
    path: PathBuf,
    mtime_ns: i64,
    size: i64,
    mode: i64,
}

/// Build a `WalkBuilder` with the standard ignore rules applied, plus whatever
/// the handle's [`Scope`](crate::Scope) adds on top.
///
/// The user's `.veloignore` and `.gitignore` always apply; a scope narrows
/// further and never widens, so an application cannot quietly start tracking
/// something the user excluded.
fn make_walker(root: &Path, scope: &crate::Scope) -> WalkBuilder {
    let mut b = WalkBuilder::new(root);
    b.hidden(false)
        .add_custom_ignore_filename(".veloignore")
        .add_custom_ignore_filename(".gitignore");
    // Exclusions only. A restriction is applied to the results instead — see
    // `Scope::restriction` for why handing it to the walker would let an
    // application override the user's own `.veloignore`.
    if let Ok(Some(overrides)) = scope.exclusions(root) {
        b.overrides(overrides);
    }
    b.filter_entry(|e| {
        let n = e.file_name().to_str().unwrap_or("");
        n != ".velo" && n != ".git" && n != "target"
    });
    b
}

/// Return all tracked paths (regular files and symlinks) under `root`.
pub(crate) fn get_tracked_files(root: &Path, scope: &crate::Scope) -> Vec<PathBuf> {
    let restriction = scope.restriction(root).ok().flatten();
    let acc: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    make_walker(root, scope).build_parallel().run(|| {
        Box::new(|res| {
            if let Ok(e) = res {
                if let Ok(meta) = fs::symlink_metadata(e.path()) {
                    if (meta.is_file() || meta.file_type().is_symlink())
                        && crate::scope::permitted(restriction.as_ref(), e.path())
                    {
                        acc.lock().push(e.into_path());
                    }
                }
            }
            WalkState::Continue
        })
    });
    acc.into_inner()
}

/// Parallel walk collecting each entry's path, lstat metadata, and mode.
/// Uses `symlink_metadata` so symlinks are recorded as symlinks (mode 2) rather
/// than being followed to their target.
fn walk_with_meta(root: &Path, scope: &crate::Scope) -> Vec<WalkEntry> {
    let restriction = scope.restriction(root).ok().flatten();
    let acc: Mutex<Vec<WalkEntry>> = Mutex::new(Vec::new());
    make_walker(root, scope).build_parallel().run(|| {
        Box::new(|res| {
            if let Ok(entry) = res {
                let path = entry.into_path();
                if !crate::scope::permitted(restriction.as_ref(), &path) {
                    return WalkState::Continue;
                }
                if let Ok(meta) = fs::symlink_metadata(&path) {
                    let is_symlink = meta.file_type().is_symlink();
                    if meta.is_file() || is_symlink {
                        let mtime_ns = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos() as i64)
                            .unwrap_or(0);
                        let size = meta.len() as i64;
                        let mode = crate::storage::capture_mode(&path);
                        acc.lock().push(WalkEntry {
                            path,
                            mtime_ns,
                            size,
                            mode,
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
pub fn get_dirty_files(repo: &Repo) -> HashMap<String, FileStatus> {
    let mut dirty = HashMap::new();
    let root = repo.root();
    let conn = repo.conn();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // ── 1. Load snapshot's file map ───────────────────────────────────────────
    // Degrade gracefully: a transient DB error (e.g. a momentary lock) yields an
    // empty map rather than panicking the whole command.
    let mut db_files: HashMap<String, String> = conn
        .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([parent_hash.trim()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            Ok(rows.filter_map(|r| r.ok()).collect::<HashMap<_, _>>())
        })
        .unwrap_or_default();

    // ── 2. Load index cache ───────────────────────────────────────────────────
    let index: HashMap<String, CacheEntry> = conn
        .prepare("SELECT path, mtime_ns, size, hash FROM index_cache")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            Ok(rows
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
                .collect::<HashMap<_, _>>())
        })
        .unwrap_or_default();

    // ── 3. Parallel walk ──────────────────────────────────────────────────────
    let entries = walk_with_meta(root, repo.scope());

    // ── 4. Parallel hash (cache-aware) ────────────────────────────────────────
    // Returns (rel_path, current_hash, mtime_ns, size, was_cache_miss)
    let results: Vec<(String, String, i64, i64, bool)> = entries
        .into_par_iter()
        .filter_map(|e| {
            // Skip paths that aren't under root or aren't valid UTF-8 rather
            // than panicking on the (rare) non-UTF-8 path.
            let rel = crate::db::normalise(e.path.strip_prefix(root).ok()?.to_str()?);
            let (hash, miss) = if let Some(cached) = index.get(&rel) {
                if cached.mtime_ns == e.mtime_ns && cached.size == e.size {
                    (cached.hash.clone(), false) // cache hit — no disk read
                } else {
                    (crate::storage::hash_for(&e.path, e.mode), true)
                }
            } else {
                (crate::storage::hash_for(&e.path, e.mode), true)
            };
            Some((rel, hash, e.mtime_ns, e.size, miss))
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
pub(crate) fn invalidate_cache_entries(repo: &Repo, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let conn = repo.conn();
    if let Ok(tx) = conn.unchecked_transaction() {
        if let Ok(mut stmt) = tx.prepare("DELETE FROM index_cache WHERE path = ?") {
            for p in paths {
                let _ = stmt.execute([p]);
            }
        }
        let _ = tx.commit();
    }
}

/// Return the list of files with active merge conflicts (reads from DB).
pub(crate) fn get_conflict_files(repo: &Repo) -> Vec<String> {
    let conn = repo.conn();
    let mut stmt = match conn.prepare("SELECT path FROM conflict_files ORDER BY path") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

/// Return `true` if the file likely contains binary data.
pub(crate) fn is_binary(path: &Path) -> bool {
    if let Ok(mut file) = fs::File::open(path) {
        let mut buf = [0u8; 1024];
        if let Ok(n) = file.read(&mut buf) {
            return buf[..n].contains(&0);
        }
    }
    false
}

// ─── Three-way file reconciliation (shared by merge / cherry-pick / rebase) ───

/// A file's identity within a tree: its object hash ("" = absent) and mode.
pub(crate) type FileRef<'a> = (&'a str, i64);

/// What to do with a single file when applying "theirs" on top of "ours",
/// relative to a common ancestor.
#[derive(Debug)]
pub(crate) enum Reconcile {
    /// Nothing to do — theirs made no change, or both sides already agree.
    Nothing,
    /// Take theirs verbatim: write object `hash` with `mode` to the working
    /// tree. `is_new` is true when the file did not exist in the ancestor.
    TakeTheirs {
        hash: String,
        mode: i64,
        is_new: bool,
    },
    /// Theirs deleted a file ours left untouched — remove it.
    Delete,
    /// Both sides changed non-overlapping regions — write this merged content
    /// with `mode`.
    AutoMerged { content: Vec<u8>, mode: i64 },
    /// Theirs deleted a file ours modified — keep ours (surface to the caller).
    KeepOurs,
    /// Both sides changed the same region differently — a real conflict.
    Conflict,
}

/// Decide how to reconcile one file across `anc` (ancestor), `our` (current),
/// and `thr` (incoming), each a `(object-hash, mode)` pair.
///
/// This is the single source of truth for merge/cherry-pick/rebase file
/// classification. Non-overlapping text changes on both sides are auto-merged
/// via a real 3-way merge; overlapping edits, binary files, and symlinks (which
/// can't be line-merged) become conflicts. A file's mode is part of its
/// identity, so an executable-bit or file↔symlink change counts as a change.
pub(crate) fn reconcile_file(
    objects_dir: &Path,
    anc: FileRef,
    our: FileRef,
    thr: FileRef,
) -> Result<Reconcile> {
    // Identical on both sides (content AND mode) — nothing to bring in.
    if thr == our {
        return Ok(Reconcile::Nothing);
    }
    let thr_changed = thr != anc;
    let our_changed = our != anc;

    if !thr_changed {
        return Ok(Reconcile::Nothing);
    }
    // Only theirs changed — apply it cleanly (content and/or mode).
    if !our_changed {
        return Ok(if thr.0.is_empty() {
            Reconcile::Delete
        } else {
            Reconcile::TakeTheirs {
                hash: thr.0.to_string(),
                mode: thr.1,
                is_new: anc.0.is_empty(),
            }
        });
    }
    // Both sides changed.
    if thr.0.is_empty() {
        return Ok(Reconcile::KeepOurs); // theirs deleted, ours modified
    }
    if our.0.is_empty() {
        // ours deleted, theirs modified — restore theirs
        return Ok(Reconcile::TakeTheirs {
            hash: thr.0.to_string(),
            mode: thr.1,
            is_new: false,
        });
    }
    // Same content, differing mode only → take theirs' mode (no content merge).
    if our.0 == thr.0 {
        return Ok(Reconcile::TakeTheirs {
            hash: thr.0.to_string(),
            mode: thr.1,
            is_new: false,
        });
    }
    // Symlinks can't be line-merged.
    if our.1 == crate::storage::MODE_SYMLINK || thr.1 == crate::storage::MODE_SYMLINK {
        return Ok(Reconcile::Conflict);
    }

    // Both modified to different non-empty content → attempt a line-level merge.
    let anc_bytes = if anc.0.is_empty() {
        Vec::new()
    } else {
        crate::storage::read_object(objects_dir, anc.0)?
    };
    let our_bytes = crate::storage::read_object(objects_dir, our.0)?;
    let thr_bytes = crate::storage::read_object(objects_dir, thr.0)?;

    if anc_bytes.contains(&0) || our_bytes.contains(&0) || thr_bytes.contains(&0) {
        return Ok(Reconcile::Conflict); // binary — cannot auto-merge
    }

    match velo_merge::try_auto_merge(
        &String::from_utf8_lossy(&anc_bytes),
        &String::from_utf8_lossy(&our_bytes),
        &String::from_utf8_lossy(&thr_bytes),
    ) {
        Some(merged) => Ok(Reconcile::AutoMerged {
            content: merged.into_bytes(),
            mode: thr.1,
        }),
        None => Ok(Reconcile::Conflict),
    }
}

// ─── Filesystem helpers ───────────────────────────────────────────────────────

/// Remove `dir` and all empty ancestors up to (but not including) `root`.
pub(crate) fn remove_empty_parents(dir: &Path, root: &Path) {
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
