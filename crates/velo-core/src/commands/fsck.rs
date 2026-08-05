//! `velo fsck` — verify repository integrity.
//!
//! Checks the invariants the rest of Velo (and sync) relies on: every referenced
//! object exists and re-hashes to its own name; every snapshot's parents resolve;
//! content-addressed snapshot ids recompute correctly; and every ref (PARENT,
//! tags, stash) points somewhere real.
//!
//! Findings are returned as typed values rather than pre-formatted lines, so a
//! consumer can branch on them — "which objects are missing?" is a question you
//! can answer from a [`Report`] without parsing text. Rendering and the exit code
//! live in `velo-cli`.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::error::Result;
use crate::progress::Phase;
use crate::storage;
use crate::Repo;
use crate::WriteGuard;

/// An integrity failure. Any of these means the repository is damaged.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Problem {
    /// An object is referenced but absent from the store.
    MissingObject { hash: String },
    /// An object's content no longer hashes to its own name.
    CorruptObject { hash: String, actual: String },
    /// An object exists but could not be decompressed.
    UndecodableObject { hash: String },
    /// A snapshot names a parent that doesn't exist.
    MissingParent { snapshot: String, parent: String },
    /// A merge commit names a second parent that doesn't exist.
    MissingMergeParent {
        snapshot: String,
        merge_parent: String,
    },
    /// A content-addressed snapshot id doesn't match what its content hashes to.
    IdMismatch {
        snapshot: String,
        message: String,
        branch: String,
        recomputed: String,
    },
    /// `.velo/PARENT` points at a snapshot that doesn't exist.
    DanglingPosition { hash: String },
    /// A tag or shelf points at a snapshot that doesn't exist.
    DanglingRef {
        table: String,
        name: String,
        hash: String,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::MissingObject { hash } => write!(
                f,
                "object {} is referenced but missing from the store",
                hash
            ),
            Problem::CorruptObject { hash, actual } => write!(
                f,
                "object {} is corrupt — its content hashes to {}",
                hash,
                &actual[..16.min(actual.len())]
            ),
            Problem::UndecodableObject { hash } => {
                write!(f, "object {} could not be decompressed (corrupt)", hash)
            }
            Problem::MissingParent { snapshot, parent } => write!(
                f,
                "snapshot {} has parent {} which does not exist",
                snapshot, parent
            ),
            Problem::MissingMergeParent {
                snapshot,
                merge_parent,
            } => write!(
                f,
                "snapshot {} has merge parent {} which does not exist",
                snapshot, merge_parent
            ),
            Problem::IdMismatch {
                snapshot,
                message,
                branch,
                recomputed,
            } => write!(
                f,
                "snapshot {} (\"{}\", branch {}) does not match its content (recomputed {})",
                snapshot, message, branch, recomputed
            ),
            Problem::DanglingPosition { hash } => {
                write!(f, "PARENT points to {} which does not exist", hash)
            }
            Problem::DanglingRef { table, name, hash } => write!(
                f,
                "{} '{}' points to snapshot {} which does not exist",
                table, name, hash
            ),
        }
    }
}

/// Cruft that isn't corruption. Safe to leave; `--repair` cleans it up.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cruft {
    /// Hunk decisions left behind for files no longer in conflict.
    OrphanHunkDecisions(usize),
    /// Shelved tags whose snapshot is gone from both history and the trash.
    OrphanShelvedTags(usize),
    /// Conflict rows with no merge in progress.
    BrokenConflictState(usize),
    /// Tracking refs pointing at snapshots this repo doesn't have. Expected after
    /// `gc` prunes history only a stale tracking ref reached; the next fetch
    /// re-establishes them.
    StaleRemoteRefs(usize),
    /// Tracking refs for a remote that has since been removed.
    OrphanRemoteRefs(usize),
}

impl Cruft {
    /// How to describe this as an outstanding issue.
    pub fn describe(&self) -> String {
        match self {
            Cruft::OrphanHunkDecisions(n) => format!("{} orphaned hunk-decision row(s)", n),
            Cruft::OrphanShelvedTags(n) => format!("{} shelved tag(s) with no snapshot", n),
            Cruft::BrokenConflictState(n) => format!(
                "{} conflict row(s) with no merge in progress (broken merge state)",
                n
            ),
            Cruft::StaleRemoteRefs(n) => format!(
                "{} remote-tracking ref(s) pointing at absent snapshots (re-fetch to refresh)",
                n
            ),
            Cruft::OrphanRemoteRefs(n) => {
                format!("{} remote-tracking ref(s) for removed remote(s)", n)
            }
        }
    }

    /// How to describe it once cleaned up.
    pub fn describe_repaired(&self) -> String {
        match self {
            Cruft::OrphanHunkDecisions(n) => {
                format!("pruned {} orphaned hunk-decision row(s)", n)
            }
            Cruft::OrphanShelvedTags(n) => format!("pruned {} orphaned shelved tag(s)", n),
            Cruft::BrokenConflictState(_) => "cleared broken conflict state".into(),
            Cruft::StaleRemoteRefs(n) => format!("pruned {} stale remote-tracking ref(s)", n),
            Cruft::OrphanRemoteRefs(n) => {
                format!("pruned {} remote-tracking ref(s) for removed remote(s)", n)
            }
        }
    }
}

/// A stage of the check, with the counts worth reporting.
///
/// Deliberately *not* `#[non_exhaustive]`, unlike [`Problem`] and [`Cruft`]:
/// those are findings a consumer inspects selectively, whereas a renderer must
/// describe every stage. Adding one here should break renderers until they
/// account for it, rather than have the new stage silently vanish from reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    Objects {
        referenced: usize,
        verified: usize,
        problems: usize,
    },
    Snapshots {
        checked: usize,
        ids_verified: usize,
        /// Snapshots predating content-addressed ids, which can't be verified.
        ids_legacy: usize,
        problems: usize,
    },
    Refs {
        problems: usize,
    },
    State {
        outstanding: usize,
        repaired: bool,
    },
}

impl Section {
    pub fn problems(&self) -> usize {
        match self {
            Section::Objects { problems, .. }
            | Section::Snapshots { problems, .. }
            | Section::Refs { problems } => *problems,
            Section::State { outstanding, .. } => *outstanding,
        }
    }
}

/// The result of a check.
#[derive(Clone, Debug)]
pub struct Report {
    /// Stages in the order they ran.
    pub sections: Vec<Section>,
    /// Integrity failures. Non-empty means the repository is damaged.
    pub problems: Vec<Problem>,
    /// Cruft still present. Empty after a successful repair.
    pub cruft: Vec<Cruft>,
    /// What repair actually cleaned up.
    pub repaired: Vec<Cruft>,
    /// Whether repair was requested.
    pub repair_requested: bool,
}

impl Report {
    /// No corruption found. Cruft alone does not make a repository unhealthy.
    pub fn is_healthy(&self) -> bool {
        self.problems.is_empty()
    }

    /// Cruft is present that a repair would clean up.
    pub fn has_cleanable_cruft(&self) -> bool {
        !self.cruft.is_empty()
    }
}

/// Verify the repository, reporting problems and any cruft found.
///
/// Read-only: takes `&Repo`, so it can run while another process works. Use
/// [`repair`] to clean up what it finds.
pub fn check(repo: &Repo) -> Result<Report> {
    inspect(repo, None)
}

/// Verify the repository and clean up the cruft it finds.
///
/// Takes `&WriteGuard` because it mutates — the split mirrors what the CLI
/// already did, which locked for `--repair` and not for a plain check.
pub fn repair(guard: &WriteGuard) -> Result<Report> {
    inspect(guard.repo(), Some(guard))
}

fn inspect(repo: &Repo, guard: Option<&WriteGuard>) -> Result<Report> {
    let repair = guard.is_some();
    let root = repo.root();
    let conn = repo.conn();
    let objects_dir = root.join(".velo/objects");

    let mut problems: Vec<Problem> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();

    // ── 1. Objects: referenced ones exist and re-hash to their own name ───────
    let referenced = referenced_objects(conn)?;
    let mut verified = 0usize;
    let before = problems.len();
    let progress = repo.phase(Phase::Verifying, Some(referenced.len() as u64));
    for hash in &referenced {
        progress.tick();
        if !objects_dir.join(hash).exists() {
            problems.push(Problem::MissingObject { hash: hash.clone() });
            continue;
        }
        match storage::read_object(&objects_dir, hash) {
            Ok(bytes) => {
                let actual = blake3::hash(&bytes).to_hex().to_string();
                if &actual == hash {
                    verified += 1;
                } else {
                    problems.push(Problem::CorruptObject {
                        hash: hash.clone(),
                        actual,
                    });
                }
            }
            Err(_) => problems.push(Problem::UndecodableObject { hash: hash.clone() }),
        }
    }
    sections.push(Section::Objects {
        referenced: referenced.len(),
        verified,
        problems: problems.len() - before,
    });

    // ── 2. Snapshots: parents resolve; content-addressed ids recompute ────────
    let all_snaps = all_snapshots(conn)?;
    let before = problems.len();
    let (ids_verified, ids_legacy) = check_snapshots(conn, &all_snaps, &mut problems)?;
    sections.push(Section::Snapshots {
        checked: all_snaps.len(),
        ids_verified,
        ids_legacy,
        problems: problems.len() - before,
    });

    // ── 3. Refs resolve: PARENT, tags, stash ─────────────────────────────────
    let before = problems.len();
    let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let position = position.trim();
    if !position.is_empty() && !all_snaps.contains(position) {
        problems.push(Problem::DanglingPosition {
            hash: position.to_string(),
        });
    }
    check_ref_table(conn, "tags", "name", &all_snaps, &mut problems)?;
    check_ref_table(conn, "stash", "name", &all_snaps, &mut problems)?;
    sections.push(Section::Refs {
        problems: problems.len() - before,
    });

    // ── 4. Cruft, and optionally its removal ─────────────────────────────────
    let found = find_cruft(conn, root);
    let repaired = match guard {
        Some(g) if !found.is_empty() => {
            repair_cruft(g.conn(), &found)?;
            found.clone()
        }
        _ => Vec::new(),
    };
    let cruft = if repaired.is_empty() {
        found
    } else {
        Vec::new() // cleaned
    };
    sections.push(Section::State {
        outstanding: cruft.len(),
        repaired: !repaired.is_empty(),
    });

    Ok(Report {
        sections,
        problems,
        cruft,
        repaired,
        repair_requested: repair,
    })
}

// ─── Object and snapshot checks ───────────────────────────────────────────────

/// Every object hash the database points at, from file maps and conflict blobs.
fn referenced_objects(conn: &rusqlite::Connection) -> Result<HashSet<String>> {
    let mut referenced: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT hash FROM file_map")?;
        let set: HashSet<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter(|h| !h.is_empty())
            .collect();
        set
    };
    // Conflict sidecars reference objects too.
    let mut stmt =
        conn.prepare("SELECT ancestor_hash, our_hash, their_hash FROM conflict_files")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows.flatten() {
        for h in [row.0, row.1, row.2] {
            if !h.is_empty() {
                referenced.insert(h);
            }
        }
    }
    Ok(referenced)
}

fn all_snapshots(conn: &rusqlite::Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT hash FROM snapshots")?;
    let set: HashSet<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(set)
}

/// Check parent links and recompute content-addressed ids.
/// Returns (ids verified, ids too old to verify).
fn check_snapshots(
    conn: &rusqlite::Connection,
    all_snaps: &HashSet<String>,
    problems: &mut Vec<Problem>,
) -> Result<(usize, usize)> {
    let mut stmt = conn.prepare(
        "SELECT hash, message, branch, parent_hash, merge_parent, created_at FROM snapshots",
    )?;
    let snaps: Vec<(String, String, String, String, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get::<_, String>(5).unwrap_or_default(),
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut verified = 0usize;
    let mut legacy = 0usize;
    for (hash, message, branch, parent, merge_parent, created_at) in &snaps {
        if !parent.is_empty() && !all_snaps.contains(parent) {
            problems.push(Problem::MissingParent {
                snapshot: hash.clone(),
                parent: parent.clone(),
            });
        }
        if !merge_parent.is_empty() && !all_snaps.contains(merge_parent) {
            problems.push(Problem::MissingMergeParent {
                snapshot: hash.clone(),
                merge_parent: merge_parent.clone(),
            });
        }

        // Only ids from the content-addressed scheme can be recomputed.
        if hash.len() == crate::commands::SNAP_HASH_LEN {
            let tree = load_tree(conn, hash)?;
            let recomputed =
                crate::commands::snapshot_id(&tree, parent, merge_parent, message, created_at);
            if &recomputed == hash {
                verified += 1;
            } else {
                problems.push(Problem::IdMismatch {
                    snapshot: hash.clone(),
                    message: message.clone(),
                    branch: branch.clone(),
                    recomputed,
                });
            }
        } else {
            legacy += 1;
        }
    }
    Ok((verified, legacy))
}

/// Load a snapshot's tree (path, object hash, mode) for id verification.
fn load_tree(conn: &rusqlite::Connection, snap: &str) -> Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let tree = stmt
        .query_map([snap], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tree)
}

/// Verify every `snapshot_hash` in `table` points at a live snapshot.
fn check_ref_table(
    conn: &rusqlite::Connection,
    table: &str,
    id_col: &str,
    snaps: &HashSet<String>,
    problems: &mut Vec<Problem>,
) -> Result<()> {
    let sql = format!("SELECT {}, snapshot_hash FROM {}", id_col, table);
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    for (name, hash) in rows {
        if !hash.is_empty() && !snaps.contains(&hash) {
            problems.push(Problem::DanglingRef {
                table: table.to_string(),
                name,
                hash,
            });
        }
    }
    Ok(())
}

// ─── Cruft ────────────────────────────────────────────────────────────────────

fn count(conn: &rusqlite::Connection, sql: &str) -> usize {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as usize
}

fn find_cruft(conn: &rusqlite::Connection, root: &Path) -> Vec<Cruft> {
    let mut found = Vec::new();

    let orphan_hunks = count(
        conn,
        "SELECT count(*) FROM hunk_decisions
         WHERE file_path NOT IN (SELECT path FROM conflict_files)",
    );
    if orphan_hunks > 0 {
        found.push(Cruft::OrphanHunkDecisions(orphan_hunks));
    }

    let orphan_tags = count(
        conn,
        "SELECT count(*) FROM trash_tags
         WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
           AND snapshot_hash NOT IN (SELECT hash FROM trash)",
    );
    if orphan_tags > 0 {
        found.push(Cruft::OrphanShelvedTags(orphan_tags));
    }

    let conflicts = count(conn, "SELECT count(*) FROM conflict_files");
    if conflicts > 0 && !root.join(".velo/MERGE_HEAD").exists() {
        found.push(Cruft::BrokenConflictState(conflicts));
    }

    let stale = count(
        conn,
        "SELECT count(*) FROM remote_refs
         WHERE hash NOT IN (SELECT hash FROM snapshots)",
    );
    if stale > 0 {
        found.push(Cruft::StaleRemoteRefs(stale));
    }

    let orphan_remotes = count(
        conn,
        "SELECT count(*) FROM remote_refs
         WHERE remote NOT IN (SELECT name FROM remotes)",
    );
    if orphan_remotes > 0 {
        found.push(Cruft::OrphanRemoteRefs(orphan_remotes));
    }

    found
}

fn repair_cruft(conn: &rusqlite::Connection, cruft: &[Cruft]) -> Result<()> {
    for item in cruft {
        match item {
            Cruft::OrphanHunkDecisions(_) => {
                conn.execute(
                    "DELETE FROM hunk_decisions
                     WHERE file_path NOT IN (SELECT path FROM conflict_files)",
                    [],
                )?;
            }
            Cruft::OrphanShelvedTags(_) => {
                conn.execute(
                    "DELETE FROM trash_tags
                     WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
                       AND snapshot_hash NOT IN (SELECT hash FROM trash)",
                    [],
                )?;
            }
            Cruft::BrokenConflictState(_) => {
                conn.execute("DELETE FROM conflict_files", [])?;
                conn.execute("DELETE FROM hunk_decisions", [])?;
            }
            Cruft::StaleRemoteRefs(_) => {
                conn.execute(
                    "DELETE FROM remote_refs WHERE hash NOT IN (SELECT hash FROM snapshots)",
                    [],
                )?;
            }
            Cruft::OrphanRemoteRefs(_) => {
                conn.execute(
                    "DELETE FROM remote_refs WHERE remote NOT IN (SELECT name FROM remotes)",
                    [],
                )?;
            }
        }
    }
    Ok(())
}
