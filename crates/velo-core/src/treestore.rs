//! How a snapshot's tree is stored, and the only place that knows.
//!
//! Every reader that wants "the tree of this snapshot" or "this path in this
//! snapshot" asks here instead of selecting from `file_map` itself. Today a
//! snapshot's tree is its full set of `file_map` rows, so each function is a
//! thin query; the point of the module is that a later storage layout (a
//! snapshot stored as a delta against its parent) changes this file and not
//! the dozens of call sites. A reader that bypasses it would quietly see the
//! wrong files under such a layout, not crash.
//!
//! Functions take a plain `&Connection` (a `Transaction` derefs to one) and
//! there is deliberately no trait: there is one storage format at a time.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::Result;

/// One tree row: `(path, object hash, mode)`.
pub(crate) type Row = (String, String, i64);

/// What a tree holds at one path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub hash: String,
    pub mode: i64,
}

/// Most paths in one `IN (...)`: one placeholder is the snapshot, so 499 paths
/// plus it make the 500 this crate allows per statement.
const CHUNK: usize = 499;

/// The full tree of `snapshot`, sorted by path (byte order). The empty id is
/// the root commit's "ancestor" and yields an empty tree, as does an id with
/// no rows.
pub(crate) fn tree_of(conn: &Connection, snapshot: &str) -> Result<Vec<Row>> {
    if snapshot.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ? ORDER BY path")?;
    let rows = stmt.query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `path` in `snapshot`, or `None` when the tree does not hold it.
pub(crate) fn file_at(conn: &Connection, snapshot: &str, path: &str) -> Result<Option<Entry>> {
    let mut stmt = conn
        .prepare_cached("SELECT hash, mode FROM file_map WHERE snapshot_hash = ? AND path = ?")?;
    let mut rows = stmt.query(rusqlite::params![snapshot, path])?;
    match rows.next()? {
        Some(r) => Ok(Some(Entry {
            hash: r.get(0)?,
            mode: r.get(1)?,
        })),
        None => Ok(None),
    }
}

/// Several paths of `snapshot` at once, in statements of at most 499 paths.
/// Paths the tree does not hold are omitted from the result.
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
                Entry {
                    hash: r.get(1)?,
                    mode: r.get(2)?,
                },
            ))
        })?;
        for row in rows {
            let (path, entry) = row?;
            found.insert(path, entry);
        }
    }
    Ok(found)
}

/// Whether `snapshot` holds `path` itself or anything beneath `path/`.
///
/// A path that was tracked but has since been deleted is still a path to a
/// caller deciding whether an argument names a file or a ref, which is why the
/// directory case counts.
pub(crate) fn holds_path_or_under(conn: &Connection, snapshot: &str, path: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_map
          WHERE snapshot_hash = ? AND (path = ? OR path LIKE ? || '/%'))",
        rusqlite::params![snapshot, path, path],
        |r| r.get::<_, bool>(0),
    )?)
}

/// What `snapshot` changed against its first parent, sorted by path: `Some` is
/// an added or changed path (including a mode change), `None` a removed one.
///
/// The parent comes from `snapshots`, falling back to `trash` for a snapshot
/// that was undone. A root (or an unknown id) has no parent, so every path is
/// an addition.
#[allow(dead_code)] // first reader is the delta-aware work that follows; tests cover it now
pub(crate) fn changes_of(
    conn: &Connection,
    snapshot: &str,
) -> Result<Vec<(String, Option<Entry>)>> {
    let parent = parent_of(conn, snapshot)?;
    let new = tree_of(conn, snapshot)?;
    let old = tree_of(conn, &parent)?;

    let mut out: Vec<(String, Option<Entry>)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    // Both trees are sorted by path, so one merge pass finds every difference.
    while i < old.len() || j < new.len() {
        let order = match (old.get(i), new.get(j)) {
            (Some(o), Some(n)) => o.0.as_bytes().cmp(n.0.as_bytes()),
            (Some(_), None) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        };
        match order {
            std::cmp::Ordering::Less => {
                out.push((old[i].0.clone(), None));
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push((new[j].0.clone(), Some(entry_of(&new[j]))));
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if old[i].1 != new[j].1 || old[i].2 != new[j].2 {
                    out.push((new[j].0.clone(), Some(entry_of(&new[j]))));
                }
                i += 1;
                j += 1;
            }
        }
    }
    Ok(out)
}

#[allow(dead_code)]
fn entry_of(row: &Row) -> Entry {
    Entry {
        hash: row.1.clone(),
        mode: row.2,
    }
}

/// The first parent of `snapshot`, from `snapshots` or else `trash`; empty for
/// a root or an id that is in neither.
#[allow(dead_code)]
fn parent_of(conn: &Connection, snapshot: &str) -> Result<String> {
    for sql in [
        "SELECT parent_hash FROM snapshots WHERE hash = ?",
        "SELECT parent_hash FROM trash WHERE hash = ?",
    ] {
        match conn.query_row(sql, [snapshot], |r| r.get::<_, String>(0)) {
            Ok(p) => return Ok(p),
            Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(String::new())
}
