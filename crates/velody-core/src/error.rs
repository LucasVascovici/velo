//! Typed errors.
//!
//! Consumers branch on outcomes; they must never have to string-match a message.
//! Every state a caller might reasonably want to handle — divergence, conflicts,
//! a dirty tree, a rejected push, a lock someone else holds — is its own variant
//! carrying the data needed to act on it.
//!
//! The enum is `#[non_exhaustive]`, so adding variants later is not a breaking
//! change for downstream crates. Match with a trailing `_` arm.

use std::fmt;
use std::path::PathBuf;

/// What kind of reference failed to resolve, for [`Error::NotFound`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefKind {
    Snapshot,
    Branch,
    Tag,
    Remote,
    RemoteBranch,
    Stash,
    Path,
    /// A ref given by a user that could be several of the above.
    Any,
}

impl fmt::Display for RefKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RefKind::Snapshot => "snapshot",
            RefKind::Branch => "branch",
            RefKind::Tag => "tag",
            RefKind::Remote => "remote",
            RefKind::RemoteBranch => "remote branch",
            RefKind::Stash => "stash shelf",
            RefKind::Path => "path",
            RefKind::Any => "snapshot, tag or branch",
        })
    }
}

/// An operation that can be in progress and block another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
}

impl fmt::Display for InProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InProgress::Merge => "merge",
            InProgress::Rebase => "rebase",
            InProgress::CherryPick => "cherry-pick",
        })
    }
}

/// Everything `velody-core` can fail with.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    // ── Repository discovery / lifecycle ─────────────────────────────────────
    /// No `.velo` directory here or in any parent.
    NotARepo {
        searched_from: PathBuf,
    },
    /// `init` called where a repository already exists.
    AlreadyInitialized {
        at: PathBuf,
    },
    /// `init` called inside an existing repository.
    NestedRepo {
        outer: PathBuf,
    },
    /// Written by a newer Velody than this one understands. Refusing to open is
    /// deliberate: proceeding risks a half-migration.
    SchemaTooNew {
        found: u32,
        supported: u32,
    },
    /// Needs migrating first; call `open_and_migrate` when ready.
    MigrationRequired {
        found: u32,
        supported: u32,
    },
    /// Written in a format too old to upgrade in place.
    ///
    /// Distinct from [`Error::MigrationRequired`] because there is nothing the
    /// caller can call to fix it: format v2 changed every snapshot id, so a v1
    /// repository is not v2 rows with a stale marker — recovering it means
    /// re-creating the history in a fresh repository.
    FormatTooOld {
        found: u32,
        supported: u32,
    },

    /// The caller asked the operation to stop, via
    /// [`Cancel`](crate::progress::Cancel).
    ///
    /// Database work is rolled back; anything already written to the working tree
    /// stays written, which is the same position a killed process would leave.
    Cancelled,

    // ── Concurrency ──────────────────────────────────────────────────────────
    /// Another process holds the repository write lock.
    Locked {
        /// Process id of the holder, when its record could be read.
        held_by: Option<u32>,
        /// Everything recorded about the holder (pid, start time, command).
        holder: Option<crate::lock::LockHolder>,
    },

    // ── Working-tree preconditions ───────────────────────────────────────────
    /// The operation would overwrite unsaved changes to these tracked files.
    DirtyWorkingTree {
        paths: Vec<PathBuf>,
    },
    /// Taking a side would replace edits made by hand to these conflicted files.
    WouldOverwriteEdits {
        paths: Vec<PathBuf>,
    },
    /// A hand edit accepted as a resolution would drop lines the other side changed cleanly.
    MissingCleanChanges {
        path: PathBuf,
        missing: usize,
        /// At most the first five missing lines.
        sample: Vec<String>,
    },
    /// Undoing this snapshot would leave other snapshots without their parent.
    SnapshotHasDependents {
        snapshot: crate::SnapshotId,
        /// Branch label of each live descendant, sorted by name, with how many.
        branches: Vec<(crate::BranchName, usize)>,
        /// Total live descendants.
        dependents: usize,
    },
    /// An operation is already underway; finish or abort it first.
    OperationInProgress {
        what: InProgress,
    },
    /// No such operation is underway (e.g. `merge --abort` with no merge).
    NoOperationInProgress {
        what: InProgress,
    },

    // ── Merge / sync outcomes ────────────────────────────────────────────────
    /// A merge or apply produced conflicts needing resolution.
    Conflicts {
        paths: Vec<PathBuf>,
    },
    /// Local and remote history have both advanced; the caller must reconcile.
    Diverged {
        branch: String,
        ahead: usize,
        behind: usize,
    },
    /// A push would discard commits the remote has and we don't.
    NotFastForward {
        branch: String,
        remote: String,
    },
    /// A remote-tracking ref names a snapshot this repository does not hold
    /// (for example one that was undone here after a push). The ref is still
    /// right about the remote; a `fetch` brings the snapshot back. Merging it
    /// would otherwise record a parent that does not exist.
    RemoteRefAbsent {
        remote: String,
        branch: String,
        snapshot: crate::SnapshotId,
    },
    /// `redo` would bring back a snapshot whose parent is no longer the branch
    /// tip, because the branch moved (a pull, merge or fetch fast-forward)
    /// after the undo. `snapshot` is the trashed one, `tip` where the branch is
    /// now. The snapshot stays in the trash; `restore` can still show it.
    RedoBranchMoved {
        branch: String,
        snapshot: crate::SnapshotId,
        tip: Option<crate::SnapshotId>,
    },
    /// `undo` found no snapshot on the branch to remove. Nothing is wrong;
    /// the CLI reports it as plain text and exits 0.
    NothingToUndo {
        branch: String,
    },
    /// `redo` found nothing in the trash for the branch. Like
    /// [`Error::NothingToUndo`] this is a nothing-to-do, not a failure.
    NothingToRedo {
        branch: String,
    },
    /// The branch has no commits yet and this operation needs at least one.
    UnbornBranch {
        branch: String,
    },

    // ── Lookup ───────────────────────────────────────────────────────────────
    /// A reference did not resolve.
    NotFound {
        kind: RefKind,
        name: String,
    },
    /// A hash prefix matched more than one snapshot — never silently pick one.
    AmbiguousPrefix {
        prefix: String,
        matches: usize,
    },

    /// The snapshot no longer exists because compaction squashed or re-minted
    /// it. `into` is the live id that replaced it, so a holder of the old id can
    /// carry on rather than treat it as missing.
    Compacted {
        id: String,
        into: crate::SnapshotId,
    },

    // ── Integrity ────────────────────────────────────────────────────────────
    /// Stored data failed verification.
    Corrupt {
        detail: String,
    },
    /// A referenced object is absent from the object store.
    MissingObject {
        hash: String,
    },
    /// Received data (bundle or sync) failed verification before import.
    UntrustedData {
        detail: String,
    },

    // ── Caller error ─────────────────────────────────────────────────────────
    /// A path in a tree, bundle or git stream is one velody will not store or
    /// write to a working tree: it escapes the root, names `.velo`, is reserved
    /// on this platform, or collides with a directory. Nothing was written.
    UnsafePath {
        path: String,
        problem: crate::safe_path::PathProblem,
    },
    /// Received data would need more memory than a configured ceiling allows.
    /// `found` is `None` when the overrun was noticed while streaming. Nothing
    /// was imported. See [`crate::Limits`].
    LimitExceeded {
        limit: crate::limits::Limit,
        max: u64,
        found: Option<u64>,
    },
    /// A remote refused the request for lack of (valid) credentials or
    /// permission. `detail` is the server's explanation and never contains a
    /// token.
    AccessDenied {
        remote: String,
        detail: String,
    },
    /// The request itself was invalid (empty message, bad range, …). Prefer a
    /// specific variant; this is the residual case.
    InvalidInput {
        detail: String,
    },
    /// Not supported in this build or on this platform.
    Unsupported {
        detail: String,
    },

    // ── Plumbing ─────────────────────────────────────────────────────────────
    Io(std::io::Error),
    Db(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotARepo { .. } => {
                write!(f, "not a Velody repository (no .velo found here or above)")
            }
            Error::AlreadyInitialized { at } => {
                write!(f, "a Velody repository already exists at '{}'", at.display())
            }
            Error::NestedRepo { outer } => write!(
                f,
                "already inside a Velody repository at '{}'; nested repositories are not supported",
                outer.display()
            ),
            Error::SchemaTooNew { found, supported } => write!(
                f,
                "repository format v{} is newer than this Velody supports (v{})",
                found, supported
            ),
            Error::MigrationRequired { found, supported } => write!(
                f,
                "repository format v{} must be migrated to v{} before use",
                found, supported
            ),
            Error::FormatTooOld { found, supported } => write!(
                f,
                "repository format v{} predates v{} and cannot be upgraded in place: \
                 v2 changed every snapshot id",
                found, supported
            ),
            Error::Cancelled => f.write_str("the operation was cancelled"),
            Error::Locked {
                holder: Some(h), ..
            } => match &h.command {
                Some(cmd) => write!(
                    f,
                    "another Velody operation holds the lock (pid {}, '{}')",
                    h.pid, cmd
                ),
                None => write!(f, "another Velody operation holds the lock (pid {})", h.pid),
            },
            Error::Locked { held_by, .. } => match held_by {
                Some(pid) => write!(f, "repository is locked by process {}", pid),
                None => write!(
                    f,
                    "another Velody operation is in progress in this repository"
                ),
            },
            Error::DirtyWorkingTree { paths } => {
                write!(f, "{} unsaved change(s) would be overwritten", paths.len())
            }
            Error::SnapshotHasDependents {
                snapshot,
                branches,
                dependents,
            } => write!(
                f,
                "snapshot {} is built on by {} snapshot(s) on {}; undoing it would leave them without a parent",
                snapshot.short(),
                dependents,
                branches
                    .iter()
                    .map(|(b, _)| b.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Error::WouldOverwriteEdits { paths } => write!(
                f,
                "{} conflicted file(s) were edited by hand; taking a side would replace those edits",
                paths.len()
            ),
            Error::MissingCleanChanges { path, missing, .. } => write!(
                f,
                "'{}' is missing {} line(s) the other side changed outside the conflict",
                path.display(),
                missing
            ),
            Error::OperationInProgress { what } => write!(f, "a {} is already in progress", what),
            Error::NoOperationInProgress { what } => write!(f, "no {} in progress", what),
            Error::Conflicts { paths } => {
                write!(f, "{} file(s) have conflicts to resolve", paths.len())
            }
            Error::Diverged {
                branch,
                ahead,
                behind,
            } => write!(
                f,
                "'{}' has diverged: {} ahead, {} behind",
                branch, ahead, behind
            ),
            Error::NotFastForward { branch, remote } => write!(
                f,
                "'{}' on '{}' has commits you don't have (non-fast-forward)",
                branch, remote
            ),
            Error::RemoteRefAbsent {
                remote,
                branch,
                snapshot,
            } => write!(
                f,
                "'{}/{}' points at snapshot {} which is not in this repository",
                remote,
                branch,
                snapshot.short()
            ),
            Error::NothingToUndo { branch } => write!(f, "Nothing to undo on branch '{branch}'."),
            Error::NothingToRedo { branch } => write!(f, "Nothing to redo on branch '{branch}'."),
            Error::RedoBranchMoved {
                branch,
                snapshot,
                tip,
            } => match tip {
                Some(tip) => write!(
                    f,
                    "cannot redo {}: branch '{}' moved to {} since it was undone",
                    snapshot.short(),
                    branch,
                    tip.short()
                ),
                None => write!(
                    f,
                    "cannot redo {}: branch '{}' moved since it was undone",
                    snapshot.short(),
                    branch
                ),
            },
            Error::UnbornBranch { branch } => write!(f, "branch '{}' has no commits yet", branch),
            Error::NotFound { kind, name } => write!(f, "no {} found matching '{}'", kind, name),
            Error::AmbiguousPrefix { prefix, matches } => write!(
                f,
                "prefix '{}' matches {} snapshots; use more characters",
                prefix, matches
            ),
            Error::Compacted { id, into } => {
                write!(f, "snapshot {} was compacted into {}", id, into)
            }
            Error::Corrupt { detail } => write!(f, "corrupt repository: {}", detail),
            Error::MissingObject { hash } => {
                write!(f, "object {} is missing from the object store", hash)
            }
            Error::UntrustedData { detail } => {
                write!(f, "received data failed verification: {}", detail)
            }
            Error::UnsafePath { path, problem } => {
                write!(f, "refusing path '{path}': {problem}")
            }
            Error::LimitExceeded { limit, max, found } => match found {
                Some(n) => write!(f, "{limit} of {n} exceeds the limit of {max}"),
                None => write!(f, "{limit} exceeds the limit of {max}"),
            },
            Error::AccessDenied { remote, detail } => {
                write!(f, "'{}' refused access: {}", remote, detail)
            }
            Error::InvalidInput { detail } => f.write_str(detail),
            Error::Unsupported { detail } => f.write_str(detail),
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::Db(e) => write!(f, "database error: {}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e)
    }
}

impl Error {
    /// Residual "bad request" case. Prefer a specific variant where one fits.
    pub fn invalid(detail: impl Into<String>) -> Self {
        Error::InvalidInput {
            detail: detail.into(),
        }
    }

    pub fn corrupt(detail: impl Into<String>) -> Self {
        Error::Corrupt {
            detail: detail.into(),
        }
    }

    pub fn not_found(kind: RefKind, name: impl Into<String>) -> Self {
        Error::NotFound {
            kind,
            name: name.into(),
        }
    }

    pub fn unsupported(detail: impl Into<String>) -> Self {
        Error::Unsupported {
            detail: detail.into(),
        }
    }

    /// The name of this variant, exactly as declared. For bindings and tests
    /// that must name a variant without parsing `Debug` output; **unstable**,
    /// not part of the public API promise.
    ///
    /// The match has no wildcard on purpose: adding a variant stops this crate
    /// compiling until it is named here.
    #[doc(hidden)]
    pub fn variant_name(&self) -> &'static str {
        match self {
            Error::NotARepo { .. } => "NotARepo",
            Error::AlreadyInitialized { .. } => "AlreadyInitialized",
            Error::NestedRepo { .. } => "NestedRepo",
            Error::SchemaTooNew { .. } => "SchemaTooNew",
            Error::MigrationRequired { .. } => "MigrationRequired",
            Error::FormatTooOld { .. } => "FormatTooOld",
            Error::Cancelled => "Cancelled",
            Error::Locked { .. } => "Locked",
            Error::DirtyWorkingTree { .. } => "DirtyWorkingTree",
            Error::WouldOverwriteEdits { .. } => "WouldOverwriteEdits",
            Error::MissingCleanChanges { .. } => "MissingCleanChanges",
            Error::SnapshotHasDependents { .. } => "SnapshotHasDependents",
            Error::OperationInProgress { .. } => "OperationInProgress",
            Error::NoOperationInProgress { .. } => "NoOperationInProgress",
            Error::Conflicts { .. } => "Conflicts",
            Error::Diverged { .. } => "Diverged",
            Error::NotFastForward { .. } => "NotFastForward",
            Error::RemoteRefAbsent { .. } => "RemoteRefAbsent",
            Error::NothingToUndo { .. } => "NothingToUndo",
            Error::NothingToRedo { .. } => "NothingToRedo",
            Error::RedoBranchMoved { .. } => "RedoBranchMoved",
            Error::UnbornBranch { .. } => "UnbornBranch",
            Error::NotFound { .. } => "NotFound",
            Error::AmbiguousPrefix { .. } => "AmbiguousPrefix",
            Error::Compacted { .. } => "Compacted",
            Error::Corrupt { .. } => "Corrupt",
            Error::MissingObject { .. } => "MissingObject",
            Error::UntrustedData { .. } => "UntrustedData",
            Error::UnsafePath { .. } => "UnsafePath",
            Error::LimitExceeded { .. } => "LimitExceeded",
            Error::AccessDenied { .. } => "AccessDenied",
            Error::InvalidInput { .. } => "InvalidInput",
            Error::Unsupported { .. } => "Unsupported",
            Error::Io(_) => "Io",
            Error::Db(_) => "Db",
        }
    }

    /// One realistic value of every variant, for binding tests that prove no
    /// variant falls through to a generic error. **Unstable**; not for
    /// application code. The `samples_cover_every_variant` unit test fails when
    /// a variant has no sample.
    #[doc(hidden)]
    pub fn samples() -> Vec<Error> {
        let id = |c: char| -> crate::SnapshotId {
            c.to_string().repeat(64).parse().expect("a valid id")
        };
        let branch = |n: &str| -> crate::BranchName { n.parse().expect("a valid branch name") };
        vec![
            Error::NotARepo {
                searched_from: PathBuf::from("/work/project"),
            },
            Error::AlreadyInitialized {
                at: PathBuf::from("/work/project"),
            },
            Error::NestedRepo {
                outer: PathBuf::from("/work"),
            },
            Error::SchemaTooNew {
                found: 99,
                supported: 5,
            },
            Error::MigrationRequired {
                found: 4,
                supported: 5,
            },
            Error::FormatTooOld {
                found: 1,
                supported: 5,
            },
            Error::Cancelled,
            Error::Locked {
                held_by: Some(4242),
                holder: Some(crate::lock::LockHolder {
                    pid: 4242,
                    started_at_ms: 1_700_000_000_000,
                    command: Some("velody save".to_string()),
                }),
            },
            Error::DirtyWorkingTree {
                paths: vec![PathBuf::from("src/main.rs")],
            },
            Error::WouldOverwriteEdits {
                paths: vec![PathBuf::from("notes.txt")],
            },
            Error::MissingCleanChanges {
                path: PathBuf::from("notes.txt"),
                missing: 2,
                sample: vec!["first line".to_string(), "second line".to_string()],
            },
            Error::SnapshotHasDependents {
                snapshot: id('a'),
                branches: vec![(branch("main"), 3)],
                dependents: 3,
            },
            Error::OperationInProgress {
                what: InProgress::Merge,
            },
            Error::NoOperationInProgress {
                what: InProgress::Rebase,
            },
            Error::Conflicts {
                paths: vec![PathBuf::from("src/lib.rs")],
            },
            Error::Diverged {
                branch: "main".to_string(),
                ahead: 2,
                behind: 1,
            },
            Error::NotFastForward {
                branch: "main".to_string(),
                remote: "origin".to_string(),
            },
            Error::RemoteRefAbsent {
                remote: "origin".to_string(),
                branch: "main".to_string(),
                snapshot: id('b'),
            },
            Error::NothingToUndo {
                branch: "main".to_string(),
            },
            Error::NothingToRedo {
                branch: "main".to_string(),
            },
            Error::RedoBranchMoved {
                branch: "main".to_string(),
                snapshot: id('b'),
                tip: Some(id('c')),
            },
            Error::UnbornBranch {
                branch: "main".to_string(),
            },
            Error::NotFound {
                kind: RefKind::Branch,
                name: "feature".to_string(),
            },
            Error::AmbiguousPrefix {
                prefix: "ab".to_string(),
                matches: 2,
            },
            Error::Compacted {
                id: "old-id".to_string(),
                into: id('c'),
            },
            Error::Corrupt {
                detail: "snapshot row is truncated".to_string(),
            },
            Error::MissingObject {
                hash: "d".repeat(64),
            },
            Error::UntrustedData {
                detail: "bundle checksum mismatch".to_string(),
            },
            Error::UnsafePath {
                path: "../escape".to_string(),
                problem: crate::safe_path::PathProblem::DotDotComponent,
            },
            Error::LimitExceeded {
                limit: crate::limits::Limit::ObjectBytes,
                max: 1024,
                found: Some(4096),
            },
            Error::AccessDenied {
                remote: "https://example.com/repo".to_string(),
                detail: "a bearer token is required".to_string(),
            },
            Error::InvalidInput {
                detail: "message must not be empty".to_string(),
            },
            Error::Unsupported {
                detail: "not supported on this platform".to_string(),
            },
            Error::Io(std::io::Error::other("disk full")),
            Error::Db(rusqlite::Error::QueryReturnedNoRows),
        ]
    }

    /// True when the caller must reconcile history before retrying.
    pub fn is_reconcile_needed(&self) -> bool {
        matches!(self, Error::Diverged { .. } | Error::NotFastForward { .. })
    }

    /// True when retrying later could plausibly succeed unchanged.
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::Locked { .. })
    }

    /// True when the working tree must be cleaned up before retrying.
    pub fn needs_clean_tree(&self) -> bool {
        matches!(
            self,
            Error::DirtyWorkingTree { .. } | Error::Conflicts { .. }
        )
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Former name of [`Error`]. Retained so `Error::Io` / `Error::Db` call sites
/// written as `VelodyError::…` keep compiling during the in-tree migration.
#[doc(hidden)]
pub type VelodyError = Error;

#[cfg(test)]
mod variant_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every variant, by name. Extend it together with `Error::samples`.
    const ALL: &[&str] = &[
        "NotARepo",
        "AlreadyInitialized",
        "NestedRepo",
        "SchemaTooNew",
        "MigrationRequired",
        "FormatTooOld",
        "Cancelled",
        "Locked",
        "DirtyWorkingTree",
        "WouldOverwriteEdits",
        "MissingCleanChanges",
        "SnapshotHasDependents",
        "OperationInProgress",
        "NoOperationInProgress",
        "Conflicts",
        "Diverged",
        "NotFastForward",
        "RemoteRefAbsent",
        "NothingToUndo",
        "NothingToRedo",
        "RedoBranchMoved",
        "UnbornBranch",
        "NotFound",
        "AmbiguousPrefix",
        "Compacted",
        "Corrupt",
        "MissingObject",
        "UntrustedData",
        "UnsafePath",
        "LimitExceeded",
        "AccessDenied",
        "InvalidInput",
        "Unsupported",
        "Io",
        "Db",
    ];

    /// Exhaustive on purpose (no wildcard): a new variant breaks compilation
    /// here until it is added to `ALL` and given a sample.
    fn _all_covered(e: &Error) -> &'static str {
        match e {
            Error::NotARepo { .. } => "NotARepo",
            Error::AlreadyInitialized { .. } => "AlreadyInitialized",
            Error::NestedRepo { .. } => "NestedRepo",
            Error::SchemaTooNew { .. } => "SchemaTooNew",
            Error::MigrationRequired { .. } => "MigrationRequired",
            Error::FormatTooOld { .. } => "FormatTooOld",
            Error::Cancelled => "Cancelled",
            Error::Locked { .. } => "Locked",
            Error::DirtyWorkingTree { .. } => "DirtyWorkingTree",
            Error::WouldOverwriteEdits { .. } => "WouldOverwriteEdits",
            Error::MissingCleanChanges { .. } => "MissingCleanChanges",
            Error::SnapshotHasDependents { .. } => "SnapshotHasDependents",
            Error::OperationInProgress { .. } => "OperationInProgress",
            Error::NoOperationInProgress { .. } => "NoOperationInProgress",
            Error::Conflicts { .. } => "Conflicts",
            Error::Diverged { .. } => "Diverged",
            Error::NotFastForward { .. } => "NotFastForward",
            Error::RemoteRefAbsent { .. } => "RemoteRefAbsent",
            Error::NothingToUndo { .. } => "NothingToUndo",
            Error::NothingToRedo { .. } => "NothingToRedo",
            Error::RedoBranchMoved { .. } => "RedoBranchMoved",
            Error::UnbornBranch { .. } => "UnbornBranch",
            Error::NotFound { .. } => "NotFound",
            Error::AmbiguousPrefix { .. } => "AmbiguousPrefix",
            Error::Compacted { .. } => "Compacted",
            Error::Corrupt { .. } => "Corrupt",
            Error::MissingObject { .. } => "MissingObject",
            Error::UntrustedData { .. } => "UntrustedData",
            Error::UnsafePath { .. } => "UnsafePath",
            Error::LimitExceeded { .. } => "LimitExceeded",
            Error::AccessDenied { .. } => "AccessDenied",
            Error::InvalidInput { .. } => "InvalidInput",
            Error::Unsupported { .. } => "Unsupported",
            Error::Io(_) => "Io",
            Error::Db(_) => "Db",
        }
    }

    #[test]
    fn samples_cover_every_variant() {
        let samples = Error::samples();
        for e in &samples {
            assert_eq!(e.variant_name(), _all_covered(e));
        }
        let names: Vec<_> = samples.iter().map(|e| e.variant_name()).collect();
        let set: BTreeSet<_> = names.iter().copied().collect();
        assert_eq!(
            set.len(),
            names.len(),
            "a variant has two samples: {names:?}"
        );
        let want: BTreeSet<_> = ALL.iter().copied().collect();
        assert_eq!(set, want, "samples() and the variant list disagree");
    }
}
