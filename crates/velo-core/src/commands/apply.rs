//! Three-way tree reconciliation, shared by `merge`, `cherry-pick` and `rebase`.
//!
//! All three answer the same question — given a common ancestor, our tree and
//! theirs, what should the working tree become? — and all three had their own
//! copy of the loop, with counters that had drifted apart (one of them silently
//! dropped delete/modify decisions, and none of them iterated paths in a
//! deterministic order).
//!
//! This module owns the loop and the vocabulary for its result. It writes to the
//! working tree but reports rather than prints, so each command's renderer words
//! the outcome its own way.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use rusqlite::params;

use crate::error::Result;
use crate::storage;

/// A snapshot's tree: path → (object hash, mode).
pub type Tree = HashMap<String, (String, i64)>;

/// What reconciliation decided for one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAction {
    /// Deleted on the incoming side and untouched on ours.
    Deleted,
    /// Taken from the incoming side; we didn't have it before.
    Added,
    /// Taken wholesale from the incoming side.
    Updated,
    /// Both sides changed it, in places that didn't overlap.
    AutoMerged,
    /// Deleted on one side and modified on the other. Ours is kept; the user
    /// decides whether that was right.
    KeptOurs,
    /// Both sides changed the same lines. Left for the user to resolve.
    Conflicted,
}

/// One file's reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOutcome {
    pub path: String,
    pub action: FileAction,
}

/// A conflict's three sides, ready to be recorded for `velo resolve`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ConflictRow {
    path: String,
    ancestor: String,
    ours: String,
    theirs: String,
}

/// The result of reconciling two trees against their common ancestor.
#[derive(Clone, Debug)]
pub struct Applied {
    /// Every file that changed, in path order. Untouched files are omitted.
    pub files: Vec<FileOutcome>,
    conflicts: Vec<ConflictRow>,
}

impl Applied {
    fn count(&self, action: FileAction) -> usize {
        self.files.iter().filter(|f| f.action == action).count()
    }

    /// Files taken from the incoming side that we didn't have.
    pub fn added(&self) -> usize {
        self.count(FileAction::Added)
    }

    /// Files whose content changed, whether taken wholesale or auto-merged.
    pub fn updated(&self) -> usize {
        self.count(FileAction::Updated) + self.count(FileAction::AutoMerged)
    }

    pub fn deleted(&self) -> usize {
        self.count(FileAction::Deleted)
    }

    /// Paths needing resolution, in path order.
    pub fn conflicts(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|f| f.action == FileAction::Conflicted)
            .map(|f| f.path.as_str())
            .collect()
    }

    /// No conflicts were left behind.
    pub fn is_clean(&self) -> bool {
        !self
            .files
            .iter()
            .any(|f| f.action == FileAction::Conflicted)
    }

    /// Nothing was applied — the two sides already agreed on every file.
    ///
    /// A kept-ours delete/modify decision doesn't count as applying anything,
    /// which is why this isn't simply "no files".
    pub fn applied_nothing(&self) -> bool {
        self.added() + self.updated() + self.deleted() == 0
    }

    /// Record the conflicts so `velo resolve` can find them.
    pub(crate) fn record_conflicts(&self, conn: &rusqlite::Connection) -> Result<()> {
        for c in &self.conflicts {
            conn.execute(
                "INSERT OR REPLACE INTO conflict_files (path, ancestor_hash, our_hash, their_hash)
                 VALUES (?, ?, ?, ?)",
                params![c.path, c.ancestor, c.ours, c.theirs],
            )?;
        }
        Ok(())
    }
}

/// Reconcile `ours` and `theirs` against `ancestor`, writing the result into the
/// working tree under `root`.
///
/// Paths are visited in sorted order, so the reported outcome is the same on
/// every run — it used to come out of a `HashSet`, making the order the hasher's.
pub fn reconcile_tree(root: &Path, ancestor: &Tree, ours: &Tree, theirs: &Tree) -> Result<Applied> {
    let objects_dir = root.join(".velo/objects");

    // Every path any of the three sides knows about. A path only the ancestor has
    // was deleted on both sides, which reconciles to "nothing" — including it
    // costs one comparison and removes a whole class of edge case.
    let all_paths: BTreeSet<&str> = ancestor
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .map(String::as_str)
        .collect();

    let mut files = Vec::new();
    let mut conflicts = Vec::new();

    for path in all_paths {
        let side = |t: &Tree| {
            t.get(path)
                .map(|(h, m)| (h.clone(), *m))
                .unwrap_or_default()
        };
        let (anc_h, anc_m) = side(ancestor);
        let (our_h, our_m) = side(ours);
        let (thr_h, thr_m) = side(theirs);
        let anc = (anc_h.as_str(), anc_m);
        let our = (our_h.as_str(), our_m);
        let thr = (thr_h.as_str(), thr_m);

        let full = root.join(crate::db::db_to_path(path));

        let action = match crate::commands::reconcile_file(&objects_dir, anc, our, thr)? {
            crate::commands::Reconcile::Nothing => continue,
            crate::commands::Reconcile::Delete => {
                if full.exists() {
                    fs::remove_file(&full)?;
                }
                FileAction::Deleted
            }
            crate::commands::Reconcile::TakeTheirs { hash, mode, is_new } => {
                write_file(&full, mode, &storage::read_object(&objects_dir, &hash)?)?;
                if is_new {
                    FileAction::Added
                } else {
                    FileAction::Updated
                }
            }
            crate::commands::Reconcile::AutoMerged { content, mode } => {
                write_file(&full, mode, &content)?;
                FileAction::AutoMerged
            }
            crate::commands::Reconcile::KeepOurs => FileAction::KeptOurs,
            crate::commands::Reconcile::Conflict => {
                conflicts.push(ConflictRow {
                    path: path.to_string(),
                    ancestor: anc_h.clone(),
                    ours: our_h.clone(),
                    theirs: thr_h.clone(),
                });
                FileAction::Conflicted
            }
        };
        files.push(FileOutcome {
            path: path.to_string(),
            action,
        });
    }

    Ok(Applied { files, conflicts })
}

fn write_file(full: &Path, mode: i64, content: &[u8]) -> Result<()> {
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    storage::apply_file(full, mode, content)?;
    Ok(())
}

/// Load a snapshot's tree. An empty hash yields an empty tree, which is how a
/// root commit's "ancestor" is represented.
pub(crate) fn load_tree(conn: &rusqlite::Connection, snapshot: &str) -> Result<Tree> {
    if snapshot.is_empty() {
        return Ok(Tree::new());
    }
    let mut stmt = conn.prepare("SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ?")?;
    let tree: Tree = stmt
        .query_map([snapshot], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tree)
}
