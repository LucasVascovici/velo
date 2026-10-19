//! How stored trees are read.
//!
//! This is the only module allowed to name the `file_map` table; the
//! `tree_store_lint` test fails the build on any other SQL string that does
//! (`db.rs`, the schema, and the tests are exempt, with reasons written next
//! to the exemption). Every function here is stated in tree
//! terms (a snapshot id in, paths and entries out) and never in rows, so the
//! storage underneath can change shape without any caller noticing. They are
//! plain functions over a connection on purpose: there is exactly one storage
//! layout at a time, so a trait would only be indirection.
//!
//! # Which tables hold a tree
//!
//! | Table | What it is | Through the accessor? |
//! |---|---|---|
//! | `file_map` | a snapshot's tree | yes |
//! | `stash` | a pointer to a snapshot on `_stash`; that snapshot's tree is a tree | pointer: no; its tree: yes |
//! | `trash` | snapshot rows `undo` moved aside; their trees stay in storage, can be delta bases and are restored by `redo` | their trees: yes |
//! | `MERGE_ORIG` (a `settings` key) | working-copy bytes saved during a merge, not a snapshot's tree | no |
//! | `renames` | rename edges between snapshots | no |
//! | `pending_renames` | working state of renames not yet saved | no |
//! | `conflict_files`, `hunk_decisions` | the merge in progress | no |
//! | `index_cache` | the stat cache of the working copy | no |
//!
//! No other table in `db.rs` holds a tree (`objects` holds content, the rest
//! hold refs and metadata).
//!
//! This part covers the single-snapshot reads. A snapshot that has no stored
//! tree reads as an empty tree; whether the snapshot exists is for the caller
//! to check against `snapshots`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// One file in a stored tree: its object id and its mode.
pub(crate) type Entry = (String, i64);

/// A whole tree, ordered by path.
pub(crate) type StoredTree = BTreeMap<String, Entry>;

/// Most paths bound in one statement by [`files_at`], one placeholder being
/// the snapshot, so the statement stays at SQLite's customary 500.
const CHUNK: usize = 499;

/// Most links a delta chain may have before a reader calls it corrupt
/// (docs/FORMAT.md, section 11.6). A writer keeps chains far shorter; the bound
/// exists so a cyclic or runaway chain is an error and never a hang.
pub(crate) const MAX_CHAIN_LINKS: usize = 4096;

fn corrupt(snapshot: &str, why: &str) -> Error {
    Error::Corrupt {
        detail: format!("tree of snapshot {snapshot}: {why}"),
    }
}

/// The snapshots whose rows make up the tree of `snapshot`, keyframe first and
/// `snapshot` last; a keyframe is a chain of one. Costs one `tree_deltas`
/// primary-key probe for a keyframe.
///
/// A cycle, a base that is in neither `snapshots` nor `trash`, or more than
/// [`MAX_CHAIN_LINKS`] links is `Error::Corrupt`. A snapshot with no rows at
/// all is an empty keyframe and fine.
fn chain(conn: &Connection, snapshot: &str) -> Result<Vec<String>> {
    let mut probe =
        conn.prepare_cached("SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?")?;
    let mut links = vec![snapshot.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(snapshot.to_string());
    loop {
        let cur = links.last().expect("chain is never empty");
        let base: Option<String> = match probe.query_row([cur], |r| r.get(0)) {
            Ok(b) => Some(b),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let Some(base) = base else { break };
        if links.len() > MAX_CHAIN_LINKS {
            return Err(corrupt(
                snapshot,
                &format!("delta chain longer than {MAX_CHAIN_LINKS} links"),
            ));
        }
        if !seen.insert(base.clone()) {
            return Err(corrupt(snapshot, &format!("delta chain cycles at {base}")));
        }
        let known: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?1)
                 OR EXISTS(SELECT 1 FROM trash WHERE hash = ?1)",
            [&base],
            |r| r.get(0),
        )?;
        if !known {
            return Err(corrupt(
                snapshot,
                &format!("delta base {base} does not exist"),
            ));
        }
        links.push(base);
    }
    links.reverse();
    Ok(links)
}

fn keyframe_rows(conn: &Connection, snapshot: &str) -> Result<StoredTree> {
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

/// The full tree of `snapshot`; empty when it has no rows.
pub(crate) fn tree_of(conn: &Connection, snapshot: &str) -> Result<StoredTree> {
    let links = chain(conn, snapshot)?;
    let mut tree = keyframe_rows(conn, &links[0])?;
    let mut stmt = conn
        .prepare_cached("SELECT path, hash, mode FROM tree_delta_rows WHERE snapshot_hash = ?")?;
    for link in &links[1..] {
        let rows = stmt.query_map([link], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows {
            match row? {
                (path, Some(hash), mode) => {
                    tree.insert(path, (hash, mode.unwrap_or(0)));
                }
                (path, None, _) => {
                    tree.remove(&path);
                }
            }
        }
    }
    Ok(tree)
}

/// One path through an already walked chain: the newest delta row for it wins,
/// else the keyframe's row; a NULL-hash delta row means absent.
fn file_in_chain(conn: &Connection, links: &[String], path: &str) -> Result<Option<Entry>> {
    if links.len() > 1 {
        let mut stmt = conn.prepare_cached(
            "SELECT hash, mode FROM tree_delta_rows WHERE snapshot_hash = ? AND path = ?",
        )?;
        for link in links[1..].iter().rev() {
            let mut rows = stmt.query(rusqlite::params![link, path])?;
            if let Some(r) = rows.next()? {
                return Ok(match r.get::<_, Option<String>>(0)? {
                    Some(hash) => Some((hash, r.get::<_, Option<i64>>(1)?.unwrap_or(0))),
                    None => None,
                });
            }
        }
    }
    let mut stmt = conn
        .prepare_cached("SELECT hash, mode FROM file_map WHERE snapshot_hash = ? AND path = ?")?;
    let mut rows = stmt.query(rusqlite::params![links[0], path])?;
    match rows.next()? {
        Some(r) => Ok(Some((r.get(0)?, r.get(1)?))),
        None => Ok(None),
    }
}

/// The entry of one `path` in `snapshot`, or `None` when it is not there.
pub(crate) fn file_at(conn: &Connection, snapshot: &str, path: &str) -> Result<Option<Entry>> {
    let links = chain(conn, snapshot)?;
    file_in_chain(conn, &links, path)
}

/// The entries of every `paths` member that is in `snapshot`, keyed by path.
/// Paths that are not in the tree are simply absent from the result.
pub(crate) fn files_at(
    conn: &Connection,
    snapshot: &str,
    paths: &[&str],
) -> Result<HashMap<String, Entry>> {
    let links = chain(conn, snapshot)?;
    let mut found: HashMap<String, Entry> = HashMap::with_capacity(paths.len());
    if links.len() > 1 {
        for p in paths {
            if let Some(entry) = file_in_chain(conn, &links, p)? {
                found.insert((*p).to_string(), entry);
            }
        }
        return Ok(found);
    }
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

/// Whether `snapshot` has a stored tree at all: `file_map` rows or a delta
/// record (an empty keyframe has none).
///
/// Nothing outside the tests asks this yet; the whole-table work in 20.2-b is
/// its first caller.
#[allow(dead_code)]
pub(crate) fn has_tree(conn: &Connection, snapshot: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tree_deltas WHERE snapshot_hash = ?1)
             OR EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ?1)",
        [snapshot],
        |r| r.get(0),
    )?)
}

/// Whether `snapshot` is stored as a delta rather than a keyframe.
fn is_delta(conn: &Connection, snapshot: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM tree_deltas WHERE snapshot_hash = ?)")?
        .query_row([snapshot], |r| r.get(0))?)
}

/// Whether any snapshot at all is stored as a delta.
fn has_any_delta(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM tree_deltas)", [], |r| r.get(0))?)
}

/// Whether `path` is in `snapshot`, either as a file or as a directory with
/// at least one file beneath it. `path` must be normalised and carry no
/// trailing slash.
pub(crate) fn has_path_or_under(conn: &Connection, snapshot: &str, path: &str) -> Result<bool> {
    if is_delta(conn, snapshot)? {
        let under = format!("{path}/");
        return Ok(tree_of(conn, snapshot)?
            .keys()
            .any(|p| p == path || p.starts_with(&under)));
    }
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
/// A keyframe is one query, a full outer join, because it is status's hot
/// path. A delta snapshot is reconstructed first and joined in memory, to the
/// same shape.
pub(crate) fn tree_with_index_cache(
    conn: &Connection,
    snapshot: &str,
) -> Result<HashMap<String, JoinedRow>> {
    if is_delta(conn, snapshot)? {
        let mut out: HashMap<String, JoinedRow> = tree_of(conn, snapshot)?
            .into_iter()
            .map(|(p, (h, _))| (p, (Some(h), None)))
            .collect();
        let mut stmt = conn.prepare("SELECT path, mtime_ns, size, hash FROM index_cache")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ),
            ))
        })?;
        for row in rows {
            let (path, cache) = row?;
            out.entry(path).or_insert((None, None)).1 = Some(cache);
        }
        return Ok(out);
    }
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

// ─── Whole-table scans and path-keyed probes ─────────────────────────────────

/// One `(path, object, mode)` row of a stored tree, as handed to
/// [`for_each_tree_in_order`].
pub(crate) type TreeRowOwned = (String, String, i64);

/// Hand every stored tree to `f` exactly once, as `(snapshot, rows)`, with the
/// rows owned so the caller may move them to another thread.
///
/// Trees arrive in chain order: a keyframe is read from `file_map`, then each
/// delta snapshot based on it is produced from a running tree by applying only
/// that delta's rows, depth first, so the reading work is the keyframes plus
/// the total number of changes and never snapshots times files. Callers must
/// not depend on the order of the rows inside one tree. Live and trashed trees
/// are visited alike (a trashed base still anchors its deltas), and a
/// snapshot that is a keyframe with no rows is not visited. Rows that cannot
/// be read as the expected types are skipped, so a damaged row cannot stop
/// `fsck` from reporting on the rest.
///
/// Returns the delta snapshots that were never reached, sorted: those on a
/// cycle, or based on a snapshot that does not exist. Their trees cannot be
/// built, and they are reported here instead of being skipped silently.
pub(crate) fn for_each_tree_in_order(
    conn: &Connection,
    mut f: impl FnMut(String, Vec<TreeRowOwned>) -> Result<()>,
) -> Result<Vec<String>> {
    let any_delta = has_any_delta(conn)?;
    let mut visited: HashSet<String> = HashSet::new();

    let mut stmt = conn
        .prepare("SELECT snapshot_hash, path, hash, mode FROM file_map ORDER BY snapshot_hash")?;
    let mut scan = stmt.query([])?;
    let mut current: Option<(String, Vec<TreeRowOwned>)> = None;
    // One finished keyframe: emitted, then its deltas are descended into.
    let mut finish =
        |done: String, rows: Vec<TreeRowOwned>, visited: &mut HashSet<String>| -> Result<()> {
            if !any_delta {
                return f(done, rows);
            }
            if is_delta(conn, &done)? {
                // Its tree is the delta's; the stray keyframe rows are ignored,
                // as `tree_of` ignores them.
                return Ok(());
            }
            if !has_children(conn, &done)? {
                return f(done, rows);
            }
            let tree: StoredTree = rows
                .iter()
                .map(|(p, h, m)| (p.clone(), (h.clone(), *m)))
                .collect();
            f(done.clone(), rows)?;
            descend(conn, &done, tree, &mut f, visited)
        };
    while let Some(row) = scan.next()? {
        let (Ok(snap), Ok(path), Ok(hash), Ok(mode)) = (
            row.get::<_, String>(0),
            row.get::<_, String>(1),
            row.get::<_, String>(2),
            row.get::<_, i64>(3),
        ) else {
            continue;
        };
        if current.as_ref().is_some_and(|(s, _)| *s != snap) {
            if let Some((done, rows)) = current.take() {
                finish(done, rows, &mut visited)?;
            }
        }
        match &mut current {
            Some((_, rows)) => rows.push((path, hash, mode)),
            None => current = Some((snap, vec![(path, hash, mode)])),
        }
    }
    if let Some((done, rows)) = current.take() {
        finish(done, rows, &mut visited)?;
    }
    drop(scan);
    drop(stmt);
    if !any_delta {
        return Ok(Vec::new());
    }

    // Keyframes with no rows: a base that is a known snapshot but has neither
    // `file_map` rows nor a delta record is an empty tree to descend from.
    let mut roots = conn.prepare(
        "SELECT DISTINCT base_hash FROM tree_deltas
          WHERE base_hash NOT IN (SELECT snapshot_hash FROM tree_deltas)
            AND NOT EXISTS (SELECT 1 FROM file_map WHERE snapshot_hash = base_hash)
            AND (base_hash IN (SELECT hash FROM snapshots)
                 OR base_hash IN (SELECT hash FROM trash))
          ORDER BY base_hash",
    )?;
    let empties: Vec<String> = roots
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    for root in empties {
        descend(conn, &root, StoredTree::new(), &mut f, &mut visited)?;
    }

    let mut all = conn.prepare("SELECT snapshot_hash FROM tree_deltas ORDER BY snapshot_hash")?;
    let mut unreached = Vec::new();
    for h in all.query_map([], |r| r.get::<_, String>(0))? {
        let h = h?;
        if !visited.contains(&h) {
            unreached.push(h);
        }
    }
    Ok(unreached)
}

fn has_children(conn: &Connection, snapshot: &str) -> Result<bool> {
    Ok(conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM tree_deltas WHERE base_hash = ?)")?
        .query_row([snapshot], |r| r.get(0))?)
}

/// Visit every delta snapshot below `root`, whose tree is `tree`, depth first.
/// Siblings share their parent's tree until the last one takes it over, so a
/// wide fan-out costs one copy per extra child and a straight chain none.
fn descend(
    conn: &Connection,
    root: &str,
    tree: StoredTree,
    f: &mut impl FnMut(String, Vec<TreeRowOwned>) -> Result<()>,
    visited: &mut HashSet<String>,
) -> Result<()> {
    let mut kids = conn.prepare_cached(
        "SELECT snapshot_hash FROM tree_deltas WHERE base_hash = ? ORDER BY snapshot_hash DESC",
    )?;
    let mut rows_stmt = conn
        .prepare_cached("SELECT path, hash, mode FROM tree_delta_rows WHERE snapshot_hash = ?")?;
    let mut stack: Vec<(String, Rc<StoredTree>)> = Vec::new();
    let mut push_children =
        |stack: &mut Vec<(String, Rc<StoredTree>)>, parent: &str, tree: StoredTree| -> Result<()> {
            let children: Vec<String> = kids
                .query_map([parent], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            let shared = Rc::new(tree);
            for c in children {
                stack.push((c, Rc::clone(&shared)));
            }
            Ok(())
        };
    push_children(&mut stack, root, tree)?;
    while let Some((snap, parent_tree)) = stack.pop() {
        if !visited.insert(snap.clone()) {
            continue;
        }
        let mut tree = Rc::try_unwrap(parent_tree).unwrap_or_else(|rc| (*rc).clone());
        let rows = rows_stmt.query_map([&snap], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?;
        for row in rows.flatten() {
            match row {
                (path, Some(hash), mode) => {
                    tree.insert(path, (hash, mode.unwrap_or(0)));
                }
                (path, None, _) => {
                    tree.remove(&path);
                }
            }
        }
        let out: Vec<TreeRowOwned> = tree
            .iter()
            .map(|(p, (h, m))| (p.clone(), h.clone(), *m))
            .collect();
        f(snap.clone(), out)?;
        push_children(&mut stack, &snap, tree)?;
    }
    Ok(())
}

/// Every object any stored tree references: a keyframe's `file_map` rows and
/// the non-NULL rows of every delta, which is what keeps `gc` from collecting
/// an object only a delta mentions. The empty hash is left out.
pub(crate) fn referenced_objects(conn: &Connection) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    // Deduplicated on the borrowed text, so a hash shared by every snapshot of
    // a file is allocated once rather than once per row.
    for sql in [
        "SELECT hash FROM file_map",
        "SELECT hash FROM tree_delta_rows WHERE hash IS NOT NULL",
    ] {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if let Some(hash) = row.get_ref(0).ok().and_then(|v| v.as_str().ok()) {
                if !hash.is_empty() && !set.contains(hash) {
                    set.insert(hash.to_string());
                }
            }
        }
    }
    Ok(set)
}

/// Delete the trees of snapshots that are in neither `snapshots` nor `trash`,
/// returning how many rows went.
///
/// The orphans are found among the distinct snapshot ids, walked by skipping
/// from one to the next (one seek per snapshot), and only then deleted.
/// Testing every row against both lists was the slow part.
///
/// A tree some remaining delta is based on is never deleted, orphan or not,
/// because that would leave the delta unreadable; deleting a childless orphan
/// can free its own base, so the pass repeats until nothing more goes. Delta
/// rows with no `tree_deltas` record belong to no tree and always go. What is
/// left (orphan deltas with dependants, or on a cycle) is for the fold of 20.6,
/// and [`orphan_delta_snapshots`] counts it so `fsck` can report it.
pub(crate) fn delete_orphan_trees(conn: &Connection) -> Result<usize> {
    let mut total = conn.execute(
        "DELETE FROM tree_delta_rows
          WHERE snapshot_hash NOT IN (SELECT snapshot_hash FROM tree_deltas)",
        [],
    )?;
    loop {
        let keyframes = conn.execute(
            "DELETE FROM file_map WHERE snapshot_hash IN (
                 WITH RECURSIVE s(h) AS (
                     SELECT MIN(snapshot_hash) FROM file_map
                     UNION ALL
                     SELECT (SELECT MIN(snapshot_hash) FROM file_map WHERE snapshot_hash > s.h)
                     FROM s WHERE h IS NOT NULL)
                 SELECT h FROM s
                 WHERE h IS NOT NULL
                   AND h NOT IN (SELECT hash FROM snapshots)
                   AND h NOT IN (SELECT hash FROM trash)
                   AND h NOT IN (SELECT base_hash FROM tree_deltas))",
            [],
        )?;
        let mut stmt = conn.prepare(
            "SELECT snapshot_hash FROM tree_deltas
              WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
                AND snapshot_hash NOT IN (SELECT hash FROM trash)
                AND snapshot_hash NOT IN (SELECT base_hash FROM tree_deltas)",
        )?;
        let orphans: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut deltas = 0;
        for h in &orphans {
            deltas += conn.execute("DELETE FROM tree_delta_rows WHERE snapshot_hash = ?", [h])?;
            deltas += conn.execute("DELETE FROM tree_deltas WHERE snapshot_hash = ?", [h])?;
        }
        total += keyframes + deltas;
        if keyframes + deltas == 0 {
            return Ok(total);
        }
    }
}

/// How many delta snapshots are in neither `snapshots` nor `trash`. After
/// [`delete_orphan_trees`] these are the ones it had to keep because another
/// delta is based on them (or they sit on a cycle).
#[allow(dead_code)]
pub(crate) fn orphan_delta_snapshots(conn: &Connection) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tree_deltas
          WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
            AND snapshot_hash NOT IN (SELECT hash FROM trash)",
        [],
        |r| r.get(0),
    )?;
    Ok(n.max(0) as usize)
}

/// Remove `index_cache` rows for paths that no live snapshot's tree holds,
/// returning how many went. Keeping a row is always allowed (the cache is
/// derived), so an implementation may be conservative.
///
/// A path is live when a live keyframe has it or any delta row names it with
/// an object. That is conservative on purpose: a path a delta inherits from a
/// trashed base, or one only a dead delta mentions, may be kept or lost, and
/// either is harmless to a derived cache.
pub(crate) fn prune_stale_index_cache(conn: &Connection) -> Result<usize> {
    // One path-index probe per cached path, stopping at the first live row,
    // instead of building the set of every path in the table.
    Ok(conn.execute(
        "DELETE FROM index_cache
         WHERE NOT EXISTS (
             SELECT 1 FROM file_map
             WHERE file_map.path = index_cache.path
               AND file_map.snapshot_hash IN (SELECT hash FROM snapshots)
         ) AND NOT EXISTS (
             SELECT 1 FROM tree_delta_rows
             WHERE tree_delta_rows.path = index_cache.path
               AND tree_delta_rows.hash IS NOT NULL
         )",
        [],
    )?)
}

/// How many stored entries exactly repeat another entry of the same tree.
///
/// Only `file_map` can hold duplicates: `tree_delta_rows` is keyed by
/// (snapshot, path), so a delta cannot repeat a row.
///
/// `fsck` counts these while it already holds every tree, which is cheaper;
/// this is the standalone form for callers that do not scan.
#[allow(dead_code)]
pub(crate) fn duplicate_rows(conn: &Connection) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COALESCE(SUM(n - 1), 0) FROM (
             SELECT COUNT(*) AS n FROM file_map
             GROUP BY snapshot_hash, path, hash, mode)",
        [],
        |r| r.get(0),
    )?;
    Ok(n.max(0) as usize)
}

/// Drop every entry that exactly repeats another entry of the same tree,
/// keeping one, and return how many went. `tree_delta_rows` cannot hold
/// duplicates (its primary key forbids them), so only `file_map` is touched.
pub(crate) fn remove_duplicate_rows(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM file_map WHERE rowid NOT IN
           (SELECT MIN(rowid) FROM file_map
            GROUP BY snapshot_hash, path, hash, mode)",
        [],
    )?)
}

/// Whether `snapshot` differs from `parent` anywhere under `path`.
///
/// `path` matches a file exactly, or a directory by prefix. A difference is an
/// entry (path, object or mode) present on one side and not the other, so an
/// addition, deletion, edit or mode flip all count. An empty `parent` is a
/// root snapshot, where anything present is newly added.
pub(crate) fn changed_under(
    conn: &Connection,
    snapshot: &str,
    parent: &str,
    path: &str,
) -> Result<bool> {
    // `_` and `%` are LIKE wildcards and are legal in filenames.
    let prefix = format!(
        "{}/%",
        path.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );

    if is_delta(conn, snapshot)? || (!parent.is_empty() && is_delta(conn, parent)?) {
        return changed_under_delta(conn, snapshot, parent, path);
    }

    if parent.is_empty() {
        return Ok(conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM file_map
                  WHERE snapshot_hash = ?1
                    AND (path = ?2 OR path LIKE ?3 ESCAPE '\\')
             )",
            rusqlite::params![snapshot, path, prefix],
            |r| r.get::<_, bool>(0),
        )?);
    }

    Ok(conn.query_row(
        "SELECT EXISTS(
             SELECT path, hash, mode FROM file_map
              WHERE snapshot_hash = ?1 AND (path = ?3 OR path LIKE ?4 ESCAPE '\\')
             EXCEPT
             SELECT path, hash, mode FROM file_map
              WHERE snapshot_hash = ?2 AND (path = ?3 OR path LIKE ?4 ESCAPE '\\')
         ) OR EXISTS(
             SELECT path, hash, mode FROM file_map
              WHERE snapshot_hash = ?2 AND (path = ?3 OR path LIKE ?4 ESCAPE '\\')
             EXCEPT
             SELECT path, hash, mode FROM file_map
              WHERE snapshot_hash = ?1 AND (path = ?3 OR path LIKE ?4 ESCAPE '\\')
         )",
        rusqlite::params![snapshot, parent, path, prefix],
        |r| r.get::<_, bool>(0),
    )?)
}

/// [`changed_under`] when a delta is involved. A delta against its own base
/// is answered from its delta rows alone (a row counts only if it really
/// differs from the base's entry); any other pairing compares the two trees.
fn changed_under_delta(
    conn: &Connection,
    snapshot: &str,
    parent: &str,
    path: &str,
) -> Result<bool> {
    let under = format!("{path}/");
    let inside = |p: &str| p == path || p.starts_with(&under);
    let base: Option<String> = conn
        .prepare_cached("SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?")?
        .query_row([snapshot], |r| r.get(0))
        .ok();
    if !parent.is_empty() && base.as_deref() == Some(parent) {
        let mut stmt = conn.prepare_cached(
            "SELECT path, hash, mode FROM tree_delta_rows WHERE snapshot_hash = ?",
        )?;
        let rows: Vec<(String, Option<String>, Option<i64>)> = stmt
            .query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for (p, hash, mode) in rows {
            if !inside(&p) {
                continue;
            }
            let after = hash.map(|h| (h, mode.unwrap_or(0)));
            if file_at(conn, parent, &p)? != after {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let mut after = tree_of(conn, snapshot)?;
    after.retain(|p, _| inside(p));
    let mut before = if parent.is_empty() {
        StoredTree::new()
    } else {
        tree_of(conn, parent)?
    };
    before.retain(|p, _| inside(p));
    Ok(after != before)
}

/// The version of one file path in every snapshot that has it.
///
/// Answered from a single path-keyed lookup, which returns one entry per
/// snapshot holding the file (a few hundred) where a per-snapshot probe would
/// walk whole trees. This is what makes `history --file` and `blame` cheap.
///
/// For a delta layout it also holds the path's delta rows and the whole
/// `tree_deltas` base map, and resolves a snapshot by walking its chain in
/// memory: the newest delta row for the path wins, else the keyframe's.
#[derive(Default)]
pub(crate) struct PathVersions {
    /// The path's `file_map` rows, by keyframe.
    by_snapshot: HashMap<String, Entry>,
    /// The path's delta rows: `None` is a removal.
    delta: HashMap<String, Option<Entry>>,
    /// Every delta snapshot's base. Empty on a keyframe-only repository.
    base: HashMap<String, String>,
}

impl PathVersions {
    /// Load the versions of the file `path`. Empty when no snapshot has a
    /// file at exactly that path (including when it is only a directory).
    pub(crate) fn load(conn: &Connection, path: &str) -> Result<Self> {
        let mut stmt =
            conn.prepare_cached("SELECT snapshot_hash, hash, mode FROM file_map WHERE path = ?")?;
        let rows = stmt.query_map([path], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        let mut by_snapshot = HashMap::new();
        for row in rows {
            let (snapshot, entry): (String, Entry) = row?;
            by_snapshot.insert(snapshot, entry);
        }
        let mut delta = HashMap::new();
        let mut base = HashMap::new();
        if has_any_delta(conn)? {
            let mut stmt = conn.prepare_cached(
                "SELECT snapshot_hash, hash, mode FROM tree_delta_rows WHERE path = ?",
            )?;
            let rows = stmt.query_map([path], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?;
            for row in rows {
                let (snapshot, hash, mode) = row?;
                delta.insert(snapshot, hash.map(|h| (h, mode.unwrap_or(0))));
            }
            let mut stmt =
                conn.prepare_cached("SELECT snapshot_hash, base_hash FROM tree_deltas")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            for row in rows {
                let (snapshot, b): (String, String) = row?;
                base.insert(snapshot, b);
            }
        }
        Ok(PathVersions {
            by_snapshot,
            delta,
            base,
        })
    }

    /// Whether any stored tree holds a file below `path/`.
    pub(crate) fn is_directory(conn: &Connection, path: &str) -> Result<bool> {
        // '0' is the character after '/', so this range is exactly the paths
        // below `path/`, and it is served by the path index.
        for sql in [
            "SELECT 1 FROM file_map WHERE path >= ?1 || '/' AND path < ?1 || '0' LIMIT 1",
            "SELECT 1 FROM tree_delta_rows
              WHERE path >= ?1 || '/' AND path < ?1 || '0' AND hash IS NOT NULL LIMIT 1",
        ] {
            if conn.query_row(sql, [path], |_| Ok(())).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The version of the path in `snapshot`, or `None` when it is not there.
    /// A chain longer than [`MAX_CHAIN_LINKS`] (a cycle) reads as absent.
    pub(crate) fn at(&self, snapshot: &str) -> Option<Entry> {
        let mut cur = snapshot;
        for _ in 0..=MAX_CHAIN_LINKS {
            if let Some(entry) = self.delta.get(cur) {
                return entry.clone();
            }
            match self.base.get(cur) {
                Some(b) => cur = b,
                None => return self.by_snapshot.get(cur).cloned(),
            }
        }
        None
    }

    /// Whether no snapshot has the path.
    pub(crate) fn is_empty(&self) -> bool {
        self.by_snapshot.is_empty() && self.delta.values().all(Option::is_none)
    }
}

/// One path that differs between a snapshot and its first parent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TreeChange {
    pub(crate) path: String,
    /// The entry in the first parent, `None` when the path was added.
    pub(crate) before: Option<Entry>,
    /// The entry in the snapshot, `None` when the path was deleted.
    pub(crate) after: Option<Entry>,
}

/// What `snapshot` changed against its first parent, sorted by path. A root
/// snapshot (or one whose parent has no stored tree) is compared with the
/// empty tree, so every path is an addition. The second parent of a merge is
/// ignored.
#[allow(dead_code)]
pub(crate) fn changes_of(conn: &Connection, snapshot: &str) -> Result<Vec<TreeChange>> {
    let parent: String = conn
        .query_row(
            "SELECT parent_hash FROM snapshots WHERE hash = ?1
             UNION ALL SELECT parent_hash FROM trash WHERE hash = ?1",
            [snapshot],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let base: Option<String> = conn
        .prepare_cached("SELECT base_hash FROM tree_deltas WHERE snapshot_hash = ?")?
        .query_row([snapshot], |r| r.get(0))
        .ok();
    if !parent.is_empty() && base.as_deref() == Some(parent.as_str()) {
        // A delta on its first parent: its rows are the changes, so only the
        // `before` side needs a lookup in the parent.
        let mut stmt = conn.prepare_cached(
            "SELECT path, hash, mode FROM tree_delta_rows WHERE snapshot_hash = ?",
        )?;
        let rows: Vec<(String, Option<String>, Option<i64>)> = stmt
            .query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut changes = Vec::new();
        for (path, hash, mode) in rows {
            let after = hash.map(|h| (h, mode.unwrap_or(0)));
            let before = file_at(conn, &parent, &path)?;
            if before != after {
                changes.push(TreeChange {
                    path,
                    before,
                    after,
                });
            }
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        return Ok(changes);
    }
    let mut before = if parent.is_empty() {
        StoredTree::new()
    } else {
        tree_of(conn, &parent)?
    };
    let after = tree_of(conn, snapshot)?;
    let mut changes = Vec::new();
    for (path, entry) in after {
        match before.remove(&path) {
            Some(old) if old == entry => {}
            old => changes.push(TreeChange {
                path,
                before: old,
                after: Some(entry),
            }),
        }
    }
    for (path, old) in before {
        changes.push(TreeChange {
            path,
            before: Some(old),
            after: None,
        });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}
