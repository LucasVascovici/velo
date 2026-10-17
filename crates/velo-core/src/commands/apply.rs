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

use rayon::prelude::*;

use crate::commands::{classify, merge_contents, require_working_tree, Classified, Reconcile};
use crate::error::{Result, VeloError};
use crate::progress::Phase;
use crate::storage;
use crate::WriteGuard;

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
        // One transaction, so a kill leaves every conflict row or none.
        let tx = conn.unchecked_transaction()?;
        for c in &self.conflicts {
            tx.execute(
                "INSERT OR REPLACE INTO conflict_files (path, ancestor_hash, our_hash, their_hash)
                 VALUES (?, ?, ?, ?)",
                params![c.path, c.ancestor, c.ours, c.theirs],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Reconcile `ours` and `theirs` against `ancestor`, writing the result into the
/// working tree under `root`.
///
/// Paths are visited in sorted order, so the reported outcome is the same on
/// every run — it used to come out of a `HashSet`, making the order the hasher's.
pub fn reconcile_tree(
    guard: &WriteGuard,
    ancestor: &Tree,
    ours: &Tree,
    theirs: &Tree,
) -> Result<Applied> {
    reconcile_tree_moving(guard, ancestor, ours, theirs, &[])
}

/// [`reconcile_tree`] for trees already remapped around moves.
///
/// For a move their side made, `ours` holds our file at `to` but the working
/// tree still has it at `from`, so it is renamed on disk before anything is
/// written. Classification at `to` then matches the disk: a conflict leaves our
/// content there, as it does for any conflicted file.
pub(crate) fn reconcile_tree_moving(
    guard: &WriteGuard,
    ancestor: &Tree,
    ours: &Tree,
    theirs: &Tree,
    moves: &[crate::commands::moves::Moved],
) -> Result<Applied> {
    require_working_tree(guard.repo(), "apply")?;
    let root = guard.root();
    let objects = guard.repo().objects();

    // Every path any of the three sides knows about. A path only the ancestor has
    // was deleted on both sides, which reconciles to "nothing" — including it
    // costs one comparison and removes a whole class of edge case.
    let all_paths: BTreeSet<&str> = ancestor
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .map(String::as_str)
        .collect();

    let progress = guard.phase(Phase::Reconciling, Some(all_paths.len() as u64));

    // Pass 1 (serial, cheap): classify every path from hashes and modes alone.
    struct Planned<'a> {
        path: &'a str,
        anc_h: &'a str,
        our_h: &'a str,
        thr_h: &'a str,
        thr_m: i64,
        decision: Option<Reconcile>,
    }
    let mut planned: Vec<Planned> = Vec::new();
    for path in all_paths {
        progress.tick();
        let (anc_h, anc_m) = side(ancestor, path);
        let (our_h, our_m) = side(ours, path);
        let (thr_h, thr_m) = side(theirs, path);
        let decision = match classify((anc_h, anc_m), (our_h, our_m), (thr_h, thr_m)) {
            Classified::Done(Reconcile::Nothing) => continue,
            Classified::Done(r) => Some(r),
            Classified::NeedsContent => None,
        };
        planned.push(Planned {
            path,
            anc_h,
            our_h,
            thr_h,
            thr_m,
            decision,
        });
    }

    // Pass 2: read each object the content merges need exactly once, in
    // parallel, then merge in parallel. Workers never touch the Repo or its
    // Connection: the database backend fetches on this thread (see
    // `par_with_content`), and the drivers are `Send + Sync`.
    let needed: Vec<&str> = planned
        .iter()
        .filter(|p| p.decision.is_none())
        .flat_map(|p| [p.anc_h, p.our_h, p.thr_h])
        .filter(|h| !h.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let fetched: Vec<(&str, Result<Vec<u8>>)> =
        objects.par_with_content(&needed, |h| h, |h, content| Some((*h, content)));
    let mut contents: HashMap<&str, Vec<u8>> = HashMap::with_capacity(fetched.len());
    for (h, content) in fetched {
        contents.insert(h, content?);
    }
    let drivers = guard.repo().drivers();
    let empty: Vec<u8> = Vec::new();
    let merged: Vec<(usize, Reconcile)> = planned
        .par_iter()
        .enumerate()
        .filter(|(_, p)| p.decision.is_none())
        .map(|(i, p)| {
            let get = |h: &str| contents.get(h).unwrap_or(&empty);
            let r = merge_contents(
                get(p.anc_h),
                get(p.our_h),
                get(p.thr_h),
                p.thr_m,
                drivers.for_path(p.path),
            );
            (i, r)
        })
        .collect();
    for (i, r) in merged {
        planned[i].decision = Some(r);
    }
    drop(contents);

    // Validate before pass 3 touches the disk. The incoming tree must be a
    // sound tree on its own, and every path about to be written, deleted or
    // moved must be one velo will write on this platform and must not reach
    // the working tree through a symlink. A refusal leaves the tree untouched.
    {
        let writing: Vec<(&str, &str, i64)> = planned
            .iter()
            .filter_map(|p| match p.decision.as_ref() {
                Some(Reconcile::TakeTheirs { hash, mode, .. }) => {
                    Some((p.path, hash.as_str(), *mode))
                }
                Some(Reconcile::AutoMerged { .. }) | Some(Reconcile::Delete) => {
                    Some((p.path, "", 0))
                }
                _ => None,
            })
            .collect();
        crate::safe_path::validate_write_set(
            root,
            theirs.keys().map(String::as_str),
            writing,
            |h| objects.get(h),
        )?;
        for m in moves
            .iter()
            .filter(|m| m.by == crate::commands::moves::MovedBy::Theirs)
        {
            crate::safe_path::join_checked(root, &m.from)?;
            crate::safe_path::join_checked(root, &m.to)?;
        }
    }

    // Pass 3: write. Deletes run first, as `restore` removes ghosts before
    // writing: an incoming change from file `a` to `a/b` must remove `a`
    // before `a/b` can create its directory. Then wholesale takes read their
    // object in parallel and merged contents are written in parallel. Every
    // failure is reported.
    let mut errors: Vec<String> = Vec::new();
    for m in moves
        .iter()
        .filter(|m| m.by == crate::commands::moves::MovedBy::Theirs)
    {
        let (Ok(old), Ok(new)) = (
            crate::safe_path::join_checked(root, &m.from),
            crate::safe_path::join_checked(root, &m.to),
        ) else {
            errors.push(format!("refused move '{}' to '{}'", m.from, m.to));
            continue;
        };
        let moved = (|| -> std::io::Result<()> {
            if let Some(parent) = new.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&old, &new)?;
            if let Some(parent) = old.parent() {
                crate::commands::remove_empty_parents(parent, root);
            }
            Ok(())
        })();
        if let Err(e) = moved {
            errors.push(format!("move '{}' to '{}': {e}", m.from, m.to));
        }
    }
    errors.extend(
        planned
            .par_iter()
            .filter_map(|p| {
                if !matches!(p.decision.as_ref(), Some(Reconcile::Delete)) {
                    return None;
                }
                let Ok(full) = crate::safe_path::join_checked(root, p.path) else {
                    return Some(format!("refused path '{}'", p.path));
                };
                if full.exists() {
                    fs::remove_file(&full)
                        .err()
                        .map(|e| format!("delete '{}': {e}", p.path))
                } else {
                    None
                }
            })
            .collect::<Vec<String>>(),
    );
    let take_items: Vec<(&str, &str, i64)> = planned
        .iter()
        .filter_map(|p| match p.decision.as_ref() {
            Some(Reconcile::TakeTheirs { hash, mode, .. }) => Some((p.path, hash.as_str(), *mode)),
            _ => None,
        })
        .collect();
    errors.extend(objects.par_with_content(
        &take_items,
        |(_, hash, _)| hash,
        |(path, _, mode), content| {
            let Ok(full) = crate::safe_path::join_checked(root, path) else {
                return Some(format!("refused path '{path}'"));
            };
            match content {
                Ok(data) => write_file(&full, *mode, &data)
                    .err()
                    .map(|e| format!("write '{path}': {e}")),
                Err(e) => Some(format!("read object for '{path}': {e}")),
            }
        },
    ));
    errors.extend(
        planned
            .par_iter()
            .filter_map(|p| match p.decision.as_ref() {
                Some(Reconcile::AutoMerged { content, mode }) => {
                    let Ok(full) = crate::safe_path::join_checked(root, p.path) else {
                        return Some(format!("refused path '{}'", p.path));
                    };
                    write_file(&full, *mode, content)
                        .err()
                        .map(|e| format!("write '{}': {e}", p.path))
                }
                _ => None,
            })
            .collect::<Vec<String>>(),
    );
    if !errors.is_empty() {
        errors.sort();
        return Err(VeloError::invalid(format!(
            "{} file(s) could not be written:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }

    crate::failpoint::hit("apply.after_files");

    // Pass 4: the report, in sorted path order.
    let mut files = Vec::with_capacity(planned.len());
    let mut conflicts = Vec::new();
    for p in &planned {
        let action = match p.decision.as_ref().expect("decided above") {
            Reconcile::Nothing => continue,
            Reconcile::Delete => FileAction::Deleted,
            Reconcile::TakeTheirs { is_new: true, .. } => FileAction::Added,
            Reconcile::TakeTheirs { .. } => FileAction::Updated,
            Reconcile::AutoMerged { .. } => FileAction::AutoMerged,
            Reconcile::KeepOurs => FileAction::KeptOurs,
            Reconcile::Conflict => {
                conflicts.push(ConflictRow {
                    path: p.path.to_string(),
                    ancestor: p.anc_h.to_string(),
                    ours: p.our_h.to_string(),
                    theirs: p.thr_h.to_string(),
                });
                FileAction::Conflicted
            }
        };
        files.push(FileOutcome {
            path: p.path.to_string(),
            action,
        });
    }

    // A move of theirs shows as the old path deleted and, when nothing else
    // reported the new path, as added; `Nothing` never reaches `planned`.
    let mut moved = false;
    for m in moves
        .iter()
        .filter(|m| m.by == crate::commands::moves::MovedBy::Theirs)
    {
        moved = true;
        files.push(FileOutcome {
            path: m.from.clone(),
            action: FileAction::Deleted,
        });
        if !files.iter().any(|f| f.path == m.to) {
            files.push(FileOutcome {
                path: m.to.clone(),
                action: FileAction::Added,
            });
        }
    }
    if moved {
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }

    Ok(Applied { files, conflicts })
}

/// A path's hash ("" = absent) and mode in `tree`, borrowed from it.
fn side<'a>(tree: &'a Tree, path: &str) -> (&'a str, i64) {
    tree.get(path).map_or(("", 0), |(h, m)| (h.as_str(), *m))
}

fn write_file(full: &Path, mode: i64, content: &[u8]) -> Result<()> {
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    storage::apply_file(full, mode, content)?;
    // Hit after every file, on whichever pool thread wrote it; a handler that
    // stops at the first hit stops after the first file to land.
    crate::failpoint::hit("apply.after_first_file");
    Ok(())
}

/// Load a snapshot's tree. An empty hash yields an empty tree, which is how a
/// root commit's "ancestor" is represented.
pub(crate) fn load_tree(conn: &rusqlite::Connection, snapshot: &str) -> Result<Tree> {
    if snapshot.is_empty() {
        return Ok(Tree::new());
    }
    Ok(crate::treestore::tree_of(conn, snapshot)?
        .into_iter()
        .map(|(path, hash, mode)| (path, (hash, mode)))
        .collect())
}
