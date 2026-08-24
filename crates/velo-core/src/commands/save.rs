use std::collections::HashSet;
use std::fs;

use rayon::prelude::*;
use rusqlite::params;

use crate::commands::FileStatus;
use crate::commands::SnapshotIdentity;
use crate::error::{Result, VeloError};
use crate::progress::Phase;
use crate::storage;
use std::path::Path;

use crate::progress::{Cancel, Observer};
use crate::{Author, SnapshotId, SnapshotMeta, WriteGuard};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveResult {
    pub hash: SnapshotId,
    pub new_count: usize,
    pub modified_count: usize,
    pub deleted_count: usize,
}

/// Why a save produced no snapshot, or the snapshot it produced.
///
/// The two no-op cases used to collapse into `Ok(None)`, so the only way to tell
/// them apart was the message `save` printed itself. Callers need the
/// distinction: `cherry-pick` reports "already up to date", while a rejected
/// amend means something different.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A snapshot was recorded.
    Saved(SaveResult),
    /// Nothing was unsaved, so there was nothing to snapshot.
    NothingToSave,
    /// An amend with neither new content nor a new message would only churn the
    /// snapshot's hash.
    NothingToAmend,
}

impl Outcome {
    /// The snapshot, when one was recorded.
    pub fn result(&self) -> Option<&SaveResult> {
        match self {
            Outcome::Saved(r) => Some(r),
            _ => None,
        }
    }

    /// The snapshot, consuming the outcome.
    pub fn into_result(self) -> Option<SaveResult> {
        match self {
            Outcome::Saved(r) => Some(r),
            _ => None,
        }
    }

    /// The recorded snapshot's hash, when there is one.
    pub fn hash(&self) -> Option<&str> {
        self.result().map(|r| r.hash.as_str())
    }

    pub fn saved(&self) -> bool {
        matches!(self, Outcome::Saved(_))
    }
}

/// How to save.
#[derive(Default)]
pub struct Options<'a> {
    /// Replace the most recent snapshot on this branch instead of adding one.
    pub amend: bool,
    /// Snapshot only files under these paths; empty means everything dirty.
    /// Files outside the pathspec stay unsaved.
    pub paths: &'a [&'a Path],
    /// Who is saving. Recorded in the reserved metadata namespace, so it is part
    /// of the snapshot's identity — see [`Author`].
    pub author: Option<&'a Author>,
    /// Where to report hashing progress, overriding the repository's observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked while hashing. A cancelled save records nothing.
    pub cancel: Option<&'a Cancel>,
}

/// Snapshot the working directory.
///
/// `message` may be `None` only when amending, in which case the amended
/// snapshot keeps its existing message — so fixing a forgotten file doesn't
/// force you to retype it.
pub fn run(guard: &WriteGuard, message: Option<&str>, options: Options<'_>) -> Result<Outcome> {
    let Options {
        amend,
        paths,
        author,
        observer,
        cancel,
    } = options;
    let paths: Vec<String> = paths
        .iter()
        .map(|p| crate::db::normalise(&p.to_string_lossy()))
        .collect();
    let paths = paths.as_slice();
    let root = guard.root();
    let provided = message.map(str::trim).filter(|m| !m.is_empty());
    if provided.is_none() && !amend {
        return Err(VeloError::invalid(
            "Snapshot message cannot be empty. Use: velo save \"<description>\"",
        ));
    }

    let full_dirty = crate::commands::get_dirty_files(guard.repo());
    // Apply pathspec filter: if paths given, only snapshot matching files
    let dirty: std::collections::HashMap<String, FileStatus> = if paths.is_empty() {
        full_dirty
    } else {
        full_dirty
            .into_iter()
            .filter(|(p, _)| paths.iter().any(|spec| p.starts_with(spec.as_str())))
            .collect()
    };
    // A pending merge is recorded even when the tree ends up unchanged.
    //
    // Resolving every conflict in favour of our own side is a normal outcome, and
    // it leaves the working tree identical to what we already had — so the dirty
    // set is empty. Returning `NothingToSave` there refused to record the merge
    // *and* left `MERGE_HEAD` in place, wedging the repository: every later merge,
    // rebase, undo, redo and cherry-pick then refused with "a merge is already in
    // progress", and the only escape was `merge --abort`, which throws the merge
    // away. The merge is real information regardless of the tree — it is the
    // second parent — so it gets a snapshot.
    let merge_pending = !amend && root.join(".velo/MERGE_HEAD").exists();
    if dirty.is_empty() && !amend && !merge_pending {
        return Ok(Outcome::NothingToSave);
    }

    let conn = guard.conn();
    let branch = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    // ── Amend: find the snapshot to replace ──────────────────────────────────
    // Its message is captured too, so `velo save --amend` with no message can
    // reuse it.
    let amend_target: Option<(String, String, String)> = if amend {
        conn.query_row(
            "SELECT hash, parent_hash, message FROM snapshots
             WHERE branch = ? ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    } else {
        None
    };
    let amend_hash: Option<(String, String)> = amend_target
        .as_ref()
        .map(|(h, p, _)| (h.clone(), p.clone()));

    // ── Resolve the message ───────────────────────────────────────────────────
    let message: String = match provided {
        Some(m) => m.to_string(),
        None => match &amend_target {
            Some((_, _, previous)) => previous.clone(),
            None => {
                return Err(VeloError::invalid(format!(
                    "Nothing to amend on branch '{}' — it has no snapshots yet.",
                    branch.trim()
                )))
            }
        },
    };
    let message = message.as_str();

    // Amending with neither new content nor a new message would only churn the
    // snapshot's hash, so say so instead.
    if amend && dirty.is_empty() && provided.is_none() {
        return Ok(Outcome::NothingToAmend);
    }

    // The effective parent for the new snapshot: amend keeps the original
    // snapshot's parent so history stays linear.
    let effective_parent = match &amend_hash {
        Some((_, orig_parent)) => orig_parent.trim().to_string(),
        None => parent_hash.trim().to_string(),
    };

    // ── Detect merge parent (second parent for merge commits) ────────────────
    // MERGE_HEAD stores "pre_merge_hash:source_branch". The source branch tip
    // at the time of the merge is the second parent of this snapshot.
    let merge_parent: String = if merge_pending {
        let info = fs::read_to_string(root.join(".velo/MERGE_HEAD")).unwrap_or_default();
        let source_branch = info.trim().split_once(':').map(|(_, b)| b).unwrap_or("");
        // Resolve the source to a snapshot: exact branch tip first, then a
        // remote ref like `origin/main`, so merges of either still record
        // their second parent (and a short branch name isn't read as a hash).
        crate::commands::branch_tip(conn, source_branch)
            .or_else(|| {
                crate::commands::resolve_snapshot_id(guard.repo(), source_branch)
                    .ok()
                    .map(SnapshotId::into_string)
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    // ── Count changes ─────────────────────────────────────────────────────────
    let new_count = dirty.values().filter(|s| **s == FileStatus::New).count();
    let modified_count = dirty
        .values()
        .filter(|s| **s == FileStatus::Modified)
        .count();
    let deleted_count = dirty
        .values()
        .filter(|s| **s == FileStatus::Deleted)
        .count();

    // ── Parallel hash + compress ───────────────────────────────────────────────
    let objects_dir = root.join(".velo/objects");
    let files_to_hash: Vec<String> = dirty
        .iter()
        .filter(|(_, s)| **s != FileStatus::Deleted)
        .map(|(p, _)| p.clone())
        .collect();

    // Hash each changed file, capturing its mode. Symlinks store their target
    // string (not the pointed-at content); regular files store content.
    let progress = crate::progress::PhaseGuard::new(
        observer.unwrap_or_else(|| guard.repo().observer()),
        Phase::Hashing,
        Some(files_to_hash.len() as u64),
    );
    let hash_results: Result<Vec<(String, String, i64)>> = files_to_hash
        .into_par_iter()
        .inspect(|_| progress.tick())
        .map(|rel| {
            // Checked per file. Hashing writes objects, which is harmless to
            // abandon — an object nothing references is what `gc` collects — so
            // stopping here leaves no snapshot and no dangling reference.
            crate::progress::Cancel::check(cancel)?;
            let full = root.join(&rel);
            let mode = storage::capture_mode(&full);
            let hash = if mode == storage::MODE_SYMLINK {
                storage::store_raw(&objects_dir, &storage::read_symlink_target(&full)?)?
            } else {
                storage::hash_and_compress(&full, &objects_dir)?
            };
            Ok((rel, hash, mode))
        })
        .collect();
    // `mut` is only needed by the non-Unix sticky-exec-bit pass below; on Unix
    // nothing mutates this, so silence the lint there rather than diverge the
    // two platforms' code paths.
    #[cfg_attr(unix, allow(unused_mut))]
    let mut hashed_files = hash_results?;
    // Belt and braces: an empty dirty set means the loop above never ran, so a
    // cancellation set before the save started would otherwise slip through.
    crate::progress::Cancel::check(cancel)?;

    // ── Assemble the complete tree for this snapshot ──────────────────────────
    // Carry forward every unchanged file from the *dirty base* (the snapshot the
    // working tree was diffed against, i.e. PARENT — which for an amend is the
    // snapshot being replaced, NOT its parent), then add the freshly hashed
    // files. Carrying from PARENT rather than the effective parent is what keeps
    // an amend from dropping files the replaced snapshot introduced.
    let modified_paths: HashSet<&str> = hashed_files.iter().map(|(p, _, _)| p.as_str()).collect();
    let mut tree: Vec<(String, String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
        let carried: Vec<(String, String, i64)> = stmt
            .query_map([parent_hash.trim()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter(|(p, _, _)| {
                !modified_paths.contains(p.as_str())
                    && dirty.get(p.as_str()) != Some(&FileStatus::Deleted)
            })
            .collect();
        carried
    };

    // On platforms that can't observe the executable bit (Windows), keep it
    // "sticky": a regular file that was executable in the parent stays
    // executable across content edits, since the filesystem can't tell us.
    #[cfg(not(unix))]
    {
        use std::collections::HashMap;
        let parent_modes: HashMap<&str, i64> =
            tree.iter().map(|(p, _, m)| (p.as_str(), *m)).collect();
        for (rel, _h, mode) in hashed_files.iter_mut() {
            if *mode == storage::MODE_REGULAR {
                if let Some(&storage::MODE_EXEC) = parent_modes.get(rel.as_str()) {
                    *mode = storage::MODE_EXEC;
                }
            }
        }
    }

    tree.extend(hashed_files.iter().cloned());

    // ── Content-addressed snapshot id ─────────────────────────────────────────
    // Authorship is the only metadata `velo save` records. A consumer that
    // wants more builds the snapshot through `WriteGuard::save_tree`.
    let mut snapshot_meta = SnapshotMeta::new();
    if let Some(author) = author {
        snapshot_meta.set_author(author);
    }

    let timestamp_ms = crate::commands::snapshot_timestamp_ms();
    let snapshot_hash = crate::commands::snapshot_id(SnapshotIdentity {
        tree: &tree,
        parent: effective_parent.trim(),
        merge_parent: merge_parent.as_str(),
        message,
        timestamp_ms,
        meta: &snapshot_meta,
    });
    let snapshot_hash = snapshot_hash.as_str();

    // ── DB transaction ────────────────────────────────────────────────────────
    let tx = guard.transaction()?;

    // If amending, delete the old snapshot and its file_map first
    if let Some((old_hash, _)) = &amend_hash {
        tx.execute("DELETE FROM file_map WHERE snapshot_hash = ?", [old_hash])?;
        tx.execute("DELETE FROM snapshots   WHERE hash = ?", [old_hash])?;
        // Also remove from trash (shouldn't be there, but be safe)
        tx.execute("DELETE FROM trash WHERE hash = ?", [old_hash])?;
    }

    tx.execute(
        "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![
            snapshot_hash,
            message,
            branch.trim(),
            effective_parent.as_str(),
            merge_parent.as_str(),
            timestamp_ms
        ],
    )?;

    {
        let mut ins = tx.prepare(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
        )?;
        for (p, h, m) in &tree {
            ins.execute(params![snapshot_hash, p, h, m])?;
        }
    }
    {
        // The id above commits to this metadata, so it has to be stored in the
        // same transaction — an id committing to rows the repository does not
        // hold fails its own `fsck`. Empty until an author is supplied, which is
        // why `save` had no reason to write here before.
        let mut ins_meta = tx.prepare(
            "INSERT INTO snapshot_meta (snapshot_id, namespace, key, value)
             VALUES (?, ?, ?, ?)",
        )?;
        for (namespace, key, value) in snapshot_meta.iter() {
            ins_meta.execute(params![snapshot_hash, namespace, key, value])?;
        }
    }

    // Moves made since the last save become edges on this one, in the same
    // transaction as the tree they describe.
    crate::commands::mv::apply_pending(&tx, snapshot_hash, &tree)?;

    // New save invalidates the redo stack for this branch. Drop the shelved
    // tags belonging to those discarded snapshots too, so they don't linger.
    tx.execute(
        "DELETE FROM trash_tags WHERE snapshot_hash IN
             (SELECT hash FROM trash WHERE branch = ?)",
        [branch.trim()],
    )?;
    tx.execute("DELETE FROM trash WHERE branch = ?", [branch.trim()])?;
    tx.commit()?;

    storage::write_atomic(&root.join(".velo/PARENT"), snapshot_hash.as_bytes())?;

    // If a merge was in progress, this save finalises it — clear the merge state
    let merge_head = root.join(".velo/MERGE_HEAD");
    if merge_head.exists() {
        let _ = fs::remove_file(&merge_head);
    }

    Ok(Outcome::Saved(SaveResult {
        hash: SnapshotId::from_stored(snapshot_hash),
        new_count,
        modified_count,
        deleted_count,
    }))
}
