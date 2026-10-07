//! Compaction: squash ranges of a branch's history into single snapshots.
//!
//! Checkpointing every agent step produces thousands of snapshots that matter
//! for an hour. This is the primitive that lets a caller fold a run of them into
//! one, and the record it leaves (decision D7, `docs/FORMAT.md` 11.3) means an
//! id someone is still holding answers [`Error::Compacted`] with the live id
//! rather than `NotFound`.
//!
//! **Store only**: nothing here reads or writes the working tree. The one file
//! touched is `.velo/PARENT`, and only to follow an id that was re-minted with an
//! identical tree, so the working tree stays consistent with it.
//!
//! # What gets rewritten
//!
//! A snapshot's id commits to its parent, so squashing a range changes the id of
//! every later snapshot on the chain. Those are *re-minted*: same tree, message,
//! timestamp, metadata and rename edges, new parent, new id. Every id that
//! stopped existing is recorded in `compactions`.
//!
//! # Events
//!
//! After the commit, [`Event::RefMoved`] is emitted for the branch and for each
//! tag that was retargeted. **No [`Event::Saved`] is emitted for a re-mint**: the
//! content is not new, and announcing it would tell a subscriber that history
//! grew when it shrank.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use rusqlite::params;

use crate::commands::{
    ancestors, branch_tip, load_snapshot_meta, snapshot_id, SnapshotIdentity, MAX_ANCESTRY_DEPTH,
};
use crate::error::{Error, Result};
use crate::events::{Event, Ref};
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::{BranchName, SnapshotId, TagName, WriteGuard};

/// An inclusive range on the branch's first-parent chain, `oldest..=newest`,
/// both recorded on that branch.
#[derive(Clone, Copy, Debug)]
pub struct Range<'a> {
    /// The oldest snapshot to squash.
    pub oldest: &'a SnapshotId,
    /// The newest snapshot to squash. Its tree, message, timestamp and metadata
    /// become the squashed snapshot's.
    pub newest: &'a SnapshotId,
}

/// How to run a compaction.
#[derive(Default)]
pub struct Options<'a> {
    /// Compute and return the result, and write nothing.
    pub dry_run: bool,
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between snapshots while planning, before anything is written.
    pub cancel: Option<&'a Cancel>,
}

/// One range, squashed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Squash {
    /// The snapshot the range became.
    pub into: SnapshotId,
    /// Every snapshot of the range, oldest first. All of them stopped existing.
    pub removed: Vec<SnapshotId>,
}

/// What a compaction did, or with `dry_run` what it would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compaction {
    /// The branch that was compacted.
    pub branch: BranchName,
    /// One entry per range, oldest first.
    pub squashed: Vec<Squash>,
    /// Snapshots re-minted only because an ancestor changed, as `(old, new)`.
    pub rewritten: Vec<(SnapshotId, SnapshotId)>,
    /// True when nothing was written.
    pub dry_run: bool,
}

/// Why a snapshot cannot be squashed away (or rewritten at all).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Protected {
    /// A tag names it.
    Tagged,
    /// It has a second parent.
    Merge,
    /// It is the parent or merge parent of a snapshot outside the rewrite.
    ParentOfOutside,
    /// It is reachable from a remote-tracking ref, so its id has been published.
    Published,
    /// `.velo/PARENT` names it.
    CheckedOut,
}

impl fmt::Display for Protected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Protected::Tagged => "tagged",
            Protected::Merge => "a merge",
            Protected::ParentOfOutside => "the parent of a snapshot outside the rewrite",
            Protected::Published => "reachable from a remote-tracking ref",
            Protected::CheckedOut => "the checked-out snapshot",
        })
    }
}

/// Every snapshot reachable from a remote-tracking ref.
pub(crate) fn published(conn: &rusqlite::Connection) -> Result<HashSet<String>> {
    let tips: Vec<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT hash FROM remote_refs")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut set = HashSet::new();
    for tip in tips {
        set.extend(ancestors(conn, &tip)?.into_keys());
    }
    Ok(set)
}

/// The context-free reasons, given the published set.
///
/// `ParentOfOutside` is not here: it depends on which snapshots are being
/// rewritten, so it is judged by [`outside_dependant`] against a rewrite set.
pub(crate) fn protection_in(
    conn: &rusqlite::Connection,
    checked_out: &str,
    published: &HashSet<String>,
    id: &str,
) -> Option<Protected> {
    let exists = |sql: &str| -> bool {
        conn.query_row(sql, [id], |r| r.get::<_, bool>(0))
            .unwrap_or(false)
    };
    if exists("SELECT EXISTS(SELECT 1 FROM tags WHERE snapshot_hash = ?)") {
        return Some(Protected::Tagged);
    }
    if exists("SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ? AND merge_parent <> '')") {
        return Some(Protected::Merge);
    }
    if published.contains(id) {
        return Some(Protected::Published);
    }
    // D7: signed snapshots are never squashed or re-minted, because re-minting
    // invalidates the signature. Signatures do not exist yet; the check goes here.
    if !checked_out.is_empty() && checked_out == id {
        return Some(Protected::CheckedOut);
    }
    None
}

pub(crate) fn checked_out(root: &Path) -> String {
    std::fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Eligibility check shared with retention: `None` when `id` may be squashed
/// away.
///
/// Covers the reasons that depend on the snapshot alone — tagged, a merge,
/// published, checked out. Whether it is the parent of something *outside* a
/// rewrite depends on the rewrite, so callers with a candidate set check that
/// separately (see `outside_dependant`).
#[allow(dead_code)] // Retention (14.5-b) is its caller; compact::run uses protection_in.
pub(crate) fn protection(conn: &rusqlite::Connection, root: &Path, id: &str) -> Option<Protected> {
    let published = published(conn).ok()?;
    protection_in(conn, &checked_out(root), &published, id)
}

/// A snapshot outside `rewrite` that has one inside it as a parent or merge
/// parent, as `(inside, outside)`. Trash counts: `redo` would resurrect it.
fn outside_dependant(
    conn: &rusqlite::Connection,
    rewrite: &HashSet<String>,
) -> Result<Option<(String, String)>> {
    for sql in [
        "SELECT hash, parent_hash, merge_parent FROM snapshots",
        "SELECT hash, parent_hash, merge_parent FROM trash",
    ] {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (hash, parent, merge) = row?;
            if rewrite.contains(&hash) {
                continue;
            }
            for p in [parent, merge] {
                if rewrite.contains(&p) {
                    return Ok(Some((p, hash)));
                }
            }
        }
    }
    Ok(None)
}

/// A chain member as stored.
struct Member {
    hash: String,
    parent: String,
    merge_parent: String,
    message: String,
    created_at_ms: i64,
}

/// A snapshot to insert, with where its rows come from.
struct Minted {
    hash: String,
    parent: String,
    merge_parent: String,
    message: String,
    created_at_ms: i64,
    /// Whose `file_map` and `snapshot_meta` rows it copies.
    tree_from: String,
    /// Its rename edges.
    renames: Vec<(String, String)>,
}

fn tree_of(conn: &rusqlite::Connection, id: &str) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn renames_of(conn: &rusqlite::Connection, id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT from_path, to_path FROM renames WHERE snapshot_hash = ? ORDER BY to_path",
    )?;
    let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Follow `from -> to` edges across `members` in order, keep those that land in
/// `final_tree` and start in `parent_tree`.
fn compose_renames(
    conn: &rusqlite::Connection,
    members: &[&Member],
    final_tree: &HashSet<&str>,
    parent_tree: &HashSet<String>,
) -> Result<Vec<(String, String)>> {
    // current path -> the path it started at, before the range.
    let mut origin: HashMap<String, String> = HashMap::new();
    for m in members {
        let edges = renames_of(conn, &m.hash)?;
        // Resolved against the state before this member, so a swap within one
        // snapshot composes correctly.
        let resolved: Vec<(String, String)> = edges
            .iter()
            .map(|(from, to)| {
                (
                    origin.get(from).cloned().unwrap_or_else(|| from.clone()),
                    to.clone(),
                )
            })
            .collect();
        for (from, _) in &edges {
            origin.remove(from);
        }
        for (start, to) in resolved {
            origin.insert(to, start);
        }
    }
    let mut out: Vec<(String, String)> = origin
        .into_iter()
        .filter(|(to, from)| from != to && final_tree.contains(to.as_str()))
        .filter(|(_, from)| parent_tree.contains(from))
        .map(|(to, from)| (from, to))
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

/// Squash `ranges` on `branch`, re-minting everything after the first of them.
///
/// Refuses the whole call with [`Error::InvalidInput`] — changing nothing — if a
/// range is malformed, or if any snapshot it would remove or rewrite is
/// protected; the message names the snapshot and the [`Protected`] reason.
///
/// Everything happens in one transaction.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// # let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
/// # let guard = repo.write()?;
/// # let (a, b) = (velo_core::commands::resolve_snapshot_id(&repo, "a")?,
/// #               velo_core::commands::resolve_snapshot_id(&repo, "b")?);
/// use velo_core::commands::compact::{self, Range};
///
/// let branch = "main".parse()?;
/// let done = compact::run(
///     &guard,
///     &branch,
///     &[Range { oldest: &a, newest: &b }],
///     Default::default(),
/// )?;
/// # let _ = done;
/// # Ok(()) }
/// ```
pub fn run(
    guard: &WriteGuard,
    branch: &BranchName,
    ranges: &[Range<'_>],
    options: Options<'_>,
) -> Result<Compaction> {
    let Options {
        dry_run,
        observer,
        cancel,
    } = options;
    let conn = guard.conn();
    let root = guard.root();

    let mut result = Compaction {
        branch: branch.clone(),
        squashed: Vec::new(),
        rewritten: Vec::new(),
        dry_run,
    };
    if ranges.is_empty() {
        return Ok(result);
    }

    // ── The chain: first parents from the tip, on this branch only ───────────
    let mut chain: Vec<Member> = Vec::new();
    let mut cursor = branch_tip(conn, branch.as_str()).unwrap_or_default();
    while !cursor.is_empty() && (chain.len() as i64) < MAX_ANCESTRY_DEPTH {
        let row = conn.query_row(
            "SELECT hash, parent_hash, merge_parent, message, created_at_ms
               FROM snapshots WHERE hash = ? AND branch = ?",
            params![cursor, branch],
            |r| {
                Ok(Member {
                    hash: r.get(0)?,
                    parent: r.get(1)?,
                    merge_parent: r.get(2)?,
                    message: r.get(3)?,
                    created_at_ms: r.get(4)?,
                })
            },
        );
        match row {
            Ok(m) => {
                cursor = m.parent.clone();
                chain.push(m);
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => break,
            Err(e) => return Err(e.into()),
        }
    }
    chain.reverse(); // oldest first
    let position: HashMap<&str, usize> = chain
        .iter()
        .enumerate()
        .map(|(i, m)| (m.hash.as_str(), i))
        .collect();

    // ── Validate the ranges ──────────────────────────────────────────────────
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for range in ranges {
        let locate = |id: &SnapshotId| -> Result<usize> {
            position.get(id.as_str()).copied().ok_or_else(|| {
                Error::invalid(format!(
                    "Cannot compact: snapshot {} is not on the first-parent chain of '{}'.",
                    id.short(),
                    branch
                ))
            })
        };
        let (lo, hi) = (locate(range.oldest)?, locate(range.newest)?);
        if lo > hi {
            return Err(Error::invalid(format!(
                "Cannot compact: {} is not an ancestor of {}.",
                range.oldest.short(),
                range.newest.short()
            )));
        }
        if hi == lo {
            return Err(Error::invalid(format!(
                "Cannot compact: the range at {} holds one snapshot; a range needs at least 2.",
                range.oldest.short()
            )));
        }
        spans.push((lo, hi));
    }
    spans.sort();
    for pair in spans.windows(2) {
        if pair[1].0 <= pair[0].1 {
            return Err(Error::invalid(format!(
                "Cannot compact: the ranges {}..{} and {}..{} overlap.",
                &chain[pair[0].0].hash[..16],
                &chain[pair[0].1].hash[..16],
                &chain[pair[1].0].hash[..16],
                &chain[pair[1].1].hash[..16],
            )));
        }
    }

    // ── Eligibility ──────────────────────────────────────────────────────────
    let start = spans[0].0;
    let rewrite: HashSet<String> = chain[start..].iter().map(|m| m.hash.clone()).collect();
    let published = published(conn)?;
    let checked_out = checked_out(root);
    let refuse = |id: &str, why: Protected| {
        Error::invalid(format!(
            "Cannot compact: snapshot {} is {}.",
            &id[..16.min(id.len())],
            why
        ))
    };

    for &(lo, hi) in &spans {
        for (i, m) in chain.iter().enumerate().take(hi + 1).skip(lo) {
            match protection_in(conn, &checked_out, &published, &m.hash) {
                // The checked-out snapshot may be the newest of a range: it
                // becomes the squashed snapshot, with the same tree.
                Some(Protected::CheckedOut) if i == hi => {}
                Some(why) => return Err(refuse(&m.hash, why)),
                None => {}
            }
        }
    }
    for m in &chain[start..] {
        if published.contains(&m.hash) {
            return Err(refuse(&m.hash, Protected::Published));
        }
    }
    if let Some((inside, _outside)) = outside_dependant(conn, &rewrite)? {
        return Err(refuse(&inside, Protected::ParentOfOutside));
    }

    // ── Plan: mint every new snapshot, oldest first ──────────────────────────
    let progress = PhaseGuard::new(
        observer.unwrap_or_else(|| guard.repo().observer()),
        Phase::Replaying,
        Some(rewrite.len() as u64),
    );
    let mut mapping: HashMap<String, String> = HashMap::new(); // old -> new
    let mut minted: Vec<Minted> = Vec::new();
    let mut i = start;
    while i < chain.len() {
        Cancel::check(cancel)?;
        let span = spans.iter().find(|s| s.0 == i).copied();
        let remap = |id: &str| mapping.get(id).cloned().unwrap_or_else(|| id.to_string());

        let (members, source) = match span {
            Some((lo, hi)) => (&chain[lo..=hi], &chain[hi]),
            None => (&chain[i..=i], &chain[i]),
        };
        let parent = remap(&members[0].parent);
        let merge_parent = remap(&source.merge_parent);
        let tree = tree_of(conn, &source.hash)?;
        let meta = load_snapshot_meta(conn, &source.hash)?;
        let new_id = snapshot_id(SnapshotIdentity {
            tree: &tree,
            parent: &parent,
            merge_parent: &merge_parent,
            message: &source.message,
            timestamp_ms: source.created_at_ms,
            meta: &meta,
        });

        let renames = if span.is_some() {
            let final_tree: HashSet<&str> = tree.iter().map(|t| t.0.as_str()).collect();
            let parent_tree: HashSet<String> = if members[0].parent.is_empty() {
                HashSet::new()
            } else {
                tree_of(conn, &members[0].parent)?
                    .into_iter()
                    .map(|t| t.0)
                    .collect()
            };
            let refs: Vec<&Member> = members.iter().collect();
            compose_renames(conn, &refs, &final_tree, &parent_tree)?
        } else {
            renames_of(conn, &source.hash)?
        };

        for m in members {
            mapping.insert(m.hash.clone(), new_id.clone());
            progress.tick();
        }
        if span.is_some() {
            result.squashed.push(Squash {
                into: SnapshotId::from_stored(new_id.clone()),
                removed: members
                    .iter()
                    .map(|m| SnapshotId::from_stored(m.hash.clone()))
                    .collect(),
            });
        } else {
            result.rewritten.push((
                SnapshotId::from_stored(source.hash.clone()),
                SnapshotId::from_stored(new_id.clone()),
            ));
        }
        minted.push(Minted {
            hash: new_id,
            parent,
            merge_parent,
            message: source.message.clone(),
            created_at_ms: source.created_at_ms,
            tree_from: source.hash.clone(),
            renames,
        });
        i += members.len();
    }
    Cancel::check(cancel)?;

    if dry_run {
        return Ok(result);
    }

    // ── Apply, in one transaction ────────────────────────────────────────────
    let now = crate::commands::snapshot_timestamp_ms();
    let old_tip = chain.last().map(|m| m.hash.clone()).unwrap_or_default();
    let new_tip = mapping.get(&old_tip).cloned().unwrap_or_default();

    let mut retargeted: Vec<(String, String, String)> = Vec::new(); // (tag, old, new)
    let tx = guard.transaction()?;
    for m in &minted {
        tx.execute(
            "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![m.hash, m.message, branch, m.parent, m.merge_parent, m.created_at_ms],
        )?;
        tx.execute(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode)
             SELECT ?, path, hash, mode FROM file_map WHERE snapshot_hash = ?",
            params![m.hash, m.tree_from],
        )?;
        tx.execute(
            "INSERT INTO snapshot_meta (snapshot_id, namespace, key, value)
             SELECT ?, namespace, key, value FROM snapshot_meta WHERE snapshot_id = ?",
            params![m.hash, m.tree_from],
        )?;
        for (from, to) in &m.renames {
            tx.execute(
                "INSERT INTO renames (snapshot_hash, from_path, to_path) VALUES (?, ?, ?)",
                params![m.hash, from, to],
            )?;
        }
    }
    for old in mapping.keys() {
        // Old rows go after the new ones are in, so a copy above never reads a
        // tree that has already been deleted.
        tx.execute("DELETE FROM snapshots WHERE hash = ?", [old])?;
        tx.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [old])?;
        tx.execute("DELETE FROM snapshot_meta WHERE snapshot_id = ?", [old])?;
        tx.execute("DELETE FROM renames WHERE snapshot_hash = ?", [old])?;
    }
    for (old, new) in &mapping {
        {
            let mut stmt = tx.prepare("SELECT name FROM tags WHERE snapshot_hash = ?")?;
            let names: Vec<String> = stmt
                .query_map([old], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for name in names {
                retargeted.push((name, old.clone(), new.clone()));
            }
        }
        tx.execute(
            "UPDATE tags SET snapshot_hash = ? WHERE snapshot_hash = ?",
            params![new, old],
        )?;
        tx.execute(
            "UPDATE branches SET tip = ? WHERE tip = ?",
            params![new, old],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO compactions (old_hash, new_hash, compacted_at_ms)
             VALUES (?, ?, ?)",
            params![old, new, now],
        )?;
    }
    tx.commit()?;

    // The tree is identical, so pointing PARENT at the new id keeps the working
    // tree consistent with it.
    if let Some(new) = mapping.get(&checked_out) {
        crate::storage::write_atomic(&root.join(".velo/PARENT"), new.as_bytes())?;
    }

    let repo = guard.repo();
    repo.emit(Event::RefMoved {
        reference: Ref::Branch(branch.clone()),
        from: Some(SnapshotId::from_stored(old_tip)),
        to: Some(SnapshotId::from_stored(new_tip)),
    });
    retargeted.sort();
    for (name, old, new) in retargeted {
        repo.emit(Event::RefMoved {
            reference: Ref::Tag(TagName::from_stored(name)),
            from: Some(SnapshotId::from_stored(old)),
            to: Some(SnapshotId::from_stored(new)),
        });
    }

    Ok(result)
}
