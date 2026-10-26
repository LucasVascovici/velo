//! `velody fsck` — verify repository integrity.
//!
//! Checks the invariants the rest of Velody (and sync) relies on: every referenced
//! object exists and re-hashes to its own name; every snapshot's parents resolve;
//! content-addressed snapshot ids recompute correctly; and every ref (PARENT,
//! tags, stash) points somewhere real.
//!
//! Findings are returned as typed values rather than pre-formatted lines, so a
//! consumer can branch on them — "which objects are missing?" is a question you
//! can answer from a [`Report`] without parsing text. Rendering and the exit code
//! live in `velody-cli`.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::error::Result;
use crate::progress::Phase;
use crate::storage::Verified;
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
    /// A chunked object's manifest names a chunk that is absent.
    MissingChunk { object: String, chunk: String },
    /// A chunk file no longer hashes to its own name (or cannot be decoded,
    /// in which case `actual` says so instead of naming a hash).
    CorruptChunk { chunk: String, actual: String },
    /// A snapshot names a parent that doesn't exist.
    MissingParent { snapshot: String, parent: String },
    /// A merge commit names a second parent that doesn't exist.
    MissingMergeParent {
        snapshot: String,
        merge_parent: String,
    },
    /// A snapshot's parent was shelved by `velody undo` (it is in the trash) while
    /// this snapshot still built on it. `velody redo` on `branch` restores it.
    UndoneParent {
        snapshot: String,
        parent: String,
        branch: String,
    },
    /// As [`Problem::UndoneParent`], for a merge's second parent.
    UndoneMergeParent {
        snapshot: String,
        merge_parent: String,
        branch: String,
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
    /// A rename edge names a destination the snapshot does not contain.
    RenameToMissing {
        snapshot: String,
        from_path: String,
        to_path: String,
    },
    /// A rename edge names a source no parent of the snapshot contained.
    RenameFromMissing {
        snapshot: String,
        from_path: String,
        to_path: String,
    },
    /// A rename edge is attached to a snapshot that doesn't exist.
    RenameOnMissingSnapshot { snapshot: String },
    /// A stored signature does not verify against its snapshot id: the row was
    /// altered, or never was a signature. Rows of an unknown algorithm are not
    /// reported, since this build cannot judge them.
    BadSignature {
        snapshot: String,
        public_key: String,
    },
    /// A delta snapshot is based on a snapshot that is in neither history nor
    /// the trash, so its tree cannot be rebuilt.
    DeltaBaseMissing { snapshot: String, base: String },
    /// A delta snapshot's base is not its first parent. A delta is always
    /// against the first parent, so the stored chain does not match history.
    DeltaBaseNotFirstParent {
        snapshot: String,
        base: String,
        parent: String,
    },
    /// A delta snapshot is on a cycle of delta bases and has no keyframe.
    DeltaCycle { snapshot: String },
    /// A delta snapshot also has keyframe rows. Readers use the delta; the
    /// rows are a second, possibly different, tree.
    StorageMismatch { snapshot: String },
}

impl Problem {
    /// True for an id mismatch on a `remotes/...` snapshot: a copy a fetch
    /// brought in, which a re-fetch refreshes, not damage to the user's work.
    pub fn is_remote_tracking(&self) -> bool {
        matches!(self, Problem::IdMismatch { branch, .. } if branch.starts_with("remotes/"))
    }
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
            Problem::MissingChunk { object, chunk } => write!(
                f,
                "object {} needs chunk {} which is missing from the store",
                &object[..16.min(object.len())],
                &chunk[..16.min(chunk.len())]
            ),
            Problem::CorruptChunk { chunk, actual } => write!(
                f,
                "chunk {} is corrupt — its content hashes to {}",
                &chunk[..16.min(chunk.len())],
                &actual[..16.min(actual.len())]
            ),
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
            Problem::UndoneParent {
                snapshot,
                parent,
                branch,
            } => write!(
                f,
                "snapshot {} has parent {} which velody undo removed from branch '{}' — run velody switch {} then velody redo to restore it",
                snapshot, parent, branch, branch
            ),
            Problem::UndoneMergeParent {
                snapshot,
                merge_parent,
                branch,
            } => write!(
                f,
                "snapshot {} has merge parent {} which velody undo removed from branch '{}' — run velody switch {} then velody redo to restore it",
                snapshot, merge_parent, branch, branch
            ),
            Problem::IdMismatch {
                snapshot,
                message,
                branch,
                recomputed,
            } => write!(
                f,
                "snapshot {} (\"{}\", branch {}) does not match its content (recomputed {}){}",
                snapshot,
                message,
                branch,
                recomputed,
                if self.is_remote_tracking() {
                    " \u{2014} a remote-tracking copy from a fetch, not your own work; re-fetch to refresh"
                } else {
                    ""
                }
            ),
            Problem::DanglingPosition { hash } => {
                write!(f, "PARENT points to {} which does not exist", hash)
            }
            Problem::DanglingRef { table, name, hash } => write!(
                f,
                "{} '{}' points to snapshot {} which does not exist",
                table, name, hash
            ),
            Problem::RenameToMissing {
                snapshot,
                from_path,
                to_path,
            } => write!(
                f,
                "snapshot {} records a rename {} → {}, but does not contain {}",
                &snapshot[..16.min(snapshot.len())],
                from_path,
                to_path,
                to_path
            ),
            Problem::RenameFromMissing {
                snapshot,
                from_path,
                to_path,
            } => write!(
                f,
                "snapshot {} records a rename {} → {}, but no parent contained {}",
                &snapshot[..16.min(snapshot.len())],
                from_path,
                to_path,
                from_path
            ),
            Problem::RenameOnMissingSnapshot { snapshot } => write!(
                f,
                "a rename edge is attached to snapshot {}, which does not exist",
                &snapshot[..16.min(snapshot.len())]
            ),
            Problem::BadSignature {
                snapshot,
                public_key,
            } => write!(
                f,
                "the signature by key {} on snapshot {} does not verify",
                &public_key[..16.min(public_key.len())],
                &snapshot[..16.min(snapshot.len())]
            ),
            Problem::DeltaBaseMissing { snapshot, base } => write!(
                f,
                "snapshot {} is stored as a delta on {}, which does not exist",
                &snapshot[..16.min(snapshot.len())],
                &base[..16.min(base.len())]
            ),
            Problem::DeltaBaseNotFirstParent {
                snapshot,
                base,
                parent,
            } => write!(
                f,
                "snapshot {} is stored as a delta on {}, but its first parent is {}",
                &snapshot[..16.min(snapshot.len())],
                &base[..16.min(base.len())],
                if parent.is_empty() {
                    "absent"
                } else {
                    &parent[..16.min(parent.len())]
                }
            ),
            Problem::DeltaCycle { snapshot } => write!(
                f,
                "snapshot {} is on a cycle of delta bases and has no keyframe",
                &snapshot[..16.min(snapshot.len())]
            ),
            Problem::StorageMismatch { snapshot } => write!(
                f,
                "snapshot {} is stored both as a delta and as a keyframe",
                &snapshot[..16.min(snapshot.len())]
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
    /// Chunk files no object's manifest names. Harmless; `gc` collects them too.
    UnreferencedChunks(usize),
    /// `file_map` rows that exactly repeat another row of the same snapshot
    /// (a 5.1.1 fetch re-imported an undone snapshot). The id check ignores
    /// them; `--repair` removes the extras.
    DuplicateFileMapRows(usize),
    /// Signature rows naming a snapshot that is in neither history nor the
    /// trash. Nothing can ever show them; `--repair` removes them.
    OrphanSignatures(usize),
    /// Delta rows with no delta record, plus delta snapshots in neither
    /// history nor the trash. `--repair` removes what nothing is based on;
    /// an orphan delta that others are based on stays (the fold of a later
    /// gc settles it), so this can remain after a repair.
    OrphanDeltaRows(usize),
    /// Delta snapshots deeper than a writer would ever leave them (more than
    /// `KEYFRAME_INTERVAL - 1` links from their keyframe). Correct, only
    /// slower to read; a repair never shortens chains.
    OverlongChains(usize),
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
            Cruft::UnreferencedChunks(n) => {
                format!("{} chunk file(s) no object references", n)
            }
            Cruft::DuplicateFileMapRows(n) => format!(
                "{} duplicate tree row(s) (a fetch re-imported an undone snapshot)",
                n
            ),
            Cruft::OrphanSignatures(n) => {
                format!("{} signature(s) on snapshots that no longer exist", n)
            }
            Cruft::OrphanDeltaRows(n) => format!(
                "{} orphan delta row(s) or delta snapshot(s) (those others are based on stay after --repair)",
                n
            ),
            Cruft::OverlongChains(n) => format!(
                "{} delta snapshot(s) more than {} links from their keyframe (--repair does not shorten chains)",
                n,
                crate::tree_store::KEYFRAME_INTERVAL - 1
            ),
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
            Cruft::UnreferencedChunks(n) => format!("removed {} unreferenced chunk file(s)", n),
            Cruft::DuplicateFileMapRows(n) => format!("removed {} duplicate tree row(s)", n),
            Cruft::OrphanSignatures(n) => format!("removed {} orphaned signature(s)", n),
            Cruft::OrphanDeltaRows(n) => {
                format!(
                    "removed orphan delta storage (of {} item(s); any others are based on)",
                    n
                )
            }
            Cruft::OverlongChains(n) => format!("{} overlong delta chain(s) left as they are", n),
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
    Renames {
        checked: usize,
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
            | Section::Refs { problems }
            | Section::Renames { problems, .. } => *problems,
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
    let conn = repo.conn();
    let objects = repo.objects();

    let mut problems: Vec<Problem> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();

    // ── 1. Objects: referenced ones exist and re-hash to their own name ───────
    // Sorted, so the order problems are reported in does not depend on hash-set
    // iteration order, which varied from run to run.
    let mut referenced: Vec<String> = referenced_objects(conn)?.into_iter().collect();
    referenced.sort_unstable();
    let before = problems.len();
    let progress = repo.phase(Phase::Verifying, Some(referenced.len() as u64));
    let verified = check_objects(&objects, &referenced, &progress, &mut problems);
    sections.push(Section::Objects {
        referenced: referenced.len(),
        verified,
        problems: problems.len() - before,
    });

    // ── 2. Snapshots: parents resolve; content-addressed ids recompute ────────
    let all_snaps = all_snapshots(conn)?;
    let before = problems.len();
    let SnapshotScan {
        verified: mut ids_verified,
        legacy: ids_legacy,
        duplicate_rows,
        storage_fixes,
    } = check_snapshots(conn, &all_snaps, &mut problems)?;
    if let Some(g) = guard {
        // A snapshot stored both ways, where one way reproduces its id, is
        // settled by keeping that one; the finding goes with the fix.
        for fix in &storage_fixes {
            match fix {
                StorageFix::DropKeyframeRows(s) => {
                    crate::tree_store::drop_keyframe_rows(g.conn(), s)?
                }
                StorageFix::DropDelta(s) => crate::tree_store::drop_delta(g.conn(), s)?,
            }
        }
        problems.retain(|p| match p {
            Problem::StorageMismatch { snapshot } => !storage_fixes.iter().any(|f| match f {
                StorageFix::DropKeyframeRows(s) | StorageFix::DropDelta(s) => s == snapshot,
            }),
            // A dropped delta is what failed to reproduce the id (or could not
            // be rebuilt at all); with it gone the finding no longer holds.
            Problem::IdMismatch { snapshot, .. }
            | Problem::DeltaCycle { snapshot }
            | Problem::DeltaBaseMissing { snapshot, .. }
            | Problem::DeltaBaseNotFirstParent { snapshot, .. } => !storage_fixes
                .iter()
                .any(|f| matches!(f, StorageFix::DropDelta(s) if s == snapshot)),
            _ => true,
        });
        // Their keyframe rows were proven to reproduce the id, so they count
        // as verified, as a fresh check would.
        ids_verified += storage_fixes
            .iter()
            .filter(|f| matches!(f, StorageFix::DropDelta(_)))
            .count();
    }
    check_signatures(conn, &mut problems)?;
    sections.push(Section::Snapshots {
        checked: all_snaps.len(),
        ids_verified,
        ids_legacy,
        problems: problems.len() - before,
    });

    // ── 3. Refs resolve: PARENT, tags, stash ─────────────────────────────────
    let before = problems.len();
    let position = repo.position().map(|p| p.into_string()).unwrap_or_default();
    let position = position.as_str();
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

    // ── 4. Rename edges point at paths that are actually there ───────────────
    let before = problems.len();
    let checked = check_renames(conn, &mut problems)?;
    sections.push(Section::Renames {
        checked,
        problems: problems.len() - before,
    });

    // ── 5. Cruft, and optionally its removal ─────────────────────────────────
    let mut found = find_cruft(conn, repo, duplicate_rows);
    let orphan_chunks = unreferenced_chunks(&objects)?;
    if !orphan_chunks.is_empty() {
        found.push(Cruft::UnreferencedChunks(orphan_chunks.len()));
    }
    let orphan_deltas = crate::tree_store::orphan_delta_rows(conn)?
        + crate::tree_store::orphan_delta_snapshots(conn)?;
    if orphan_deltas > 0 {
        found.push(Cruft::OrphanDeltaRows(orphan_deltas));
    }
    let overlong =
        crate::tree_store::overlong_chains(conn, crate::tree_store::KEYFRAME_INTERVAL - 1)?;
    if overlong > 0 {
        found.push(Cruft::OverlongChains(overlong));
    }
    // Chains are never shortened by a repair, so that cruft always stays.
    let (mut persistent, fixable): (Vec<Cruft>, Vec<Cruft>) = found
        .iter()
        .cloned()
        .partition(|c| matches!(c, Cruft::OverlongChains(_)));
    let repaired = match guard {
        Some(g) if !fixable.is_empty() => {
            repair_cruft(g.conn(), &fixable)?;
            for chunk in &orphan_chunks {
                objects.remove_chunk(chunk)?;
            }
            fixable.clone()
        }
        _ => Vec::new(),
    };
    let cruft = if guard.is_none() {
        found
    } else {
        // Orphan deltas something is based on survive the repair.
        let left = crate::tree_store::orphan_delta_snapshots(conn)?;
        if left > 0 {
            persistent.push(Cruft::OrphanDeltaRows(left));
        }
        persistent
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

/// Check every referenced object, and the chunks its manifest names.
/// Returns how many objects verified.
///
/// The hashing is the cost, so it runs on the pool: manifests, then each
/// distinct chunk once, then every object. The findings are then replayed
/// sequentially in `referenced` order, which keeps the report identical to a
/// single-threaded walk: a shared chunk is named once when corrupt and once per
/// object when missing, and an object with a damaged chunk is not also
/// reported as unreadable. Objects behind a damaged chunk are verified anyway
/// and their result discarded; that only costs time on a repository that is
/// already reporting damage.
fn check_objects(
    objects: &crate::storage::ObjectStore,
    referenced: &[String],
    progress: &crate::progress::PhaseGuard<'_>,
    problems: &mut Vec<Problem>,
) -> usize {
    let manifests = objects.par_chunks_of(referenced);
    let mut chunk_index: HashMap<&str, usize> = HashMap::new();
    let mut chunk_names: Vec<String> = Vec::new();
    for chunk in manifests.iter().flatten().flatten() {
        if !chunk_index.contains_key(chunk.as_str()) {
            chunk_index.insert(chunk.as_str(), chunk_names.len());
            chunk_names.push(chunk.clone());
        }
    }
    let chunk_state = objects.par_verify_chunks(&chunk_names);
    let object_state = objects.par_verify(referenced);

    let mut verified = 0usize;
    let mut chunk_seen = vec![false; chunk_names.len()];
    for (i, hash) in referenced.iter().enumerate() {
        progress.tick();
        let before_object = problems.len();
        // A manifest's chunks are named precisely rather than surfacing as a
        // vague reassembly failure.
        if let Some(chunks) = &manifests[i] {
            for chunk in chunks {
                let at = chunk_index[chunk.as_str()];
                let first = !std::mem::replace(&mut chunk_seen[at], true);
                match &chunk_state[at] {
                    Verified::Ok => {}
                    // Reported once per object that needs it.
                    Verified::Missing => problems.push(Problem::MissingChunk {
                        object: hash.clone(),
                        chunk: chunk.clone(),
                    }),
                    // Corrupt: reported once, however many objects share it.
                    Verified::Undecodable if first => problems.push(Problem::CorruptChunk {
                        chunk: chunk.clone(),
                        actual: "undecodable".into(),
                    }),
                    Verified::Mismatch { actual } if first => {
                        problems.push(Problem::CorruptChunk {
                            chunk: chunk.clone(),
                            actual: actual.clone(),
                        })
                    }
                    Verified::Undecodable | Verified::Mismatch { .. } => {}
                }
            }
            if problems.len() > before_object {
                continue; // reassembly would only repeat the chunk findings
            }
        }
        match &object_state[i] {
            Verified::Ok => verified += 1,
            Verified::Missing => problems.push(Problem::MissingObject { hash: hash.clone() }),
            Verified::Undecodable => {
                problems.push(Problem::UndecodableObject { hash: hash.clone() })
            }
            Verified::Mismatch { actual } => problems.push(Problem::CorruptObject {
                hash: hash.clone(),
                actual: actual.clone(),
            }),
        }
    }
    verified
}

// ─── Object and snapshot checks ───────────────────────────────────────────────

/// Every object hash the database points at, from file maps and conflict blobs.
fn referenced_objects(conn: &rusqlite::Connection) -> Result<HashSet<String>> {
    let mut referenced: HashSet<String> = crate::tree_store::referenced_objects(conn)?;
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

/// How many snapshots' trees go to the pool at once. Bounds memory: only this
/// many are held while the ordered scan of `file_map` carries on.
const ID_BATCH: usize = 64;

/// One snapshot's tree, read from the ordered scan of `file_map`.
struct TreeRows {
    /// Index into the snapshot list when its id can be recomputed.
    wanted: Option<usize>,
    rows: Vec<TreeRow>,
}

/// `(path, object hash, mode)`.
type TreeRow = (String, String, i64);

type SnapRow = (String, String, String, String, String, i64);
/// Per tree: exact-duplicate rows dropped, and (snapshot index, recomputed id)
/// when the recomputed id differs from the stored one.
type TreeVerdict = (usize, Option<(usize, String)>);

/// Check parent links and recompute content-addressed ids.
/// Returns (ids verified, ids too old to verify, exact-duplicate `file_map` rows).
///
/// Every `file_map` row is read once, in `snapshot_hash` order, rather than
/// once per snapshot; each snapshot's tree is then de-duplicated and hashed on
/// the pool. Problems are still emitted in `snapshots` table order, so the
/// report does not depend on which thread finished first.
fn check_snapshots(
    conn: &rusqlite::Connection,
    all_snaps: &HashSet<String>,
    problems: &mut Vec<Problem>,
) -> Result<SnapshotScan> {
    use rayon::prelude::*;

    let mut stmt = conn.prepare(
        "SELECT hash, message, branch, parent_hash, merge_parent, created_at_ms FROM snapshots",
    )?;
    let snaps: Vec<SnapRow> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get::<_, i64>(5).unwrap_or_default(),
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Snapshots shelved by `velody undo`. A live snapshot that still builds on
    // one is recoverable with `velody redo`, which is a better thing to tell the
    // user than "does not exist".
    let mut stmt = conn.prepare("SELECT hash, branch FROM trash")?;
    let trash: std::collections::HashMap<String, String> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Metadata is part of the identity. It is small, so it is read whole.
    let mut metas: HashMap<String, crate::meta::SnapshotMeta> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT snapshot_id, namespace, key, value FROM snapshot_meta
         ORDER BY snapshot_id, namespace, key",
    )?;
    let meta_rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in meta_rows {
        let (snapshot, namespace, key, value) = row?;
        metas
            .entry(snapshot)
            .or_default()
            .insert_stored(namespace, key, value);
    }
    drop(stmt);
    let no_meta = crate::meta::SnapshotMeta::new();

    // Only ids from the content-addressed scheme can be recomputed.
    let wanted: HashMap<&str, usize> = snaps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.0.len() == crate::commands::SNAP_ID_LEN)
        .map(|(i, s)| (s.0.as_str(), i))
        .collect();

    // Exact repeats are dropped before hashing (a 5.1.1 fetch could import an
    // undone snapshot twice). Returns, per tree, the repeats removed and, for
    // a snapshot whose id can be checked, the id it recomputes to when wrong.
    let process = |batch: &mut Vec<TreeRows>| -> Vec<TreeVerdict> {
        let verdicts = batch
            .par_iter_mut()
            .map(|t| {
                let total = t.rows.len();
                t.rows.sort_unstable();
                t.rows.dedup();
                let repeats = total - t.rows.len();
                let verdict = t.wanted.and_then(|i| {
                    let (hash, message, _, parent, merge_parent, created_at_ms) = &snaps[i];
                    let recomputed =
                        crate::commands::snapshot_id(crate::commands::SnapshotIdentity {
                            tree: &t.rows,
                            parent,
                            merge_parent,
                            message,
                            timestamp_ms: *created_at_ms,
                            meta: metas.get(hash).unwrap_or(&no_meta),
                        });
                    (&recomputed != hash).then_some((i, recomputed))
                });
                (repeats, verdict)
            })
            .collect();
        batch.clear();
        verdicts
    };

    // Per snapshot index: the recomputed id when it differs from the stored one.
    let mut mismatch: HashMap<usize, String> = HashMap::new();
    let mut duplicate_rows = 0usize;
    let mut absorb = |verdicts: Vec<TreeVerdict>| {
        for (repeats, verdict) in verdicts {
            duplicate_rows += repeats;
            if let Some((i, recomputed)) = verdict {
                mismatch.insert(i, recomputed);
            }
        }
    };

    let mut seen: HashSet<usize> = HashSet::new();
    let mut batch: Vec<TreeRows> = Vec::with_capacity(ID_BATCH);
    // Delta trees come out of the same ordered scan, rebuilt from their
    // keyframe; the ones it could not reach (a cycle, a missing base, or
    // below either) have no tree to hash and are reported by `check_deltas`.
    let unreached: HashSet<String> =
        crate::tree_store::for_each_tree_in_order(conn, |done, rows| {
            let want = wanted.get(done.as_str()).copied();
            seen.extend(want);
            batch.push(TreeRows { wanted: want, rows });
            if batch.len() >= ID_BATCH {
                absorb(process(&mut batch));
            }
            Ok(())
        })?
        .into_iter()
        .collect();
    absorb(process(&mut batch));

    // A snapshot with no rows at all still has an id to check: the empty tree's.
    let mut empty: Vec<TreeRows> = wanted
        .iter()
        .filter(|(h, i)| !seen.contains(i) && !unreached.contains(**h))
        .map(|(_, i)| i)
        .map(|&i| TreeRows {
            wanted: Some(i),
            rows: Vec::new(),
        })
        .collect();
    absorb(process(&mut empty));

    // Delta storage: a snapshot stored both ways is settled by whichever
    // representation reproduces its id (never by guessing).
    let mut storage_problems: Vec<Problem> = Vec::new();
    let mut storage_fixes: Vec<StorageFix> = Vec::new();
    for snap in crate::tree_store::delta_snapshots_with_keyframe_rows(conn)? {
        storage_problems.push(Problem::StorageMismatch {
            snapshot: snap.clone(),
        });
        let Some(&i) = wanted.get(snap.as_str()) else {
            continue;
        };
        if !mismatch.contains_key(&i) && !unreached.contains(&snap) {
            storage_fixes.push(StorageFix::DropKeyframeRows(snap));
            continue;
        }
        if crate::tree_store::has_dependants(conn, &snap)? {
            continue;
        }
        let kf = crate::tree_store::keyframe_rows(conn, &snap)?;
        let tree: Vec<TreeRow> = kf.into_iter().map(|(p, (h, m))| (p, h, m)).collect();
        let (hash, message, _, parent, merge_parent, created_at_ms) = &snaps[i];
        let id = crate::commands::snapshot_id(crate::commands::SnapshotIdentity {
            tree: &tree,
            parent,
            merge_parent,
            message,
            timestamp_ms: *created_at_ms,
            meta: metas.get(hash).unwrap_or(&no_meta),
        });
        if &id == hash {
            storage_fixes.push(StorageFix::DropDelta(snap));
        }
    }

    let mut verified = 0usize;
    let mut legacy = 0usize;
    for (i, (hash, message, branch, parent, merge_parent, _)) in snaps.iter().enumerate() {
        if !parent.is_empty() && !all_snaps.contains(parent) {
            problems.push(match trash.get(parent) {
                Some(b) => Problem::UndoneParent {
                    snapshot: hash.clone(),
                    parent: parent.clone(),
                    branch: b.trim().to_string(),
                },
                None => Problem::MissingParent {
                    snapshot: hash.clone(),
                    parent: parent.clone(),
                },
            });
        }
        if !merge_parent.is_empty() && !all_snaps.contains(merge_parent) {
            problems.push(match trash.get(merge_parent) {
                Some(b) => Problem::UndoneMergeParent {
                    snapshot: hash.clone(),
                    merge_parent: merge_parent.clone(),
                    branch: b.trim().to_string(),
                },
                None => Problem::MissingMergeParent {
                    snapshot: hash.clone(),
                    merge_parent: merge_parent.clone(),
                },
            });
        }

        if unreached.contains(hash) {
            // No tree could be built; the delta checks below say why.
        } else if wanted.contains_key(hash.as_str()) {
            match mismatch.remove(&i) {
                None => verified += 1,
                Some(recomputed) => problems.push(Problem::IdMismatch {
                    snapshot: hash.clone(),
                    message: message.clone(),
                    branch: branch.clone(),
                    recomputed,
                }),
            }
        } else {
            legacy += 1;
        }
    }
    check_deltas(conn, all_snaps, &trash, &snaps, problems)?;
    problems.extend(storage_problems);
    Ok(SnapshotScan {
        verified,
        legacy,
        duplicate_rows,
        storage_fixes,
    })
}

/// What [`check_snapshots`] found out beyond the problems it pushed.
struct SnapshotScan {
    /// Ids that recomputed correctly.
    verified: usize,
    /// Ids too old to verify.
    legacy: usize,
    /// Exact-duplicate `file_map` rows.
    duplicate_rows: usize,
    /// Settlements `--repair` may apply to snapshots stored both ways.
    storage_fixes: Vec<StorageFix>,
}

/// How `--repair` settles a snapshot stored as a delta and a keyframe.
enum StorageFix {
    /// The delta reproduces the id: drop the stray keyframe rows.
    DropKeyframeRows(String),
    /// Only the keyframe rows do, and nothing is based on the delta.
    DropDelta(String),
}

/// The shape of the delta chains: every base exists, is the first parent, and
/// the chain reaches a keyframe. Sorted by snapshot, so the report is stable.
fn check_deltas(
    conn: &rusqlite::Connection,
    all_snaps: &HashSet<String>,
    trash: &HashMap<String, String>,
    snaps: &[SnapRow],
    problems: &mut Vec<Problem>,
) -> Result<()> {
    let links = crate::tree_store::delta_links(conn)?;
    if links.is_empty() {
        return Ok(());
    }
    // First parents of live and trashed snapshots.
    let mut first_parents: HashMap<&str, &str> =
        snaps.iter().map(|s| (s.0.as_str(), s.3.as_str())).collect();
    let mut stmt = conn.prepare("SELECT hash, parent_hash FROM trash")?;
    let trashed: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    for (h, p) in &trashed {
        first_parents.insert(h.as_str(), p.as_str());
    }
    let base_of: HashMap<&str, &str> = links
        .iter()
        .map(|(s, b)| (s.as_str(), b.as_str()))
        .collect();
    // Delta snapshots some live or trashed snapshot needs, found by walking
    // the bases up from each of them. A link outside this set that is itself
    // in neither table is an unreachable chain: cruft, not corruption.
    let mut needed: HashSet<&str> = HashSet::new();
    for start in first_parents.keys() {
        let mut cur = *start;
        for _ in 0..=crate::tree_store::MAX_CHAIN_LINKS {
            if !needed.insert(cur) {
                break;
            }
            match base_of.get(cur) {
                Some(b) => cur = b,
                None => break,
            }
        }
    }
    for (snap, base) in &links {
        let unreachable = !needed.contains(snap.as_str())
            && !all_snaps.contains(snap)
            && !trash.contains_key(snap);
        if !unreachable && !all_snaps.contains(base) && !trash.contains_key(base) {
            problems.push(Problem::DeltaBaseMissing {
                snapshot: snap.clone(),
                base: base.clone(),
            });
        }
        if let Some(parent) = first_parents.get(snap.as_str()) {
            if crate::tree_store::first_parent(parent) != Some(base.as_str()) {
                problems.push(Problem::DeltaBaseNotFirstParent {
                    snapshot: snap.clone(),
                    base: base.clone(),
                    parent: parent.to_string(),
                });
            }
        }
        // On a cycle when walking the bases comes back to this snapshot.
        let mut cur = base.as_str();
        for _ in 0..crate::tree_store::MAX_CHAIN_LINKS {
            if cur == snap {
                problems.push(Problem::DeltaCycle {
                    snapshot: snap.clone(),
                });
                break;
            }
            match base_of.get(cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
    }
    Ok(())
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

/// Check every rename edge against the trees it claims to describe.
///
/// A rename is not part of a snapshot's identity, so nothing about the id says
/// whether an edge is true — which is exactly why it is checked here. `to` must
/// be in the snapshot and `from` in one of its parents; an edge failing either
/// makes `blame` follow a file that was never there.
///
/// `save_tree` rejects a bad `to` outright. `from` is checked only here, because
/// a caller may legitimately record a move onto a parent it never read, and
/// finding out at save time would mean reading it.
fn check_renames(conn: &rusqlite::Connection, problems: &mut Vec<Problem>) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT snapshot_hash, from_path, to_path FROM renames")?;
    let edges: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();

    for (snapshot, from_path, to_path) in &edges {
        let parents: Option<(String, String)> = conn
            .query_row(
                "SELECT parent_hash, merge_parent FROM snapshots WHERE hash = ?",
                [snapshot],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let Some((parent, merge_parent)) = parents else {
            problems.push(Problem::RenameOnMissingSnapshot {
                snapshot: snapshot.clone(),
            });
            continue;
        };

        if !tracks(conn, snapshot, to_path) {
            problems.push(Problem::RenameToMissing {
                snapshot: snapshot.clone(),
                from_path: from_path.clone(),
                to_path: to_path.clone(),
            });
        }
        // Either parent will do: a merge can absorb a branch that did the move.
        let in_a_parent = [parent, merge_parent]
            .iter()
            .filter(|p| !p.is_empty())
            .any(|p| tracks(conn, p, from_path));
        if !in_a_parent {
            problems.push(Problem::RenameFromMissing {
                snapshot: snapshot.clone(),
                from_path: from_path.clone(),
                to_path: to_path.clone(),
            });
        }
    }
    Ok(edges.len())
}

fn tracks(conn: &rusqlite::Connection, snapshot: &str, path: &str) -> bool {
    crate::tree_store::file_at(conn, snapshot, path)
        .map(|e| e.is_some())
        .unwrap_or(false)
}

/// Every stored signature must verify against the snapshot id it names. A bad
/// one is corruption; a row of an unknown algorithm is not reported.
fn check_signatures(conn: &rusqlite::Connection, problems: &mut Vec<Problem>) -> Result<()> {
    use crate::sign::{check_row, RowCheck};
    if !crate::sign::table_exists(conn)? {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT snapshot_hash, algorithm, public_key, signature FROM signatures
         ORDER BY snapshot_hash, public_key",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (snapshot, algorithm, public_key, signature) = row?;
        if check_row(&snapshot, &algorithm, &public_key, &signature) == RowCheck::Invalid {
            problems.push(Problem::BadSignature {
                snapshot,
                public_key,
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

fn find_cruft(conn: &rusqlite::Connection, repo: &Repo, duplicate_rows: usize) -> Vec<Cruft> {
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
    if conflicts > 0 && repo.has_working_tree() && !repo.root().join(".velo/MERGE_HEAD").exists() {
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

    // Counted while the snapshot stage had every row in hand: asking SQLite for
    // `COUNT(*) - COUNT(DISTINCT ...)` re-sorted all of `file_map` a second time.
    if duplicate_rows > 0 {
        found.push(Cruft::DuplicateFileMapRows(duplicate_rows));
    }

    // `count` reads a missing table as zero, which is right for a repository
    // that predates `signatures`.
    let orphan_sigs = count(
        conn,
        "SELECT count(*) FROM signatures
         WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
           AND snapshot_hash NOT IN (SELECT hash FROM trash)",
    );
    if orphan_sigs > 0 {
        found.push(Cruft::OrphanSignatures(orphan_sigs));
    }

    found
}

/// Chunk files that no object on disk names. Read from the objects directory
/// rather than the database so it agrees with what `gc` keeps. If a manifest
/// cannot be parsed its chunks are unknowable, so nothing is called cruft.
fn unreferenced_chunks(objects: &crate::storage::ObjectStore) -> Result<Vec<String>> {
    let on_disk = objects.list_chunks()?;
    if on_disk.is_empty() {
        return Ok(Vec::new());
    }
    let mut needed: HashSet<String> = HashSet::new();
    for (name, _) in objects.list()? {
        match objects.chunks_of(&name) {
            Ok(Some(chunks)) => needed.extend(chunks),
            Ok(None) => {}
            Err(_) => return Ok(Vec::new()),
        }
    }
    Ok(on_disk
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| !needed.contains(name))
        .collect())
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
            Cruft::DuplicateFileMapRows(_) => {
                crate::tree_store::remove_duplicate_rows(conn)?;
            }
            Cruft::OrphanSignatures(_) => {
                conn.execute(
                    "DELETE FROM signatures
                     WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
                       AND snapshot_hash NOT IN (SELECT hash FROM trash)",
                    [],
                )?;
            }
            // Files, not rows: removed by the caller, which holds the list.
            Cruft::OrphanDeltaRows(_) => {
                crate::tree_store::delete_orphan_delta_trees(conn)?;
            }
            Cruft::OverlongChains(_) => {}
            Cruft::UnreferencedChunks(_) => {}
        }
    }
    Ok(())
}
