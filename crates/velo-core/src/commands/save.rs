use std::collections::HashSet;
use std::fs;

use rusqlite::params;

use crate::commands::require_working_tree;
use crate::commands::FileStatus;
use crate::commands::SnapshotIdentity;
use crate::error::{InProgress, Result, VeloError};
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
    /// App-namespaced metadata to attach, hashed into the snapshot's id exactly
    /// as [`WriteGuard::save_tree`] does. Empty by default.
    ///
    /// The author is applied on top, so it always wins the reserved keys. When
    /// amending, the new snapshot carries this plus the author, as a fresh save
    /// would: the replaced snapshot's metadata is **not** carried forward.
    pub meta: SnapshotMeta,
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
    crate::timing::begin();
    require_working_tree(guard.repo(), "save")?;
    let Options {
        amend,
        paths,
        author,
        meta,
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

    // Amending mid-merge used to skip `merge_pending` yet still delete
    // `MERGE_HEAD` at the end, silently discarding the merge. Refuse instead.
    if amend && root.join(".velo/MERGE_HEAD").exists() {
        return Err(VeloError::OperationInProgress {
            what: InProgress::Merge,
        });
    }
    // Saving over unresolved conflicts records "ours" for each file, drops
    // theirs, and leaves the rows behind so `status` reports a merge that no
    // longer exists. Make the caller resolve first.
    let unresolved = crate::commands::get_conflict_files(guard.repo());
    if !unresolved.is_empty() {
        return Err(VeloError::Conflicts {
            paths: unresolved
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
        });
    }

    let scan = crate::commands::dirty_scan(guard.repo());
    crate::timing::lap("save.scan");
    let full_dirty = scan.status;
    let known_hashes = scan.hashes;
    let unhashed = scan.unhashed;
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
    let amend_target: Option<(String, String, String, String)> = if amend {
        conn.query_row(
            "SELECT hash, parent_hash, message, merge_parent FROM snapshots
             WHERE branch = ? ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
            [branch.trim()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok()
    } else {
        None
    };
    let amend_hash: Option<(String, String)> = amend_target
        .as_ref()
        .map(|(h, p, _, _)| (h.clone(), p.clone()));

    // An amend replaces the tip under a new id, which a signature cannot survive.
    if let Some((old, _)) = &amend_hash {
        if crate::commands::compact::signed(guard.repo())?.contains(old) {
            return Err(VeloError::invalid(format!(
                "Cannot amend: snapshot {} is signed; signatures cannot survive a rewrite.",
                &old[..16.min(old.len())]
            )));
        }
    }

    // ── Resolve the message ───────────────────────────────────────────────────
    let message: String = match provided {
        Some(m) => m.to_string(),
        None => match &amend_target {
            Some((_, _, previous, _)) => previous.clone(),
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
    // snapshot's parent (and merge parent, below) so history is unchanged.
    let effective_parent = match &amend_hash {
        Some((_, orig_parent)) => orig_parent.trim().to_string(),
        None => parent_hash.trim().to_string(),
    };

    // ── Detect merge parent (second parent for merge commits) ────────────────
    // MERGE_HEAD stores "pre_merge_hash:source_branch". The source branch tip
    // at the time of the merge is the second parent of this snapshot.
    // An amend keeps the replaced snapshot's second parent: identity covers
    // parents and ancestry follows `merge_parent`, so dropping it would make
    // `merge_base` and `blame` give wrong answers.
    let merge_parent: String = if let Some((_, _, _, mp)) = &amend_target {
        mp.clone()
    } else if merge_pending {
        let info = fs::read_to_string(root.join(".velo/MERGE_HEAD")).unwrap_or_default();
        let source_branch = info.trim().split_once(':').map(|(_, b)| b).unwrap_or("");
        // Resolve the source to a snapshot: exact branch tip first, then a
        // remote ref like `origin/main`, so merges of either still record
        // their second parent (and a short branch name isn't read as a hash).
        let mut absent_ref = None;
        let resolved = crate::commands::branch_tip(conn, source_branch)
            .or_else(
                || match crate::commands::resolve_snapshot_id(guard.repo(), source_branch) {
                    Ok(id) => Some(id.into_string()),
                    Err(e @ crate::error::Error::RemoteRefAbsent { .. }) => {
                        absent_ref = Some(e);
                        None
                    }
                    Err(_) => None,
                },
            )
            .unwrap_or_default();
        // A merge parent must be a live snapshot; refuse before the transaction
        // opens rather than record one that does not exist.
        if let Some(e) = absent_ref {
            return Err(e);
        }
        if !resolved.is_empty() && !crate::commands::snapshot_is_live(conn, &resolved) {
            return Err(
                match crate::commands::resolve_snapshot_id(guard.repo(), source_branch) {
                    Err(e @ crate::error::Error::RemoteRefAbsent { .. }) => e,
                    _ => crate::error::Error::not_found(crate::error::RefKind::Snapshot, resolved),
                },
            );
        }
        resolved
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

    crate::timing::lap("save.prepare");

    // ── Parallel hash + compress ───────────────────────────────────────────────
    let objects = guard.repo().objects();
    // The dirty scan already hashed each changed file; passing its hash lets the
    // store skip reading a file whose object exists.
    let files_to_hash: Vec<(String, Option<String>)> = dirty
        .iter()
        .filter(|(_, s)| **s != FileStatus::Deleted)
        .map(|(p, _)| (p.clone(), known_hashes.get(p).cloned()))
        .collect();

    // Hash each changed file, capturing its mode. Symlinks store their target
    // string (not the pointed-at content); regular files store content.
    let progress = crate::progress::PhaseGuard::new(
        observer.unwrap_or_else(|| guard.repo().observer()),
        Phase::Hashing,
        Some(files_to_hash.len() as u64),
    );
    // Checked per file. Hashing writes objects, which is harmless to abandon:
    // an object nothing references is what `gc` collects, so stopping here
    // leaves no snapshot and no dangling reference.
    let hash_results: Result<Vec<(String, String, i64)>> =
        objects.put_paths_known(root, files_to_hash, &progress, || {
            crate::progress::Cancel::check(cancel)
        });
    // `mut` is only needed by the non-Unix sticky-exec-bit pass below; on Unix
    // nothing mutates this, so silence the lint there rather than diverge the
    // two platforms' code paths.
    #[cfg_attr(unix, allow(unused_mut))]
    let mut hashed_files = hash_results?;
    // Belt and braces: an empty dirty set means the loop above never ran, so a
    // cancellation set before the save started would otherwise slip through.
    crate::progress::Cancel::check(cancel)?;
    crate::timing::lap("save.hash_store");

    // ── Assemble the complete tree for this snapshot ──────────────────────────
    // Carry forward every unchanged file from the *dirty base* (the snapshot the
    // working tree was diffed against, i.e. PARENT — which for an amend is the
    // snapshot being replaced, NOT its parent), then add the freshly hashed
    // files. Carrying from PARENT rather than the effective parent is what keeps
    // an amend from dropping files the replaced snapshot introduced.
    let modified_paths: HashSet<&str> = hashed_files.iter().map(|(p, _, _)| p.as_str()).collect();
    let mut tree: Vec<(String, String, i64)> = {
        let carried: Vec<(String, String, i64)> =
            crate::tree_store::tree_of(conn, parent_hash.trim())?
                .into_iter()
                .map(|(p, (h, m))| (p, h, m))
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
    crate::timing::lap("save.parent_tree");

    // ── Content-addressed snapshot id ─────────────────────────────────────────
    // Caller metadata first, then authorship on top so the author always wins
    // the reserved keys.
    let mut snapshot_meta = meta;
    if let Some(author) = author {
        snapshot_meta.set_author(author);
    }

    let timestamp_ms = guard.repo().now_ms();
    let snapshot_hash = crate::commands::snapshot_id(SnapshotIdentity {
        tree: &tree,
        parent: effective_parent.trim(),
        merge_parent: merge_parent.as_str(),
        message,
        timestamp_ms,
        meta: &snapshot_meta,
    });
    let snapshot_hash = snapshot_hash.as_str();
    crate::timing::lap("save.snapshot_id");

    crate::failpoint::hit("save.objects_stored");

    // ── DB transaction ────────────────────────────────────────────────────────
    let tx = guard.transaction()?;

    // If amending, delete the old snapshot and its file_map first
    if let Some((old_hash, _)) = &amend_hash {
        crate::tree_store::delete_trees(&tx, &[old_hash.as_str()])?;
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
    crate::timing::lap("save.begin_tx");

    {
        let rows: Vec<(&str, &str, i64)> = tree
            .iter()
            .map(|(p, h, m)| (p.as_str(), h.as_str(), *m))
            .collect();
        crate::tree_store::write_tree(
            &tx,
            snapshot_hash,
            crate::tree_store::first_parent(&effective_parent),
            &rows,
        )?;
    }
    crate::timing::lap("save.insert_file_map");
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
    // A trash row that a live snapshot still builds on is the only copy of
    // that parent (a repository damaged by an older `undo`), so it stays for
    // `velo redo` to restore.
    tx.execute(
        "DELETE FROM trash_tags WHERE snapshot_hash IN
             (SELECT hash FROM trash WHERE branch = ?1
                AND hash NOT IN (SELECT parent_hash FROM snapshots
                UNION SELECT merge_parent FROM snapshots WHERE COALESCE(merge_parent, '') <> ''))",
        [branch.trim()],
    )?;
    tx.execute(
        "DELETE FROM trash WHERE branch = ?1
           AND hash NOT IN (SELECT parent_hash FROM snapshots
                UNION SELECT merge_parent FROM snapshots WHERE COALESCE(merge_parent, '') <> '')",
        [branch.trim()],
    )?;
    crate::timing::lap("save.insert_rest");
    crate::failpoint::hit("save.before_commit");
    tx.commit()?;
    crate::timing::lap("save.commit");
    crate::failpoint::hit("save.after_commit");
    // The scan left new files unhashed so each is read once, here. Seed their
    // cache rows from the stat the scan took *before* the read: a file edited
    // since then has a different mtime and is simply re-hashed next time.
    let seed: Vec<(String, i64, i64, String)> = hashed_files
        .iter()
        .filter_map(|(p, h, _)| {
            unhashed
                .get(p)
                .map(|(mtime, size)| (p.clone(), *mtime, *size, h.clone()))
        })
        .collect();
    crate::commands::seed_cache_entries(guard.repo(), &seed);
    guard.repo().emit_saved_raw(
        snapshot_hash,
        branch.trim(),
        effective_parent.as_str(),
        merge_parent.as_str(),
    );

    storage::write_atomic(&root.join(".velo/PARENT"), snapshot_hash.as_bytes())?;
    crate::timing::lap("save.parent_file");
    crate::failpoint::hit("save.after_parent");

    // If a merge was in progress, this save finalises it — clear the merge state
    let merge_head = root.join(".velo/MERGE_HEAD");
    if merge_head.exists() {
        let _ = fs::remove_file(&merge_head);
        let _ = fs::remove_file(root.join(".velo/MERGE_ORIG"));
    }

    Ok(Outcome::Saved(SaveResult {
        hash: SnapshotId::from_stored(snapshot_hash),
        new_count,
        modified_count,
        deleted_count,
    }))
}
