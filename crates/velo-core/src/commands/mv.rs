//! `velo mv <from> <to>` — move a tracked file and record that it moved.
//!
//! Velo has no staging area: what is on disk is what gets saved. A rename is the
//! one thing that principle cannot express, because it is not a state of the
//! disk at all. After the move, the working tree looks exactly as it would if
//! the old file had been deleted and a new one written — and no amount of
//! looking at it afterwards recovers which of those two it was.
//!
//! So the fact is recorded at the only moment it exists, and held until the next
//! save picks it up. That is a staging area for exactly one kind of fact, and
//! the narrowness is the point: it holds no content, and losing it costs
//! provenance rather than work.
//!
//! A consumer using [`SaveTree`](crate::tree::SaveTree) does not go through
//! this — it passes its edges to the save directly, because it never had a
//! working tree to move anything in.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::db;
use crate::error::{Result, VeloError};
use crate::WriteGuard;

/// What a move did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub from: PathBuf,
    pub to: PathBuf,
    /// True when this move continued one already pending, and the two were
    /// folded into a single edge from the original name.
    pub extended_a_pending_move: bool,
}

/// Move `from` to `to`, and record the rename for the next save.
///
/// Refuses when `to` already exists: overwriting is a different operation with a
/// different failure mode, and doing it silently as part of a move would destroy
/// content that nothing has recorded yet.
pub fn run(guard: &WriteGuard, from: &Path, to: &Path) -> Result<Moved> {
    let root = guard.root();
    let from_rel = db::normalise(&from.to_string_lossy());
    let to_rel = db::normalise(&to.to_string_lossy());

    if from_rel.is_empty() || to_rel.is_empty() {
        return Err(VeloError::invalid(
            "Both a source and a destination are needed.",
        ));
    }
    if from_rel == to_rel {
        return Err(VeloError::invalid(format!(
            "'{}' is already where it is.",
            from_rel
        )));
    }

    let from_abs = root.join(db::db_to_path(&from_rel));
    let to_abs = root.join(db::db_to_path(&to_rel));

    if !from_abs.exists() {
        return Err(VeloError::invalid(format!(
            "'{}' does not exist.",
            from_rel
        )));
    }
    if from_abs.is_dir() {
        // A directory move is several file moves, and recording it as one edge
        // would describe a path no `file_map` row has. Refused rather than
        // half-recorded; the caller can move the files.
        return Err(VeloError::invalid(format!(
            "'{}' is a directory. Move the files inside it instead.",
            from_rel
        )));
    }
    if to_abs.exists() {
        return Err(VeloError::invalid(format!(
            "'{}' already exists. Remove it first if you mean to replace it.",
            to_rel
        )));
    }

    if let Some(parent) = to_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&from_abs, &to_abs)?;

    let conn = guard.conn();
    // `mv a b` then `mv b c` is one move from a to c. Left as two edges the
    // middle name would be a step in the file's history that never appeared in
    // any snapshot, and the walk would look for it in a tree that never had it.
    let earlier: Option<String> = conn
        .query_row(
            "SELECT from_path FROM pending_renames WHERE to_path = ?",
            [&to_rel],
            |r| r.get(0),
        )
        .ok();
    let earlier: Option<String> = match earlier {
        Some(_) => earlier,
        None => conn
            .query_row(
                "SELECT from_path FROM pending_renames WHERE to_path = ?",
                [&from_rel],
                |r| r.get(0),
            )
            .ok(),
    };
    let extended = earlier.is_some();
    let origin = earlier.unwrap_or_else(|| from_rel.clone());

    conn.execute("DELETE FROM pending_renames WHERE to_path = ?", [&from_rel])?;
    // A move back to where a file started is not a move. Recording `a → a`
    // would fail `save_tree`'s own check, and rightly.
    if origin == to_rel {
        conn.execute("DELETE FROM pending_renames WHERE to_path = ?", [&to_rel])?;
    } else {
        conn.execute(
            "INSERT OR REPLACE INTO pending_renames (from_path, to_path) VALUES (?, ?)",
            params![origin, to_rel],
        )?;
    }

    crate::commands::invalidate_cache_entries(guard.repo(), &[from_rel.clone(), to_rel.clone()]);

    Ok(Moved {
        from: PathBuf::from(from_rel),
        to: PathBuf::from(to_rel),
        extended_a_pending_move: extended,
    })
}

/// The moves waiting for a save, as `(from, to)`.
pub fn pending(guard: &WriteGuard) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut stmt = guard
        .conn()
        .prepare("SELECT from_path, to_path FROM pending_renames ORDER BY to_path")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            PathBuf::from(r.get::<_, String>(0)?),
            PathBuf::from(r.get::<_, String>(1)?),
        ))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Forget every pending move.
///
/// For the commands that rewrite the working tree wholesale — `switch`,
/// `restore`, `merge --abort`. The tree they write is not the tree the edges
/// describe, so keeping them would attach a move to a snapshot that never made
/// one, which is exactly what `fsck` reports.
pub(crate) fn discard_pending(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM pending_renames", [])?;
    Ok(())
}

/// Move the pending edges that this snapshot's tree actually reflects into
/// `renames`, and forget them.
///
/// An edge counts as reflected when the destination is in the new tree and the
/// source is not — that is what a move looks like once it has happened. One that
/// does not is left pending rather than dropped: `velo mv a b` followed by a
/// save restricted to other paths is a reasonable thing to do, and the move is
/// still going to happen.
pub(crate) fn apply_pending(
    tx: &rusqlite::Connection,
    snapshot: &str,
    tree: &[(String, String, i64)],
) -> Result<()> {
    let paths: std::collections::HashSet<&str> = tree.iter().map(|(p, _, _)| p.as_str()).collect();

    let pending: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT from_path, to_path FROM pending_renames")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    let mut ins = tx.prepare(
        "INSERT OR IGNORE INTO renames (snapshot_hash, from_path, to_path) VALUES (?, ?, ?)",
    )?;
    let mut done = tx.prepare("DELETE FROM pending_renames WHERE to_path = ?")?;
    for (from, to) in pending {
        if paths.contains(to.as_str()) && !paths.contains(from.as_str()) {
            ins.execute(params![snapshot, from, to])?;
            done.execute([&to])?;
        }
    }
    Ok(())
}
