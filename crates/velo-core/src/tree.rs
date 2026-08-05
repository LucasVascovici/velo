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
//! use velo_core::SnapshotMeta;
//!
//! let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
//! let guard = repo.write()?;
//!
//! // Provenance travels with the snapshot instead of being smuggled into the
//! // message. It is part of the id, so it cannot be quietly rewritten later.
//! let mut meta = SnapshotMeta::new();
//! meta.set("registry", "published_by", "ci")?;
//!
//! let first = guard.save_tree(SaveTree {
//!     branch: "registry".parse()?,
//!     parent: None,
//!     message: "publish 1.0",
//!     entries: vec![TreeEntry::file("pkg/lib.rs", b"pub fn f() {}\n".to_vec())],
//!     meta,
//! })?;
//!
//! // Chain the next one onto it. Nothing was written to disk, and the
//! // repository's own branch and position are untouched.
//! let _second = guard.save_tree(SaveTree {
//!     branch: "registry".parse()?,
//!     parent: Some(&first),
//!     message: "publish 1.1",
//!     entries: vec![TreeEntry::file("pkg/lib.rs", b"pub fn f() -> u8 { 1 }\n".to_vec())],
//!     meta: SnapshotMeta::new(),
//! })?;
//!
//! assert_eq!(repo.read_file_at(&first, "pkg/lib.rs")?, b"pub fn f() {}\n");
//! assert_eq!(
//!     repo.snapshot_meta(&first)?.get("registry", "published_by"),
//!     Some("ci"),
//! );
//! # Ok(())
//! # }
//! ```
//!
//! # Why the ids are typed
//!
//! `branch` is a [`BranchName`] and `parent` a [`SnapshotId`], so the two cannot
//! be swapped and `save_tree` cannot be handed a tag or a hash prefix where a
//! branch belongs. Both parse from text, so the cost is a `.parse()?` — see
//! [`crate::ids`] for why user-typed *specs* stay `&str`.
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

use crate::commands::SnapshotIdentity;
use crate::error::{RefKind, Result, VeloError};
use crate::{db, storage, BranchName, ObjectHash, Repo, SnapshotId, SnapshotMeta, WriteGuard};

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

/// Where a tree entry's bytes come from.
///
/// A snapshot is a whole tree rather than a diff, which is what makes its id
/// verifiable — but it means changing one file requires naming every other file
/// too. Supplying the bytes for all of them would make each save cost the size of
/// the whole tree in decompression, hashing and recompression, so an entry can
/// instead point at content the store already holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// Bytes to store. Hashed and compressed; a duplicate costs nothing extra
    /// because the store is content-addressed.
    Bytes(Vec<u8>),
    /// An object already in the store, named by its hash — as handed back by
    /// [`TreeFile::object`]. Nothing is read, hashed or written for it.
    Stored(ObjectHash),
}

/// One file to put into a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    /// Repository-relative path. Backslashes are normalised to forward slashes,
    /// so a caller on Windows needn't care which it uses.
    pub path: String,
    pub content: Content,
    pub kind: FileKind,
}

impl TreeEntry {
    /// An ordinary file, from bytes.
    pub fn file(path: impl Into<String>, content: Vec<u8>) -> Self {
        TreeEntry {
            path: path.into(),
            content: Content::Bytes(content),
            kind: FileKind::Regular,
        }
    }

    /// A file whose content is already in the store.
    ///
    /// This is how a tree is carried forward cheaply. Reading a tree gives you a
    /// [`TreeFile`] per path with its `object`, so building the next snapshot is
    /// a map with no I/O:
    ///
    /// ```no_run
    /// # fn main() -> Result<(), velo_core::Error> {
    /// # use velo_core::tree::{SaveTree, TreeEntry};
    /// # let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
    /// # let previous = velo_core::commands::resolve_snapshot_id(&repo, "main")?;
    /// let mut entries: Vec<TreeEntry> = repo
    ///     .tree_at(&previous)?
    ///     .into_iter()
    ///     .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
    ///     .collect();
    ///
    /// // …then replace or add just the ones that changed.
    /// entries.push(TreeEntry::file("notes.txt", b"new".to_vec()));
    /// # Ok(()) }
    /// ```
    ///
    /// The object must exist: [`WriteGuard::save_tree`] rejects a hash the store
    /// does not hold, rather than recording a snapshot that `fsck` would later
    /// report as corrupt.
    pub fn stored(path: impl Into<String>, object: ObjectHash, kind: FileKind) -> Self {
        TreeEntry {
            path: path.into(),
            content: Content::Stored(object),
            kind,
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
            content: Content::Bytes(target.into().into_bytes()),
            kind: FileKind::Symlink,
        }
    }
}

/// A snapshot to create from in-memory content.
#[derive(Clone, Debug)]
pub struct SaveTree<'a> {
    /// Branch to record the snapshot on. Since a tip is derived from this, using
    /// a name of your own keeps the snapshot out of the way of everything else.
    pub branch: BranchName,
    /// The snapshot this one follows, or `None` to start a history.
    pub parent: Option<&'a SnapshotId>,
    pub message: &'a str,
    /// The complete contents of the snapshot — this is a whole tree, not a diff.
    pub entries: Vec<TreeEntry>,
    /// App-namespaced metadata to attach.
    ///
    /// Part of the snapshot's identity, so two snapshots differing only in their
    /// metadata are two different snapshots. `Default::default()` is an empty
    /// set, which is what `velo save` produces.
    pub meta: SnapshotMeta,
}

/// One file in a stored snapshot, without its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFile {
    pub path: String,
    /// The object holding this file's content, for [`Repo::read_object`].
    pub object: ObjectHash,
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
    pub fn save_tree(&self, spec: SaveTree<'_>) -> Result<SnapshotId> {
        // No empty-branch check: `BranchName` cannot be empty by construction.
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
                return Err(VeloError::not_found(RefKind::Snapshot, parent.as_str()));
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
            let object = match entry.content {
                Content::Bytes(bytes) => {
                    // Line endings are normalised for text exactly as the
                    // filesystem path normalises them. Without this, content
                    // saved from memory and the same content saved from disk
                    // would land in different objects — and a file restored with
                    // its CRLFs intact would then re-hash to something else and
                    // read as permanently modified.
                    let content = if mode == storage::MODE_SYMLINK {
                        bytes
                    } else {
                        storage::normalise_crlf(bytes)
                    };
                    storage::store_raw(&objects_dir, &content)?
                }
                Content::Stored(object) => {
                    // Already-stored content is already normalised, so it is
                    // taken as-is. But it must actually be there: recording a
                    // snapshot that names a missing object would manufacture
                    // corruption through the public API, discoverable only later
                    // by `fsck`.
                    if !objects_dir.join(object.as_str()).exists() {
                        return Err(VeloError::MissingObject {
                            hash: object.into_string(),
                        });
                    }
                    object.into_string()
                }
            };
            tree.push((path, object, mode));
        }

        let parent = spec.parent.map_or("", |p| p.as_str());
        let timestamp_ms = crate::commands::snapshot_timestamp_ms();
        let snapshot = SnapshotId::from_stored(crate::commands::snapshot_id(SnapshotIdentity {
            tree: &tree,
            parent,
            merge_parent: "",
            message: spec.message,
            timestamp_ms,
            meta: &spec.meta,
        }));

        let tx = self.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
             VALUES (?, ?, ?, ?, '', ?)",
            params![snapshot, spec.message, spec.branch, parent, timestamp_ms],
        )?;
        {
            // Metadata is hashed into the id above, so it has to be stored in the
            // same transaction: an id that commits to metadata the repository
            // does not hold would fail its own `fsck`.
            let mut ins_meta = tx.prepare(
                "INSERT INTO snapshot_meta (snapshot_id, namespace, key, value)
                 VALUES (?, ?, ?, ?)",
            )?;
            for (namespace, key, value) in spec.meta.iter() {
                ins_meta.execute(params![snapshot, namespace, key, value])?;
            }
        }
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
        crate::commands::register_branch(&tx, &spec.branch, &snapshot)?;
        tx.commit()?;

        Ok(snapshot)
    }
}

impl Repo {
    /// Every file in `snapshot`, in path order, without their content.
    ///
    /// Use [`Repo::read_object`] to fetch a file's bytes, or
    /// [`Repo::read_file_at`] to go straight from a path to its content.
    pub fn tree_at(&self, snapshot: &SnapshotId) -> Result<Vec<TreeFile>> {
        let known: bool = self
            .conn()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !known {
            return Err(VeloError::not_found(RefKind::Snapshot, snapshot.as_str()));
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
    pub fn read_file_at(&self, snapshot: &SnapshotId, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        let rel = db::normalise(&path.as_ref().to_string_lossy());
        let object: Option<ObjectHash> = self
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
                snapshot.short()
            ))),
        }
    }

    /// The bytes of a stored object, by its content hash.
    ///
    /// The hash is verified on the way out, so a corrupted store is an error
    /// rather than silently wrong content.
    pub fn read_object(&self, object: &ObjectHash) -> Result<Vec<u8>> {
        storage::read_object(&self.root().join(".velo/objects"), object)
    }

    /// The metadata attached to `snapshot`.
    ///
    /// Empty when none was attached — which is not distinguishable from "the
    /// snapshot has no metadata", because to the id recipe those are the same
    /// thing.
    pub fn snapshot_meta(&self, snapshot: &SnapshotId) -> Result<SnapshotMeta> {
        Ok(crate::commands::load_snapshot_meta(self.conn(), snapshot)?)
    }
}
