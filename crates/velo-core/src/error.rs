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

/// Everything `velo-core` can fail with.
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
    /// Written by a newer Velo than this one understands. Refusing to open is
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

    // ── Concurrency ──────────────────────────────────────────────────────────
    /// Another process holds the repository write lock.
    Locked {
        held_by: Option<u32>,
    },

    // ── Working-tree preconditions ───────────────────────────────────────────
    /// The operation would overwrite unsaved changes to these tracked files.
    DirtyWorkingTree {
        paths: Vec<PathBuf>,
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
                write!(f, "not a Velo repository (no .velo found here or above)")
            }
            Error::AlreadyInitialized { at } => {
                write!(f, "a Velo repository already exists at '{}'", at.display())
            }
            Error::NestedRepo { outer } => write!(
                f,
                "already inside a Velo repository at '{}'; nested repositories are not supported",
                outer.display()
            ),
            Error::SchemaTooNew { found, supported } => write!(
                f,
                "repository format v{} is newer than this Velo supports (v{})",
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
            Error::Locked { held_by } => match held_by {
                Some(pid) => write!(f, "repository is locked by process {}", pid),
                None => write!(
                    f,
                    "another Velo operation is in progress in this repository"
                ),
            },
            Error::DirtyWorkingTree { paths } => {
                write!(f, "{} unsaved change(s) would be overwritten", paths.len())
            }
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
            Error::UnbornBranch { branch } => write!(f, "branch '{}' has no commits yet", branch),
            Error::NotFound { kind, name } => write!(f, "no {} found matching '{}'", kind, name),
            Error::AmbiguousPrefix { prefix, matches } => write!(
                f,
                "prefix '{}' matches {} snapshots; use more characters",
                prefix, matches
            ),
            Error::Corrupt { detail } => write!(f, "corrupt repository: {}", detail),
            Error::MissingObject { hash } => {
                write!(f, "object {} is missing from the object store", hash)
            }
            Error::UntrustedData { detail } => {
                write!(f, "received data failed verification: {}", detail)
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
/// written as `VeloError::…` keep compiling during the in-tree migration.
#[doc(hidden)]
pub type VeloError = Error;
