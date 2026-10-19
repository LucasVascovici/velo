//! How stored trees are read.
//!
//! This is the only module allowed to name the `file_map` table (a lint
//! enforcing that lands with 20.2-d). Every function here is stated in tree
//! terms (a snapshot id in, paths and entries out) and never in rows, so the
//! storage underneath can change shape without any caller noticing. They are
//! plain functions over a connection on purpose: there is exactly one storage
//! layout at a time, so a trait would only be indirection.
//!
//! This part covers the single-snapshot reads. A snapshot that has no stored
//! tree reads as an empty tree; whether the snapshot exists is for the caller
//! to check against `snapshots`.

use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;

use crate::error::Result;

/// One file in a stored tree: its object id and its mode.
pub(crate) type Entry = (String, i64);

/// A whole tree, ordered by path.
pub(crate) type StoredTree = BTreeMap<String, Entry>;

/// Most paths bound in one statement by [`files_at`], one placeholder being
/// the snapshot, so the statement stays at SQLite's customary 500.
const CHUNK: usize = 499;

/// The full tree of `snapshot`; empty when it has no rows.
pub(crate) fn tree_of(conn: &Connection, snapshot: &str) -> Result<StoredTree> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let rows = stmt.query_map([snapshot], |r| {
        Ok((
            r.get::<_, String>(0)?,
            (r.get::<_, String>(1)?, r.get::<_, i64>(2)?),
        ))
    })?;
    let mut tree = StoredTree::new();
    for row in rows {
        let (path, entry) = row?;
        tree.insert(path, entry);
    }
    Ok(tree)
}

/// The entry of one `path` in `snapshot`, or `None` when it is not there.
pub(crate) fn file_at(conn: &Connection, snapshot: &str, path: &str) -> Result<Option<Entry>> {
    let mut stmt = conn
        .prepare_cached("SELECT hash, mode FROM file_map WHERE snapshot_hash = ? AND path = ?")?;
    let mut rows = stmt.query(rusqlite::params![snapshot, path])?;
    match rows.next()? {
        Some(r) => Ok(Some((r.get(0)?, r.get(1)?))),
        None => Ok(None),
    }
}

/// The entries of every `paths` member that is in `snapshot`, keyed by path.
/// Paths that are not in the tree are simply absent from the result.
pub(crate) fn files_at(
    conn: &Connection,
    snapshot: &str,
    paths: &[&str],
) -> Result<HashMap<String, Entry>> {
    let mut found: HashMap<String, Entry> = HashMap::with_capacity(paths.len());
    for chunk in paths.chunks(CHUNK) {
        let marks = (2..chunk.len() + 2)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?1 AND path IN ({marks})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut args: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len() + 1);
        args.push(&snapshot);
        for p in chunk {
            args.push(p);
        }
        let rows = stmt.query_map(args.as_slice(), |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, i64>(2)?),
            ))
        })?;
        for row in rows {
            let (path, entry) = row?;
            found.insert(path, entry);
        }
    }
    Ok(found)
}

/// Whether `snapshot` has a stored tree at all (an empty tree has none).
///
/// Nothing outside the tests asks this yet; the whole-table work in 20.2-b is
/// its first caller.
#[allow(dead_code)]
pub(crate) fn has_tree(conn: &Connection, snapshot: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ?)",
        [snapshot],
        |r| r.get(0),
    )?)
}

/// Whether `path` is in `snapshot`, either as a file or as a directory with
/// at least one file beneath it. `path` must be normalised and carry no
/// trailing slash.
pub(crate) fn has_path_or_under(conn: &Connection, snapshot: &str, path: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_map
          WHERE snapshot_hash = ? AND (path = ? OR path LIKE ? || '/%'))",
        rusqlite::params![snapshot, path, path],
        |r| r.get(0),
    )?)
}

/// A path's `index_cache` row: mtime in ns, size, and the hash it produced.
pub(crate) type CacheRow = (i64, i64, String);

/// What the joined query reports for one path: its object id in the tree (if
/// tracked there) and its cache row (if cached).
pub(crate) type JoinedRow = (Option<String>, Option<CacheRow>);

/// Every path in the tree of `snapshot` or in `index_cache`, with its object
/// id (when tracked) and its cache row (when cached).
///
/// It is one query, a full outer join, because it is status's hot path.
pub(crate) fn tree_with_index_cache(
    conn: &Connection,
    snapshot: &str,
) -> Result<HashMap<String, JoinedRow>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(f.path, c.path), f.hash, c.mtime_ns, c.size, c.hash
         FROM (SELECT path, hash FROM file_map WHERE snapshot_hash = ?1) f
         FULL OUTER JOIN index_cache c ON c.path = f.path",
    )?;
    let rows = stmt.query_map([snapshot], |r| {
        let cache = match (
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<String>>(4)?,
        ) {
            (Some(m), Some(s), Some(h)) => Some((m, s, h)),
            _ => None,
        };
        Ok((
            r.get::<_, String>(0)?,
            (r.get::<_, Option<String>>(1)?, cache),
        ))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// `parent` as the optional first parent [`write_tree`] takes: the empty
/// string every caller holds for "no parent" becomes `None`.
pub(crate) fn first_parent(parent: &str) -> Option<&str> {
    let parent = parent.trim();
    (!parent.is_empty()).then_some(parent)
}

/// Stores the full tree of `snapshot`.
///
/// `first_parent` is unused while every tree is stored whole, but each caller
/// passes the real one (for a merge, `parent_hash` and not `merge_parent`)
/// because the delta layout deltas a tree against it. One prepared statement
/// serves every row, since saves write hundreds of thousands of them.
pub(crate) fn write_tree(
    tx: &Connection,
    snapshot: &str,
    first_parent: Option<&str>,
    tree: &[(&str, &str, i64)],
) -> Result<()> {
    let _ = first_parent;
    let mut ins = tx.prepare_cached(
        "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
    )?;
    for (path, object, mode) in tree {
        ins.execute(rusqlite::params![snapshot, path, object, mode])?;
    }
    Ok(())
}

/// Removes the stored trees of `snapshots`; a snapshot without one is skipped.
///
/// Contract for the delta layout: it must refuse when a stored tree outside
/// the set depends on a member, because deleting that member would leave the
/// dependant unreadable. Callers delete whole ranges together for this reason.
pub(crate) fn delete_trees(tx: &Connection, snapshots: &[&str]) -> Result<()> {
    if snapshots.is_empty() {
        return Ok(());
    }
    let marks = vec!["?"; snapshots.len()].join(", ");
    tx.execute(
        &format!("DELETE FROM file_map WHERE snapshot_hash IN ({marks})"),
        rusqlite::params_from_iter(snapshots.iter()),
    )?;
    Ok(())
}

/// Moves the stored tree of `old` to `new` in place, which is compaction's
/// re-mint: no row is copied and no index entry is written for one that did
/// not move. `new` must not already have a tree.
pub(crate) fn rekey_tree(tx: &Connection, old: &str, new: &str) -> Result<()> {
    tx.execute(
        "UPDATE file_map SET snapshot_hash = ? WHERE snapshot_hash = ?",
        rusqlite::params![new, old],
    )?;
    Ok(())
}

/// The rows of `snapshot`'s tree as a bundle sends them: path, object id and
/// mode, in no particular order.
pub(crate) fn tree_rows_for_export(
    conn: &Connection,
    snapshot: &str,
) -> Result<Vec<(String, String, i64)>> {
    let mut stmt =
        conn.prepare_cached("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let rows = stmt.query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.flatten().collect())
}
