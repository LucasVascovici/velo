//! Following a file across the renames in its past.
//!
//! A snapshot is a whole tree, so a move leaves no trace in `file_map` — the old
//! path is simply absent and the new one simply present, exactly as a delete and
//! an add would look. Velo therefore records the move when it happens (see
//! [`SaveTree::renames`](crate::tree::SaveTree::renames)) and this module is the
//! one place that reads those edges back.
//!
//! One implementation, several consumers: `blame` needs it per step of its walk,
//! a path filter on `history` needs the whole list up front, and `show` needs to
//! name the move. Written three times it would go wrong in three ways.
//!
//! # Why the starting point is a parameter
//!
//! Following renames backwards needs to know where "backwards" starts from: a
//! path is only a name *as of* some snapshot, and the edges that apply are the
//! ones on the way from there to the point being asked about. The obvious
//! three-argument shape — `path_at(repo, path, snapshot)` — hides that endpoint,
//! and would have to invent one. Reading `.velo/PARENT` is exactly the wrong
//! guess for a consumer using `save_tree`, which deliberately never writes it.
//!
//! So the anchor is passed in — the snapshot the caller is looking at. A
//! headless consumer passes the tip it is working from, which
//! [`Repo::branch_tip`](crate::Repo::branch_tip) gives it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::db;
use crate::error::Result;
use crate::{Repo, SnapshotId};

/// A name a file has had, and the snapshot that gave it that name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathAlias {
    /// The path the file was known by.
    pub path: PathBuf,
    /// The snapshot that renamed the file *to* [`path`](PathAlias::path). The
    /// name holds from there forward, until the next rename.
    ///
    /// Empty for the oldest name reached, which nothing on this walk renamed to
    /// — the name the file was created with, unless the walk ran out first.
    pub renamed_by: SnapshotId,
}

impl PathAlias {
    /// True when nothing on the walk gave the file this name.
    pub fn is_original(&self) -> bool {
        self.renamed_by.is_empty()
    }
}

/// Every name a file has had, newest first, walking back from `from`.
///
/// The first entry is `path` itself; entries after it are the names it had
/// before, each paired with the snapshot that renamed the file *to* the name
/// above it. The last entry is always the oldest name reached, with no snapshot
/// — so a file that was never renamed yields exactly one entry, which is the
/// common case and costs one indexed lookup per snapshot walked.
///
/// Follows first parents only. A rename recorded on a branch that was merged in
/// is not on this chain, and the alternative — following both parents — can
/// yield two different histories for one name, with nothing to choose between
/// them. Reporting the one that leads to `from` is the answer that matches what
/// the caller is looking at.
pub fn aliases(repo: &Repo, path: &Path, from: &SnapshotId) -> Result<Vec<PathAlias>> {
    let conn = repo.conn();
    let mut current = db::normalise(&path.to_string_lossy());
    let mut out = Vec::new();
    let mut walk = from.to_string();
    // Guards a history that a corrupt parent chain has made cyclic; `fsck`
    // reports that separately, and a blame should not hang while it does.
    let mut seen = HashSet::new();

    while !walk.is_empty() && seen.insert(walk.clone()) {
        if let Some(previous) = renamed_from(conn, &walk, &current) {
            out.push(PathAlias {
                path: PathBuf::from(std::mem::replace(&mut current, previous)),
                renamed_by: SnapshotId::from_stored(walk.clone()),
            });
        }
        walk = first_parent(conn, &walk);
    }

    // The oldest name has no rename after it within this walk.
    out.push(PathAlias {
        path: PathBuf::from(current),
        renamed_by: SnapshotId::from_stored(String::new()),
    });
    Ok(out)
}

/// The name `path` was known by at `at`, following renames back from `from`.
///
/// `path` is the name as of `from`. Returns it unchanged when nothing on the way
/// renamed the file, and when `at` is not on the first-parent chain from `from`
/// — none of the edges apply there, and there is nothing better to say.
///
/// Walks rather than indexing into [`aliases`]: pairing an alias with a snapshot
/// means knowing how far each is from `from`, and a depth taken from
/// [`crate::commands::ancestors`] counts through *both* parents, so a merge can
/// make an older snapshot look nearer than the chain says it is. Walking the one
/// chain answers the question directly.
pub fn path_at(repo: &Repo, path: &Path, from: &SnapshotId, at: &SnapshotId) -> Result<PathBuf> {
    let conn = repo.conn();
    let original = db::normalise(&path.to_string_lossy());
    let mut current = original.clone();
    let mut walk = from.to_string();
    let mut seen = HashSet::new();

    while !walk.is_empty() && seen.insert(walk.clone()) {
        // Checked before stepping to the parent: a rename recorded on `walk`
        // means `walk` already holds the new name, so the name at `walk` is
        // whatever `current` is on arrival.
        if walk == at.as_str() {
            return Ok(PathBuf::from(current));
        }
        if let Some(previous) = renamed_from(conn, &walk, &current) {
            current = previous;
        }
        walk = first_parent(conn, &walk);
    }
    Ok(PathBuf::from(original))
}

/// The name a file had before `snapshot` renamed it to `to_path`.
pub(crate) fn renamed_from(
    conn: &rusqlite::Connection,
    snapshot: &str,
    to_path: &str,
) -> Option<String> {
    conn.query_row(
        "SELECT from_path FROM renames WHERE snapshot_hash = ? AND to_path = ?",
        params![snapshot, to_path],
        |r| r.get(0),
    )
    .ok()
}

fn first_parent(conn: &rusqlite::Connection, snapshot: &str) -> String {
    conn.query_row(
        "SELECT parent_hash FROM snapshots WHERE hash = ?",
        [snapshot],
        |r| r.get::<_, String>(0),
    )
    .unwrap_or_default()
}

/// Every name `path` has had, as normalised strings, for a query filter.
///
/// The shape [`crate::commands::history`] wants: it compares stored `file_map`
/// paths, which are normalised strings, and does not care which snapshot each
/// name belonged to.
pub(crate) fn alias_strings(repo: &Repo, path: &Path, from: &SnapshotId) -> Result<Vec<String>> {
    Ok(aliases(repo, path, from)?
        .into_iter()
        .map(|a| db::normalise(&a.path.to_string_lossy()))
        .collect())
}

/// The rename edges `snapshot` recorded, as `(from, to)`.
pub fn recorded_by(repo: &Repo, snapshot: &SnapshotId) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut stmt = repo.conn().prepare(
        "SELECT from_path, to_path FROM renames WHERE snapshot_hash = ? ORDER BY to_path",
    )?;
    let rows = stmt.query_map([snapshot], |r| {
        Ok((
            PathBuf::from(r.get::<_, String>(0)?),
            PathBuf::from(r.get::<_, String>(1)?),
        ))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
