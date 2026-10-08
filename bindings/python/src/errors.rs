//! One exception class per `velo_core::Error` variant.
//!
//! `to_py` is the only conversion, so the typed errors of velo-core are never
//! flattened into strings at the boundary.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use velo_core::error::{InProgress, RefKind};
use velo_core::Error;

create_exception!(
    _velo,
    VeloError,
    PyException,
    "Base class of every velo error."
);

macro_rules! exceptions {
    ($($name:ident),* $(,)?) => {
        $(create_exception!(_velo, $name, VeloError);)*

        pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
            m.add("VeloError", m.py().get_type::<VeloError>())?;
            $(m.add(stringify!($name), m.py().get_type::<$name>())?;)*
            Ok(())
        }
    };
}

exceptions!(
    NotARepo,
    AlreadyInitialized,
    NestedRepo,
    SchemaTooNew,
    MigrationRequired,
    FormatTooOld,
    Cancelled,
    Locked,
    DirtyWorkingTree,
    OperationInProgress,
    NoOperationInProgress,
    Conflicts,
    Diverged,
    NotFastForward,
    UnbornBranch,
    NotFound,
    AmbiguousPrefix,
    Compacted,
    Corrupt,
    MissingObject,
    UntrustedData,
    InvalidInput,
    Unsupported,
    VeloIOError,
    DatabaseError,
);

fn ref_kind(kind: &RefKind) -> &'static str {
    match kind {
        RefKind::Snapshot => "snapshot",
        RefKind::Branch => "branch",
        RefKind::Tag => "tag",
        RefKind::Remote => "remote",
        RefKind::RemoteBranch => "remote_branch",
        RefKind::Stash => "stash",
        RefKind::Path => "path",
        RefKind::Any => "any",
        _ => "unknown",
    }
}

fn in_progress(what: &InProgress) -> &'static str {
    match what {
        InProgress::Merge => "merge",
        InProgress::Rebase => "rebase",
        InProgress::CherryPick => "cherry_pick",
        _ => "unknown",
    }
}

fn paths(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

fn path(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Convert a velo error to the matching Python exception, with the variant's
/// fields attached as attributes. This is the only conversion in the crate.
pub fn to_py(err: Error) -> PyErr {
    let msg = err.to_string();
    Python::attach(|py| {
        macro_rules! raise {
            ($ty:ty $(, $attr:literal => $val:expr)* $(,)?) => {{
                let e = PyErr::new::<$ty, _>(msg.clone());
                $(
                    // A failed setattr must not mask the original error.
                    let _ = e.value(py).setattr($attr, $val);
                )*
                e
            }};
        }
        match &err {
            Error::NotARepo { searched_from } => {
                raise!(NotARepo, "searched_from" => path(searched_from))
            }
            Error::AlreadyInitialized { at } => raise!(AlreadyInitialized, "at" => path(at)),
            Error::NestedRepo { outer } => raise!(NestedRepo, "outer" => path(outer)),
            Error::SchemaTooNew { found, supported } => {
                raise!(SchemaTooNew, "found" => *found, "supported" => *supported)
            }
            Error::MigrationRequired { found, supported } => {
                raise!(MigrationRequired, "found" => *found, "supported" => *supported)
            }
            Error::FormatTooOld { found, supported } => {
                raise!(FormatTooOld, "found" => *found, "supported" => *supported)
            }
            Error::Cancelled => raise!(Cancelled),
            Error::Locked { held_by } => raise!(Locked, "held_by" => *held_by),
            Error::DirtyWorkingTree { paths: p } => {
                raise!(DirtyWorkingTree, "paths" => paths(p))
            }
            Error::OperationInProgress { what } => {
                raise!(OperationInProgress, "what" => in_progress(what))
            }
            Error::NoOperationInProgress { what } => {
                raise!(NoOperationInProgress, "what" => in_progress(what))
            }
            Error::Conflicts { paths: p } => raise!(Conflicts, "paths" => paths(p)),
            Error::Diverged {
                branch,
                ahead,
                behind,
            } => raise!(Diverged,
                "branch" => branch.as_str(), "ahead" => *ahead, "behind" => *behind),
            Error::NotFastForward { branch, remote } => {
                raise!(NotFastForward, "branch" => branch.as_str(), "remote" => remote.as_str())
            }
            Error::UnbornBranch { branch } => raise!(UnbornBranch, "branch" => branch.as_str()),
            Error::NotFound { kind, name } => {
                raise!(NotFound, "kind" => ref_kind(kind), "name" => name.as_str())
            }
            Error::AmbiguousPrefix { prefix, matches } => {
                raise!(AmbiguousPrefix, "prefix" => prefix.as_str(), "matches" => *matches)
            }
            Error::Compacted { id, into } => {
                raise!(Compacted, "id" => id.as_str(), "into" => into.as_str())
            }
            Error::Corrupt { detail } => raise!(Corrupt, "detail" => detail.as_str()),
            Error::MissingObject { hash } => raise!(MissingObject, "hash" => hash.as_str()),
            Error::UntrustedData { detail } => raise!(UntrustedData, "detail" => detail.as_str()),
            Error::InvalidInput { detail } => raise!(InvalidInput, "detail" => detail.as_str()),
            Error::Unsupported { detail } => raise!(Unsupported, "detail" => detail.as_str()),
            Error::Io(_) => raise!(VeloIOError),
            Error::Db(_) => raise!(DatabaseError),
            // `Error` is non_exhaustive: a variant added later still surfaces
            // as a velo error rather than a panic.
            _ => raise!(VeloError),
        }
    })
}
