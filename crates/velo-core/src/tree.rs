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
//! // Bound once and borrowed by each save, rather than cloned per call.
//! let branch: velo_core::BranchName = "registry".parse()?;
//!
//! // Provenance travels with the snapshot instead of being smuggled into the
//! // message. It is part of the id, so it cannot be quietly rewritten later.
//! let mut meta = SnapshotMeta::new();
//! meta.set("registry", "published_by", "ci")?;
//!
//! let first = guard.save_tree(SaveTree {
//!     branch: &branch,
//!     parent: None,
//!     merge_parent: None,
//!     message: "publish 1.0",
//!     entries: vec![TreeEntry::file("pkg/lib.rs", b"pub fn f() {}\n".to_vec())],
//!     meta,
//!     timestamp_ms: None,
//!     author: None,
//!     renames: &[],
//! })?;
//!
//! // Chain the next one onto it. Nothing was written to disk, and the
//! // repository's own branch and position are untouched.
//! let _second = guard.save_tree(SaveTree {
//!     branch: &branch,
//!     parent: Some(&first),
//!     merge_parent: None,
//!     message: "publish 1.1",
//!     entries: vec![TreeEntry::file("pkg/lib.rs", b"pub fn f() -> u8 { 1 }\n".to_vec())],
//!     meta: SnapshotMeta::new(),
//!     timestamp_ms: None,
//!     author: None,
//!     renames: &[],
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

use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::commands::SnapshotIdentity;
use crate::error::{RefKind, Result, VeloError};
use crate::{
    db, storage, Author, BranchName, ObjectHash, Repo, SnapshotId, SnapshotMeta, WriteGuard,
};

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

    pub(crate) fn from_mode(mode: i64) -> FileKind {
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
    ///
    /// Borrowed, like `parent`: a consumer that publishes to one branch holds a
    /// `BranchName` and would otherwise clone it on every save. For a one-off,
    /// bind it first — `let branch = "registry".parse()?;`.
    pub branch: &'a BranchName,
    /// The snapshot this one follows, or `None` to start a history.
    pub parent: Option<&'a SnapshotId>,
    /// The second parent, when this snapshot is a merge.
    ///
    /// Recording it matters beyond drawing the graph: merge-base computation
    /// walks both parents, so a merge saved as a linear snapshot hands the *next*
    /// merge an ancestor that is too old, and conflicts the author already
    /// resolved are presented again.
    pub merge_parent: Option<&'a SnapshotId>,
    pub message: &'a str,
    /// The complete contents of the snapshot — this is a whole tree, not a diff.
    pub entries: Vec<TreeEntry>,
    /// App-namespaced metadata to attach.
    ///
    /// Part of the snapshot's identity, so two snapshots differing only in their
    /// metadata are two different snapshots. `Default::default()` is an empty
    /// set, which is what `velo save` produces.
    pub meta: SnapshotMeta,
    /// Who made this snapshot.
    ///
    /// Recorded in the reserved `velo` metadata namespace, so it is hashed into
    /// the id and cannot be quietly rewritten — see [`Author`]. `None` records no
    /// author, which is what every snapshot written before this existed has.
    pub author: Option<&'a Author>,
    /// Paths this snapshot moved, as `(from, to)`.
    ///
    /// A rename is **recorded, not detected**. `entries` is a whole tree, so a
    /// move arrives as a delete and an add and is gone by construction — but the
    /// caller that moved the file knows it moved, and a fact known exactly is
    /// worth more than a similarity score guessing at it afterwards.
    ///
    /// What it buys: `blame` follows the file across the rename instead of
    /// attributing every line to the snapshot that moved it, and a path filter
    /// on `history` finds the snapshots from before the move.
    ///
    /// `from` must exist in the parent and `to` in this snapshot's `entries`;
    /// `fsck` reports edges that fail either. Empty is the normal case, and is
    /// what every snapshot written before this existed has.
    pub renames: &'a [(PathBuf, PathBuf)],
    /// When this snapshot was made, as epoch milliseconds, or `None` for now.
    ///
    /// The timestamp is part of a snapshot's identity, so a caller that cannot
    /// supply it cannot control the id. Two things need that:
    ///
    /// - **Reproducible tests.** With the clock read internally, an id changes
    ///   every millisecond, so no test can assert one against a constant — which
    ///   rules out exactly the golden-file testing a storage format most wants.
    /// - **Importing history.** Bringing a repository over from another system,
    ///   or restoring a backup, has to preserve the original dates; otherwise
    ///   every snapshot is stamped with the moment of the import.
    ///
    /// It is also the last ambient dependency in a crate that otherwise
    /// [reads no process environment](crate) — and the one baked into
    /// content-addressed identity.
    ///
    /// # Supplying an out-of-order timestamp
    ///
    /// A branch's tip is *derived*: the newest snapshot carrying that branch
    /// name, by `created_at_ms`, breaking ties on insertion order. So a snapshot
    /// saved with a timestamp **older than the branch's current tip does not
    /// become the tip**. That is correct for an importer replaying history in
    /// order, and surprising for anything else. Values are not validated —
    /// negative ones are legitimately pre-1970, which an importer may need.
    pub timestamp_ms: Option<i64>,
}

/// One change to apply to a parent snapshot's tree, for
/// [`WriteGuard::save_changes`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Change {
    /// Add or replace the file at `entry.path`.
    ///
    /// Bytes are CRLF-normalised exactly as `save_tree` does; a stored object
    /// must already exist.
    Put(TreeEntry),
    /// Remove a path present in the tree being built.
    Delete(String),
    /// Move the parent's file at `from` to `to`, keeping its object and kind,
    /// and record the rename edge so `blame` and `history` follow it.
    Rename { from: String, to: String },
}

/// A snapshot to create from a parent plus changes.
///
/// The parent's tree is loaded inside the database, so a caller that changes
/// one file in a large tree does not send the rest across the API.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SaveChanges<'a> {
    /// Branch to record the snapshot on.
    pub branch: &'a BranchName,
    /// The snapshot to start from, or `None` for an empty tree and a root
    /// snapshot.
    pub parent: Option<&'a SnapshotId>,
    /// The second parent, when this snapshot is a merge. Its tree is not read.
    pub merge_parent: Option<&'a SnapshotId>,
    pub message: &'a str,
    /// Applied in order to the parent's tree.
    pub changes: Vec<Change>,
    /// App-namespaced metadata, part of the snapshot's identity.
    pub meta: SnapshotMeta,
    /// Who made this snapshot.
    pub author: Option<&'a Author>,
    /// When this snapshot was made, as epoch milliseconds, or `None` for now.
    pub timestamp_ms: Option<i64>,
}

impl<'a> SaveChanges<'a> {
    /// A save of `parent` with no changes yet.
    pub fn new(branch: &'a BranchName, parent: Option<&'a SnapshotId>, message: &'a str) -> Self {
        SaveChanges {
            branch,
            parent,
            merge_parent: None,
            message,
            changes: Vec::new(),
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: None,
        }
    }

    /// Add or replace a file.
    pub fn put(mut self, entry: TreeEntry) -> Self {
        self.changes.push(Change::Put(entry));
        self
    }

    /// Remove a path.
    pub fn delete(mut self, path: impl Into<String>) -> Self {
        self.changes.push(Change::Delete(path.into()));
        self
    }

    /// Move a file and record the rename.
    pub fn rename(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.changes.push(Change::Rename {
            from: from.into(),
            to: to.into(),
        });
        self
    }

    /// Record a second parent.
    pub fn merge_parent(mut self, id: &'a SnapshotId) -> Self {
        self.merge_parent = Some(id);
        self
    }

    /// Attach metadata.
    pub fn meta(mut self, meta: SnapshotMeta) -> Self {
        self.meta = meta;
        self
    }

    /// Record an author.
    pub fn author(mut self, a: &'a Author) -> Self {
        self.author = Some(a);
        self
    }

    /// Fix the timestamp, as epoch milliseconds.
    pub fn timestamp_ms(mut self, ms: i64) -> Self {
        self.timestamp_ms = Some(ms);
        self
    }
}

/// The result of [`WriteGuard::save_changes`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Saved {
    pub snapshot: SnapshotId,
    /// The object of every file the changes put and that is still in the final
    /// tree, sorted by path. Files carried over from the parent are not listed.
    pub written: Vec<(String, ObjectHash)>,
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
    ///
    /// # There is no "nothing changed" guard
    ///
    /// `velo save` refuses an empty save. This does not: hand it the same tree
    /// twice and you get two snapshots, differing only in parent and timestamp.
    /// That is right for a primitive — a caller may well want to record that
    /// something was checked at a particular moment — but it means the failure
    /// mode is a history full of duplicates rather than an error, and it is on
    /// the caller to notice.
    ///
    /// Comparing against the parent's tree is the check most consumers want:
    ///
    /// ```no_run
    /// # fn main() -> Result<(), velo_core::Error> {
    /// # use velo_core::tree::TreeFile;
    /// # let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
    /// # let parent = velo_core::commands::resolve_snapshot_id(&repo, "main")?;
    /// # let proposed: Vec<TreeFile> = Vec::new();
    /// let unchanged = repo.tree_at(&parent)? == proposed;
    /// # let _ = unchanged;
    /// # Ok(()) }
    /// ```
    pub fn save_tree(&self, spec: SaveTree<'_>) -> Result<SnapshotId> {
        crate::timing::begin();
        // No empty-branch check: `BranchName` cannot be empty by construction.
        // Both parents are checked: naming one that does not exist would create
        // dangling history that only `fsck` would find later.
        for parent in [spec.parent, spec.merge_parent].into_iter().flatten() {
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

        let objects = self.repo().objects();
        let mut tree: Vec<(String, String, i64)> = Vec::with_capacity(spec.entries.len());
        let mut seen = std::collections::HashSet::new();

        // The whole path rule is applied before any object is written: every
        // path, the file/directory exclusivity of the set, and both ends of
        // every recorded rename. A refusal creates nothing.
        let normalised: Vec<String> = spec
            .entries
            .iter()
            .map(|e| db::normalise(&e.path))
            .collect();
        crate::safe_path::check_rows(normalised.iter().map(String::as_str))?;
        for (from, to) in spec.renames {
            crate::safe_path::check_named(&db::normalise(&from.to_string_lossy()))?;
            crate::safe_path::check_named(&db::normalise(&to.to_string_lossy()))?;
        }
        crate::timing::lap("save_tree.validate");

        // Which `Stored` objects need proving present. The first parent's tree
        // is read once (and handed to `write_tree` below); an object it names
        // is trusted, so only the rest are asked of the store.
        let has_stored = spec
            .entries
            .iter()
            .any(|e| matches!(e.content, Content::Stored(_)));
        let parent_tree = match spec.parent {
            Some(parent) if has_stored => {
                let t = crate::tree_store::tree_of(self.conn(), parent.as_str())?;
                crate::timing::lap("save_tree.load_parent");
                Some(t)
            }
            _ => None,
        };
        if has_stored {
            let known: std::collections::HashSet<&str> = parent_tree
                .iter()
                .flat_map(|t| t.values().map(|(h, _)| h.as_str()))
                .collect();
            let mut queued = std::collections::HashSet::new();
            let unseen: Vec<&str> = spec
                .entries
                .iter()
                .filter_map(|e| match &e.content {
                    Content::Stored(o) => Some(o.as_str()),
                    _ => None,
                })
                .filter(|o| !known.contains(o) && queued.insert(*o))
                .collect();
            crate::timing::count("save_tree.checked", unseen.len() as u64);
            if !unseen.is_empty() {
                let missing = objects.missing(&unseen)?;
                if let Some(first) = missing.into_iter().next() {
                    return Err(VeloError::MissingObject { hash: first });
                }
            }
        }

        for (entry, path) in spec.entries.into_iter().zip(normalised) {
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
                    objects.put(&content)?
                }
                Content::Stored(object) => {
                    // Already-stored content is already normalised, so it is
                    // taken as-is. It must actually be there, or the snapshot
                    // would manufacture corruption through the public API; that
                    // was proved above, before anything was written, for every
                    // object the first parent does not reference. An object the
                    // parent does reference is trusted: an intact repository
                    // holds it, and one that does not is `fsck`'s to report.
                    object.into_string()
                }
            };
            tree.push((path, object, mode));
        }
        crate::timing::lap("save_tree.objects");

        let renames: Vec<(String, String)> = spec
            .renames
            .iter()
            .map(|(from, to)| {
                (
                    db::normalise(&from.to_string_lossy()),
                    db::normalise(&to.to_string_lossy()),
                )
            })
            .collect();
        self.commit_tree(
            tree,
            &renames,
            Identity {
                branch: spec.branch,
                parent: spec.parent,
                merge_parent: spec.merge_parent,
                message: spec.message,
                meta: spec.meta,
                author: spec.author,
                timestamp_ms: spec.timestamp_ms,
            },
            parent_tree.as_ref(),
        )
    }
}

/// A tree entry while changes are being applied: carried over from the parent
/// (already stored) or a put not yet written.
enum Slot {
    Parent(String, i64),
    Put(TreeEntry),
}

impl WriteGuard<'_> {
    /// Create a snapshot from a parent plus a list of changes.
    ///
    /// Returns the snapshot id and the object id of every file written. The
    /// parent's tree is read inside the database, so only the changes cross the
    /// API. The result is still a whole-tree snapshot: the id is the one
    /// [`save_tree`](WriteGuard::save_tree) gives the same resulting tree.
    ///
    /// Changes apply in order. `Delete` of an absent path, `Rename` of a path
    /// not in the parent's tree (or already moved or deleted by an earlier
    /// change), and `Rename` onto an existing path are invalid-input errors. A
    /// `Put` after a `Rename` to the same path replaces the content, which is
    /// how an edit-and-move is expressed. The whole path rule runs before any
    /// object is written, so a refusal writes nothing.
    ///
    /// # There is no "nothing changed" guard
    ///
    /// An empty `changes` records the parent's tree again, as a new snapshot
    /// differing only in parent and timestamp. A caller that wants to skip that
    /// compares the trees with [`Repo::tree_at`]; see
    /// [`save_tree`](WriteGuard::save_tree) for the reasoning.
    pub fn save_changes(&self, spec: SaveChanges<'_>) -> Result<Saved> {
        crate::timing::begin();
        for parent in [spec.parent, spec.merge_parent].into_iter().flatten() {
            let known: bool = self
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                    [parent],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !known {
                return Err(self.repo().missing_snapshot(parent));
            }
        }

        let mut working: std::collections::BTreeMap<String, Slot> =
            std::collections::BTreeMap::new();
        if let Some(parent) = spec.parent {
            for (path, (hash, mode)) in crate::tree_store::tree_of(self.conn(), parent)? {
                working.insert(path, Slot::Parent(hash, mode));
            }
        }
        let in_parent: std::collections::HashSet<String> = working.keys().cloned().collect();
        crate::timing::lap("save_changes.load_parent");

        let mut renames: Vec<(String, String)> = Vec::new();
        // Every path that was a rename source or was deleted in this call. A
        // later Put may reuse the name, but the parent's file is gone, so it
        // cannot be a rename source again.
        let mut consumed: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Deleting the target of a recorded rename is refused (rather than
        // dropping the edge): the edge would be false once the path holds
        // something else, and a second rename onto it would collide on the
        // `renames` primary key. This runs before any object is written.
        let mut rename_targets: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for change in spec.changes {
            match change {
                Change::Put(mut entry) => {
                    let path = db::normalise(&entry.path);
                    entry.path = path.clone();
                    working.insert(path, Slot::Put(entry));
                }
                Change::Delete(path) => {
                    let path = db::normalise(&path);
                    if rename_targets.contains(&path) {
                        return Err(VeloError::invalid(format!(
                            "cannot delete '{path}': it is the target of a rename in this call."
                        )));
                    }
                    if working.remove(&path).is_none() {
                        return Err(VeloError::invalid(format!(
                            "cannot delete '{path}': it is not in the tree."
                        )));
                    }
                    consumed.insert(path);
                }
                Change::Rename { from, to } => {
                    let from = db::normalise(&from);
                    let to = db::normalise(&to);
                    if !in_parent.contains(&from) || consumed.contains(&from) {
                        return Err(VeloError::invalid(format!(
                            "cannot rename '{from}': it is not in the parent's tree, or was already moved or deleted."
                        )));
                    }
                    if working.contains_key(&to) || rename_targets.contains(&to) {
                        return Err(VeloError::invalid(format!(
                            "cannot rename onto '{to}': it already exists."
                        )));
                    }
                    let Some(slot) = working.remove(&from) else {
                        return Err(VeloError::invalid(format!(
                            "cannot rename '{from}': it was already moved or deleted."
                        )));
                    };
                    working.insert(to.clone(), slot);
                    consumed.insert(from.clone());
                    rename_targets.insert(to.clone());
                    renames.push((from, to));
                }
            }
        }

        // The path rule is applied to the final tree and both ends of every
        // rename before any object is written: a refusal creates nothing.
        crate::safe_path::check_rows(working.keys().map(String::as_str))?;
        for (from, to) in &renames {
            crate::safe_path::check_named(from)?;
            crate::safe_path::check_named(to)?;
        }
        crate::timing::lap("save_changes.apply");

        let objects = self.repo().objects();
        let mut tree: Vec<(String, String, i64)> = Vec::with_capacity(working.len());
        let mut written: Vec<(String, ObjectHash)> = Vec::new();
        for (path, slot) in working {
            match slot {
                Slot::Parent(hash, mode) => tree.push((path, hash, mode)),
                Slot::Put(entry) => {
                    let mode = entry.kind.mode();
                    let object = match entry.content {
                        // As in `save_tree`: text is CRLF-normalised so memory
                        // and filesystem saves agree on the object.
                        Content::Bytes(bytes) => {
                            let content = if mode == storage::MODE_SYMLINK {
                                bytes
                            } else {
                                storage::normalise_crlf(bytes)
                            };
                            objects.put(&content)?
                        }
                        Content::Stored(object) => {
                            if !objects.contains(object.as_str()) {
                                return Err(VeloError::MissingObject {
                                    hash: object.into_string(),
                                });
                            }
                            object.into_string()
                        }
                    };
                    written.push((path.clone(), ObjectHash::from_stored(object.clone())));
                    tree.push((path, object, mode));
                }
            }
        }
        // `working` was a BTreeMap, so both lists are already in path order.

        let snapshot = self.commit_tree(
            tree,
            &renames,
            Identity {
                branch: spec.branch,
                parent: spec.parent,
                merge_parent: spec.merge_parent,
                message: spec.message,
                meta: spec.meta,
                author: spec.author,
                timestamp_ms: spec.timestamp_ms,
            },
            None,
        )?;
        Ok(Saved { snapshot, written })
    }
}

/// Everything that gives a snapshot its identity apart from its tree.
struct Identity<'a> {
    branch: &'a BranchName,
    parent: Option<&'a SnapshotId>,
    merge_parent: Option<&'a SnapshotId>,
    message: &'a str,
    meta: SnapshotMeta,
    author: Option<&'a Author>,
    timestamp_ms: Option<i64>,
}

impl WriteGuard<'_> {
    /// Insert a validated whole tree as a snapshot and commit it.
    ///
    /// Shared by `save_tree` and `save_changes` so both give the same id and the
    /// same rows for the same tree. The caller has already checked the parents,
    /// applied the path rule and stored the objects; `renames` are normalised.
    fn commit_tree(
        &self,
        tree: Vec<(String, String, i64)>,
        renames: &[(String, String)],
        id: Identity<'_>,
        parent_tree: Option<&crate::tree_store::StoredTree>,
    ) -> Result<SnapshotId> {
        // The author joins the metadata *before* the id is computed, because
        // that is the point: it is part of identity, not a column beside it.
        let mut meta = id.meta;
        if let Some(author) = id.author {
            meta.set_author(author);
        }

        let parent = id.parent.map_or("", |p| p.as_str());
        let merge_parent = id.merge_parent.map_or("", |p| p.as_str());
        // The caller's clock when they supplied one, ours otherwise. This is the
        // only place the wall clock is read.
        let timestamp_ms = id.timestamp_ms.unwrap_or_else(|| self.repo().now_ms());
        let snapshot = SnapshotId::from_stored(crate::commands::snapshot_id(SnapshotIdentity {
            tree: &tree,
            parent,
            merge_parent,
            message: id.message,
            timestamp_ms,
            meta: &meta,
        }));
        crate::timing::lap("save_tree.snapshot_id");

        let tx = self.transaction()?;

        // The id may already exist, and that is not an error.
        //
        // Identity covers the tree, parents, message, metadata and timestamp —
        // but deliberately *not* the branch. So saving the same content, with the
        // same message and parent, twice inside one millisecond produces one id,
        // whether that is a retry or the same tree recorded on a second branch.
        // Content addressing says those are the same snapshot; re-inserting the
        // rows would fail on the primary key and surface a raw SQLite constraint
        // violation for what is a legitimate call.
        //
        // The rows are identical by construction, so the existing ones are left
        // alone. The branch is still registered below, which is what the caller
        // actually asked for.
        let already: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                [&snapshot],
                |r| r.get(0),
            )
            .unwrap_or(false);

        if !already {
            tx.execute(
                "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent, created_at_ms)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    snapshot,
                    id.message,
                    id.branch,
                    parent,
                    merge_parent,
                    timestamp_ms
                ],
            )?;
            {
                // Metadata is hashed into the id above, so it has to be stored in
                // the same transaction: an id that commits to metadata the
                // repository does not hold would fail its own `fsck`.
                let mut ins_meta = tx.prepare(
                    "INSERT INTO snapshot_meta (snapshot_id, namespace, key, value)
                     VALUES (?, ?, ?, ?)",
                )?;
                for (namespace, key, value) in meta.iter() {
                    ins_meta.execute(params![snapshot, namespace, key, value])?;
                }
            }
            {
                let rows: Vec<(&str, &str, i64)> = tree
                    .iter()
                    .map(|(p, o, m)| (p.as_str(), o.as_str(), *m))
                    .collect();
                crate::tree_store::write_tree(
                    &tx,
                    snapshot.as_str(),
                    crate::tree_store::first_parent(parent),
                    parent_tree,
                    &rows,
                    self.repo().keyframe_interval(),
                )?;
            }
            crate::timing::lap("save_tree.insert_file_map");
            // Inside `if !already`, with everything else the snapshot owns.
            // Rename edges are not part of the id, so an identical tree saved a
            // second time is the same snapshot — and the edges recorded the
            // first time are the ones that stand. Letting the second call
            // overwrite them would make the id stop describing what is stored.
            let mut ins_rename = tx.prepare(
                "INSERT INTO renames (snapshot_hash, from_path, to_path) VALUES (?, ?, ?)",
            )?;
            let present: std::collections::HashSet<&str> = if renames.is_empty() {
                std::collections::HashSet::new()
            } else {
                tree.iter().map(|(p, _, _)| p.as_str()).collect()
            };
            for (from, to) in renames {
                if from.is_empty() || to.is_empty() {
                    return Err(VeloError::invalid("a rename edge needs both paths."));
                }
                if from == to {
                    return Err(VeloError::invalid(format!(
                        "'{}' is recorded as renamed to itself.",
                        from
                    )));
                }
                // Checked here rather than left to `fsck`: an edge naming a path
                // this snapshot does not contain is wrong at the moment it is
                // written, and reporting it later means the history already has
                // it. The other half — that `from` existed in the parent — is
                // `fsck`'s, because a caller may legitimately record a rename
                // onto a parent it did not read.
                if !present.contains(to.as_str()) {
                    return Err(VeloError::invalid(format!(
                        "renamed to '{}', which is not in this tree.",
                        to
                    )));
                }
                ins_rename.execute(params![snapshot, from, to])?;
            }
        }
        // Through `tx`, not the connection: an unchecked transaction begins on the
        // shared connection, so writing via `self.conn()` here would land inside
        // this transaction anyway — but only by accident of how it is
        // implemented. Being explicit says the branch row commits with the rest.
        crate::commands::register_branch(&tx, id.branch, &snapshot)?;
        crate::timing::lap("save_tree.insert_rest");
        tx.commit()?;
        crate::timing::lap("save_tree.commit");

        // After the commit, and only for a row this call inserted: an idempotent
        // re-save changed nothing, so there is nothing to announce.
        if !already {
            self.repo()
                .emit_saved(&snapshot, id.branch, id.parent, id.merge_parent);
        }

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
            return Err(self.missing_snapshot(snapshot));
        }

        let tree = crate::tree_store::tree_of(self.conn(), snapshot)?;
        crate::tree_store::check_path_limits(&tree, self.limits().max_string_bytes)?;
        let files: Vec<TreeFile> = tree
            .into_iter()
            .map(|(path, (object, mode))| TreeFile {
                path,
                object: ObjectHash::from_stored(object),
                kind: FileKind::from_mode(mode),
            })
            .collect();
        Ok(files)
    }

    /// The object id of `path` in `snapshot`, or `None` when the path is not
    /// in that tree. One indexed lookup; the content is not read.
    pub fn object_at(
        &self,
        snapshot: &SnapshotId,
        path: impl AsRef<Path>,
    ) -> Result<Option<ObjectHash>> {
        let rel = db::normalise(&path.as_ref().to_string_lossy());
        Ok(
            crate::tree_store::file_at(self.conn(), snapshot.as_ref(), &rel)?
                .map(|(hash, _)| ObjectHash::from_stored(hash)),
        )
    }

    /// The content of `path` as it was in `snapshot`.
    ///
    /// For a symlink this is the target path, matching how it was stored.
    pub fn read_file_at(&self, snapshot: &SnapshotId, path: impl AsRef<Path>) -> Result<Vec<u8>> {
        let rel = db::normalise(&path.as_ref().to_string_lossy());
        let object: Option<ObjectHash> =
            crate::tree_store::file_at(self.conn(), snapshot.as_ref(), &rel)?
                .map(|(hash, _)| ObjectHash::from_stored(hash));
        match object {
            Some(object) => self.read_object(&object),
            None => {
                // A compacted snapshot has no rows left, so say where it went
                // rather than blaming the path.
                if let Some(into) = crate::commands::compacted_into(self.conn(), snapshot) {
                    let live: bool = self
                        .conn()
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                            [snapshot],
                            |r| r.get(0),
                        )
                        .unwrap_or(false);
                    if !live {
                        return Err(VeloError::Compacted {
                            id: snapshot.to_string(),
                            into,
                        });
                    }
                }
                Err(VeloError::invalid(format!(
                    "'{}' is not in snapshot {}.",
                    rel,
                    snapshot.short()
                )))
            }
        }
    }

    /// The content of every path in `paths` as it was in `snapshot`, in the
    /// order asked, as `(normalised path, bytes)` pairs.
    ///
    /// The hashes come from one `file_map` statement per 500 paths rather than
    /// one per file, and the objects are read in parallel through the same
    /// `ObjectStore::get` as [`Repo::read_file_at`], so the hash check and
    /// every object limit apply unchanged. A path asked for twice is returned
    /// twice. The first path that is not in the snapshot (in input order) is
    /// an error, as is the first unreadable object.
    pub fn read_files_at<P: AsRef<Path>>(
        &self,
        snapshot: &SnapshotId,
        paths: &[P],
    ) -> Result<Vec<(String, Vec<u8>)>> {
        let known: bool = self
            .conn()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                [snapshot],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !known {
            return Err(self.missing_snapshot(snapshot));
        }
        if paths.is_empty() {
            return Ok(Vec::new());
        }

        let wanted: Vec<String> = paths
            .iter()
            .map(|p| db::normalise(&p.as_ref().to_string_lossy()))
            .collect();
        let mut unique: Vec<&str> = wanted.iter().map(String::as_str).collect();
        unique.sort_unstable();
        unique.dedup();

        // One placeholder is the snapshot, so 499 paths plus it make the 500
        // allowed per statement.
        let mut hashes: std::collections::HashMap<String, ObjectHash> =
            std::collections::HashMap::with_capacity(unique.len());
        for (path, (hash, _)) in
            crate::tree_store::files_at(self.conn(), snapshot.as_ref(), &unique)?
        {
            hashes.insert(path, ObjectHash::from_stored(hash));
        }

        let mut items: Vec<(String, ObjectHash)> = Vec::with_capacity(wanted.len());
        for path in wanted {
            match hashes.get(&path) {
                Some(hash) => items.push((path, hash.clone())),
                None => {
                    return Err(VeloError::invalid(format!(
                        "'{}' is not in snapshot {}.",
                        path,
                        snapshot.short()
                    )))
                }
            }
        }

        let read = self.objects().par_with_content(
            &items,
            |(_, hash)| hash.as_str(),
            |(path, _), content| Some(content.map(|bytes| (path.clone(), bytes))),
        );
        read.into_iter().collect()
    }

    /// The error for a snapshot id with no row: `Compacted` when compaction
    /// removed it, `NotFound` otherwise.
    fn missing_snapshot(&self, snapshot: &SnapshotId) -> VeloError {
        match crate::commands::compacted_into(self.conn(), snapshot) {
            Some(into) => VeloError::Compacted {
                id: snapshot.to_string(),
                into,
            },
            None => VeloError::not_found(RefKind::Snapshot, snapshot.as_str()),
        }
    }

    /// The bytes of a stored object, by its content hash.
    ///
    /// The hash is verified on the way out, so a corrupted store is an error
    /// rather than silently wrong content.
    pub fn read_object(&self, object: &ObjectHash) -> Result<Vec<u8>> {
        self.objects().get(object)
    }

    /// The metadata attached to `snapshot`.
    ///
    /// Empty when none was attached — which is not distinguishable from "the
    /// snapshot has no metadata", because to the id recipe those are the same
    /// thing. A snapshot that compaction removed is the exception, and reports
    /// [`Error::Compacted`](crate::Error::Compacted); any other unknown id stays
    /// empty.
    pub fn snapshot_meta(&self, snapshot: &SnapshotId) -> Result<SnapshotMeta> {
        let meta = crate::commands::load_snapshot_meta(self.conn(), snapshot)?;
        if meta.iter().next().is_none() {
            let live: bool = self
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
                    [snapshot],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            if !live {
                if let Some(into) = crate::commands::compacted_into(self.conn(), snapshot) {
                    return Err(VeloError::Compacted {
                        id: snapshot.to_string(),
                        into,
                    });
                }
            }
        }
        Ok(meta)
    }
}
