//! `velody repack` — convert stored full snapshots into deltas, storage only.
//!
//! A repository written before deltas (or at a keyframe interval of 1) keeps
//! every snapshot's whole tree in `file_map`. Migrating to the v4 layout is
//! O(1) because it rewrites nothing, which also means it shrinks nothing; this
//! command does the rewrite, a few snapshots at a time.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::tree_store::{self, StoredTree};
use crate::Repo;

/// How to run a repack.
#[non_exhaustive]
pub struct Options<'a> {
    /// Report what a repack would save and write nothing. The whole plan runs
    /// inside one transaction that is rolled back, so the counts match a real
    /// run made straight afterwards.
    ///
    /// Bytes are estimated, not measured: each stored row is costed at the
    /// length of its path and hash plus 8 bytes of mode and row overhead, and
    /// the saving is the drop in that cost between the rows before and after.
    /// Index pages and page slack are ignored, so the figure is a lower bound
    /// on the reclaim that the vacuum of a real run makes.
    pub dry_run: bool,
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between chunks. Cancelling returns [`Error::Cancelled`] and
    /// leaves a valid, partly repacked repository: every chunk is its own
    /// committed transaction, and a later run finishes the job.
    pub cancel: Option<&'a Cancel>,
    /// Snapshots converted per transaction, at least 1. The write lock is held
    /// for one chunk at a time, so a concurrent save waits at most that long.
    pub chunk: usize,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options {
            dry_run: false,
            observer: None,
            cancel: None,
            chunk: 64,
        }
    }
}

/// What a repack did, or for a dry run what it would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Repacked {
    /// Keyframes rewritten as deltas.
    pub snapshots_converted: usize,
    /// Snapshots that are keyframes afterwards.
    pub keyframes_kept: usize,
    /// Deltas deeper than `K - 1` turned back into keyframes, so every chain
    /// is within the interval.
    pub chains_shortened: usize,
    /// Rows holding trees (`file_map` plus `tree_delta_rows`) before.
    pub rows_before: u64,
    /// The same count after.
    pub rows_after: u64,
    /// Database size before: measured, in both a real and a dry run.
    pub bytes_before: u64,
    /// Database size after: measured after the vacuum, or for a dry run
    /// estimated as described on [`Options::dry_run`].
    pub bytes_after: u64,
    /// This was a dry run: nothing was written.
    pub dry_run: bool,
}

/// Convert existing keyframes into deltas, in short resumable transactions.
///
/// Each chunk takes the write lock, converts up to `chunk` snapshots and
/// commits, so the repository is valid after every chunk and a kill costs only
/// the remaining work; the lock is released between chunks, so a concurrent
/// save is not starved. A final incremental vacuum returns the freed pages.
///
/// This is storage only. It never touches `snapshots`, signatures, renames,
/// tags or ids, and a snapshot's tree reads back identical, so nothing a
/// signature or a published id covers can change; a protected snapshot is
/// converted like any other because its id and tree are unchanged. Snapshots
/// are visited parents first and a delta's base is always the first parent, as
/// a save writes it, keeping every chain within `K - 1` links. Running it
/// twice converts nothing the second time.
pub fn run(repo: &Repo, options: Options<'_>) -> Result<Repacked> {
    let Options {
        dry_run,
        observer,
        cancel,
        chunk,
    } = options;
    let chunk = chunk.max(1);
    let k = repo.keyframe_interval();

    let (order, mut out) = {
        let guard = repo.write()?;
        let conn = guard.conn();
        let order = plan_order(conn)?;
        let out = Repacked {
            rows_before: measure(conn)?.0,
            dry_run,
            bytes_before: page_bytes(conn)?,
            ..Default::default()
        };
        (order, out)
    };

    let progress = PhaseGuard::new(
        observer.unwrap_or_else(|| repo.observer()),
        Phase::Repacking,
        Some(order.len() as u64),
    );

    if dry_run {
        let guard = repo.write()?;
        let tx = guard.conn().unchecked_transaction()?;
        let (_, cost_before) = measure(&tx)?;
        let mut at = 0;
        while at < order.len() {
            at = step(&tx, &order, at, chunk, k, &mut out)?;
        }
        let (rows, cost_after) = measure(&tx)?;
        out.rows_after = rows;
        out.bytes_after = out
            .bytes_before
            .saturating_sub(cost_before.saturating_sub(cost_after));
        out.keyframes_kept = count_keyframes(&tx, &order)?;
        // Dropped without a commit: nothing persists.
        drop(tx);
        progress.advance(order.len() as u64);
        return Ok(out);
    }

    let mut at = 0;
    while at < order.len() {
        let before = at;
        {
            let guard = repo.write()?;
            let tx = guard.conn().unchecked_transaction()?;
            at = step(&tx, &order, at, chunk, k, &mut out)?;
            tx.commit()?;
        }
        progress.advance((at - before) as u64);
        crate::failpoint::hit("repack.after_chunk");
        if cancel.is_some_and(Cancel::is_cancelled) && at < order.len() {
            return Err(Error::Cancelled);
        }
    }

    let guard = repo.write()?;
    out.keyframes_kept = count_keyframes(guard.conn(), &order)?;
    out.rows_after = measure(guard.conn())?.0;
    let vac = super::gc::vacuum(&guard, super::gc::Vacuum::Incremental)?;
    out.bytes_after = vac.bytes_after;
    Ok(out)
}

/// Every snapshot with a stored tree, live or trashed, parents first, so a
/// parent is converted before its child looks at its depth. Within a
/// generation the order is by id, which is stable across runs.
fn plan_order(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut parent: HashMap<String, String> = HashMap::new();
    for table in ["snapshots", "trash"] {
        let mut stmt = conn.prepare(&format!("SELECT hash, parent_hash FROM {table}"))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (h, p) = row?;
            parent.insert(h, p);
        }
    }
    // Generation 0 is a root or a snapshot whose parent is unknown.
    let mut gen: HashMap<&str, usize> = HashMap::new();
    for start in parent.keys() {
        let mut path = vec![start.as_str()];
        let mut base = 0;
        // The bound stops a cycle in damaged data; fsck reports those.
        while path.len() <= parent.len() {
            let cur = path[path.len() - 1];
            if let Some(g) = gen.get(cur) {
                base = *g + 1;
                path.pop();
                break;
            }
            match parent.get_key_value(parent[cur].as_str()) {
                Some((p, _)) => path.push(p.as_str()),
                None => break,
            }
        }
        for node in path.into_iter().rev() {
            gen.insert(node, base);
            base += 1;
        }
    }
    let mut order: Vec<(usize, String, String)> = tree_store::stored_snapshots(conn)?
        .into_iter()
        .filter_map(|h| {
            let p = parent.get(&h)?.clone();
            Some((gen.get(h.as_str()).copied().unwrap_or(0), h, p))
        })
        .collect();
    order.sort();
    Ok(order.into_iter().map(|(_, h, p)| (h, p)).collect())
}

/// Converts or shortens snapshots from `order[at..]` until `chunk` of them
/// changed or the list ends, and returns the next position.
fn step(
    tx: &Connection,
    order: &[(String, String)],
    mut at: usize,
    chunk: usize,
    k: u32,
    out: &mut Repacked,
) -> Result<usize> {
    let mut changed = 0;
    let mut samples: Vec<(&str, StoredTree)> = Vec::new();
    while at < order.len() && changed < chunk {
        let (snap, parent) = &order[at];
        at += 1;
        if cfg!(debug_assertions) && samples.len() < 4 {
            if let Ok(t) = tree_store::tree_of(tx, snap) {
                samples.push((snap, t));
            }
        }
        let parent = tree_store::first_parent(parent);
        if tree_store::convert_to_delta(tx, snap, parent, k)? {
            out.snapshots_converted += 1;
            changed += 1;
        } else if tree_store::promote_overlong(tx, snap, k)? {
            out.chains_shortened += 1;
            changed += 1;
        }
    }
    // Storage must never change a tree: re-read a sample through its new rows.
    for (snap, before) in &samples {
        debug_assert!(
            tree_store::tree_of(tx, snap).is_ok_and(|after| after == *before),
            "repack changed the tree of {snap}"
        );
    }
    Ok(at)
}

fn count_keyframes(conn: &Connection, order: &[(String, String)]) -> Result<usize> {
    let mut n = 0;
    for (h, _) in order {
        if tree_store::is_keyframe(conn, h)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Tree rows and their estimated cost in bytes.
fn measure(conn: &Connection) -> Result<(u64, u64)> {
    tree_store::row_stats(conn)
}

fn page_bytes(conn: &Connection) -> Result<u64> {
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages.max(0) as u64 * size.max(0) as u64)
}
