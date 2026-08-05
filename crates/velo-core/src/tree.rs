//! Snapshots built from content in memory, not from a working tree.
//!
//! Velo's own model is "the disk is the snapshot", which is right for a version
//! control tool and wrong for an editor, a package registry, or anything else
//! that holds content in memory and never wants a scratch directory. This module
//! is the other adapter onto the same store: hand it bytes, get a snapshot back.
//!
//! ```no_run
//! # fn main() -> Result<(), velo_core::Error> {
//! use velo_core::tree::{SaveTree, TreeEntry};
//!
//! let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
//! let guard = repo.write()?;
//!
//! let first = guard.save_tree(SaveTree {
//!     branch: "registry",
//!     parent: None,
//!     message: "publish 1.0",
//!     entries: vec![TreeEntry::file("pkg/lib.rs", b"pub fn f() {}\n".to_vec())],
//! })?;
//!
//! // Chain the next one onto it. Nothing was written to disk, and the
//! // repository's own branch and position are untouched.
//! let _second = guard.save_tree(SaveTree {
//!     branch: "registry",
//!     parent: Some(&first),
//!     message: "publish 1.1",
//!     entries: vec![TreeEntry::file("pkg/lib.rs", b"pub fn f() -> u8 { 1 }\n".to_vec())],
//! })?;
//!
//! assert_eq!(repo.read_file_at(&first, "pkg/lib.rs")?, b"pub fn f() {}\n");
//! # Ok(())
//! # }
//! ```
//!
//! # Why the branch is explicit
//!
//! A branch's tip is *derived* — it is the newest snapshot carrying that branch
//! name — so inserting a snapshot on a branch moves it, with no ref to update.
//! Passing the branch in makes that consequence visible: a consumer that gives
//! its own name can never disturb the branches a human is using, and one that
//! passes `"main"` has said so deliberately.
//!
//! Nothing here touches `.velo/PARENT` or the working tree. That is the point:
//! a headless consumer has no working tree, and silently moving the position out
//! from under one that does would leave every file looking modified.

use std::path::Path;

use rusqlite::params;

use crate::error::{RefKind, Result, VeloError};
use crate::{db, storage, Repo, WriteGuard};

/// What a file is: content, plus how the filesystem should represent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// An ordinary file.
    Regular,
    /// An ordinary file that should be executable when written out.
    Executable,
    /// A symbolic link. Its "content" is the target path.
    Symlink,
}

impl FileKind {
    fn mode(self) -> i64 {
        match self {
            FileKind::Regular => storage::MODE_REGULAR,
            FileKind::Executable => storage::MODE_EXEC,
            FileKind::Symlink => storage::MODE_SYMLINK,
        }
    }

    fn from_mode(mode: i64) -> FileKind {
        match mode {
            storage::MODE_EXEC => FileKind::Executable,
            storage::MODE_SYMLINK => FileKind::Symlink,
            _ => FileKind::Regular,
        }
    }
}

/// One file to put into a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    /// Repository-relative path. Backslashes are normalised to forward slashes,
    /// so a caller on Windows needn't care which it uses.
    pub path: String,
    pub content: Vec<u8>,
    pub kind: FileKind,
}

impl TreeEntry {
    /// An ordinary file.
    pub fn file(path: impl Into<String>, content: Vec<u8>) -> Self {
        TreeEntry {
            path: path.into(),
            content,
            kind: FileKind::Regular,
        }
    }

    /// An executable file.
    pub fn executable(path: impl Into<String>, content: Vec<u8>) -> Self {
        TreeEntry {
            kind: FileKind::Executable,
            ..TreeEntry::file(path, content)
        }
    }

    /// A symlink pointing at `target`.
    pub fn symlink(path: impl Into<String>, target: impl Into<String>) -> Self {
        TreeEntry {
            path: path.into(),
            content: target.into().into_bytes(),
            kind: FileKind::Symlink,
        }
    }
}

/// A snapshot to create from in-memory content.
#[derive(Clone, Debug)]
pub struct SaveTree<'a> {
    /// Branch to record the snapshot on. Since a tip is derived from this, using
    /// a name of your own keeps the snapshot out of the way of everything else.
    pub branch: &'a str,
    /// The snapshot this one follows, or `None` to start a history.
    pub parent: Option<&'a str>,
    pub message: &'a str,
    /// The complete contents of the snapshot — this is a whole tree, not a diff.
    pub entries: Vec<TreeEntry>,
}

/// One file in a stored snapshot, without its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    /// The object holding this file's content, for [`Repo::read_object`].
    pub object: String,
    pub kind: FileKind,
}

impl WriteGuard<'_> {
    /// Create a snapshot from content held in memory.
    ///
    /// Returns the new snapshot's id. Nothing is written to the working tree and
    /// neither `.velo/PARENT` nor any other branch is touched — see the
    /// [module docs](crate::tree) for why.
    ///
    /// Content is stored exactly as the filesystem path would store it, so a
    /// snapshot built this way is indistinguishable from one built by `save`:
    /// the same bytes yield the same object, and the same tree yields the same
    /// snapshot id.
    pub fn save_tree(&self, spec: SaveTree<'_>) -> Result<String> {
        if spec.branch.is_empty() {
            return Err(VeloError::invalid(
                "save_tree needs a branch to record the snapshot on.",
            ));
        }
        if let Some(parent) = spec.parent {
            let known: bool = self
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                    [parent],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !known {
                return Err(VeloError::not_found(RefKind::Snapshot, parent));
            }
        }

        let objects_dir = self.root().join(".velo/objects");
        let mut tree: Vec<(String, String, i64)> = Vec::with_capacity(spec.entries.len());
        let mut seen = std::collections::HashSet::new();

        for entry in spec.entries {
            let path = db::normalise(&entry.path);
            if path.is_empty() {
                return Err(VeloError::invalid("a tree entry needs a path."));
            }
            if !seen.insert(path.clone()) {
                return Err(VeloError::invalid(format!(
                    "'{}' appears twice in the same tree.",
                    path
                )));
            }
            let mode = entry.kind.mode();
            // Line endings are normalised for text exactly as the filesystem
            // path normalises them. Without this, content saved from memory and
            // the same content saved from disk would land in different objects —
            // and a file restored with its CRLFs intact would then re-hash to
            // something else and read as permanently modified.
            let content = if mode == storage::MODE_SYMLINK {
                entry.content
            } else {
                storage::normalise_crlf(entry.content)
            };
            let object = storage::store_raw(&objects_dir, &content)?;
            tree.push((path, object, mode));
        }

        let parent = spec.parent.unwrap_or("");
        let timestamp = crate::commands::snapshot_timestamp();
        let snapshot = crate::commands::snapshot_id(&tree, parent, "", spec.message, &timestamp);

        let tx = self.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at)
             VALUES (?, ?, ?, ?, '', ?)",
            params![snapshot, spec.message, spec.branch, parent, timestamp],
        )?;
        {
            let mut ins = tx.prepare(
                "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
            )?;
            for (path, object, mode) in &tree {
                ins.execute(params![snapshot, path, object, mode])?;
            }
        }
        // Through `tx`, not the connection: an unchecked transaction begins on the
        // shared connection, so writing via `self.conn()` here would land inside
        // this transaction anyway — but only by accident of how it is
        // implemented. Being explicit says the branch row commits with the rest.
        crate::commands::register_branch(&tx, spec.branch, &snapshot)?;
        tx.commit()?;

        Ok(snapshot)
    }
}

impl Repo {
    /// Every file in `snapshot`, in path order, without their content.
    ///
    /// Use [`Repo::read_object`] to fetch a file's bytes, or
    /// [`Repo::read_file_at`] to go straight from a path to its content.
    pub fn tree_at(&self, snapshot: &str) -> Result<Vec<TreeFile>> {
        let known: bool = self
            .conn()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !known {
            return Err(VeloError::not_found(RefKind::Snapshot, snapshot));
        }

        let mut stmt = self.conn().prepare(
            "SELECT path, hash, mode FROM file_map WHERE snapshot_hash = ? ORDER BY path",
        )?;
        let files: Vec<TreeFile> = stmt
            .query_map([snapshot], |r| {
                Ok(TreeFile {
                    path: r.get(0)?,
                    object: r.get(1)?,
                    kind: FileKind::from_mode(r.get::<_, i64>(2)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(files)
    }

    /// The content of `path` as it was in `snapshot`.
    ///
    /// For a symlink this is the target path, matching how it was stored.
    pub fn read_file_at(&self, snapshot: &str, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        let rel = db::normalise(&path.as_ref().to_string_lossy());
        let object: Option<String> = self
            .conn()
            .query_row(
                "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
                params![snapshot, rel],
                |r| r.get(0),
            )
            .ok();
        match object {
            Some(object) => self.read_object(&object),
            None => Err(VeloError::invalid(format!(
                "'{}' is not in snapshot {}.",
                rel,
                &snapshot[..8.min(snapshot.len())]
            ))),
        }
    }

    /// The bytes of a stored object, by its content hash.
    ///
    /// The hash is verified on the way out, so a corrupted store is an error
    /// rather than silently wrong content.
    pub fn read_object(&self, object: &str) -> Result<Vec<u8>> {
        storage::read_object(&self.root().join(".velo/objects"), object)
    }
}
