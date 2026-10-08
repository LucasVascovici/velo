//! One conversion from `velo_core::Error` to a JavaScript `Error`.
//!
//! The rejection's `code` is the variant name and the variant's fields are
//! attached as properties, so the typed errors of velo-core are not flattened
//! into strings at the boundary.

use napi::bindgen_prelude::*;
use napi::Env;
use velo_core::error::{InProgress, RefKind};
use velo_core::Error;

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

/// The variant name, which becomes the rejection's `code`.
fn code(err: &Error) -> &'static str {
    match err {
        Error::NotARepo { .. } => "NotARepo",
        Error::AlreadyInitialized { .. } => "AlreadyInitialized",
        Error::NestedRepo { .. } => "NestedRepo",
        Error::SchemaTooNew { .. } => "SchemaTooNew",
        Error::MigrationRequired { .. } => "MigrationRequired",
        Error::FormatTooOld { .. } => "FormatTooOld",
        Error::Cancelled => "Cancelled",
        Error::Locked { .. } => "Locked",
        Error::DirtyWorkingTree { .. } => "DirtyWorkingTree",
        Error::OperationInProgress { .. } => "OperationInProgress",
        Error::NoOperationInProgress { .. } => "NoOperationInProgress",
        Error::Conflicts { .. } => "Conflicts",
        Error::Diverged { .. } => "Diverged",
        Error::NotFastForward { .. } => "NotFastForward",
        Error::UnbornBranch { .. } => "UnbornBranch",
        Error::NotFound { .. } => "NotFound",
        Error::AmbiguousPrefix { .. } => "AmbiguousPrefix",
        Error::Compacted { .. } => "Compacted",
        Error::Corrupt { .. } => "Corrupt",
        Error::MissingObject { .. } => "MissingObject",
        Error::UntrustedData { .. } => "UntrustedData",
        Error::InvalidInput { .. } => "InvalidInput",
        Error::Unsupported { .. } => "Unsupported",
        Error::Io(_) => "Io",
        Error::Db(_) => "Db",
        // `Error` is non_exhaustive: a variant added later still rejects with
        // a recognisable code rather than panicking.
        _ => "Unknown",
    }
}

/// Build the JS `Error` for a velo error and return it as a napi error that
/// rejects the promise with exactly that object. This is the only conversion
/// in the crate; it must run on the JS thread.
pub fn to_js(env: &Env, err: Error) -> napi::Error {
    match build(env, &err) {
        Ok(e) => e,
        // Building the object failed; still reject, with the plain message.
        Err(_) => napi::Error::from_reason(err.to_string()),
    }
}

fn build(env: &Env, err: &Error) -> napi::Result<napi::Error> {
    let mut obj: Object = env
        .create_error(napi::Error::new(Status::GenericFailure, err.to_string()))?
        .coerce_to_object()?;
    obj.set("code", code(err))?;
    match err {
        Error::NotARepo { searched_from } => obj.set("searchedFrom", path(searched_from))?,
        Error::AlreadyInitialized { at } => obj.set("at", path(at))?,
        Error::NestedRepo { outer } => obj.set("outer", path(outer))?,
        Error::SchemaTooNew { found, supported }
        | Error::MigrationRequired { found, supported }
        | Error::FormatTooOld { found, supported } => {
            obj.set("found", *found as f64)?;
            obj.set("supported", *supported as f64)?;
        }
        Error::Locked { held_by } => obj.set("heldBy", held_by.map(f64::from))?,
        Error::DirtyWorkingTree { paths: p } | Error::Conflicts { paths: p } => {
            obj.set("paths", paths(p))?
        }
        Error::OperationInProgress { what } | Error::NoOperationInProgress { what } => {
            obj.set("what", in_progress(what))?
        }
        Error::Diverged {
            branch,
            ahead,
            behind,
        } => {
            obj.set("branch", branch.as_str())?;
            obj.set("ahead", *ahead as f64)?;
            obj.set("behind", *behind as f64)?;
        }
        Error::NotFastForward { branch, remote } => {
            obj.set("branch", branch.as_str())?;
            obj.set("remote", remote.as_str())?;
        }
        Error::UnbornBranch { branch } => obj.set("branch", branch.as_str())?,
        Error::NotFound { kind, name } => {
            obj.set("kind", ref_kind(kind))?;
            obj.set("name", name.as_str())?;
        }
        Error::AmbiguousPrefix { prefix, matches } => {
            obj.set("prefix", prefix.as_str())?;
            obj.set("matches", *matches as f64)?;
        }
        Error::Compacted { id, into } => {
            obj.set("id", id.as_str())?;
            obj.set("into", into.as_str())?;
        }
        Error::MissingObject { hash } => obj.set("hash", hash.as_str())?,
        Error::Corrupt { detail }
        | Error::UntrustedData { detail }
        | Error::InvalidInput { detail }
        | Error::Unsupported { detail } => obj.set("detail", detail.as_str())?,
        _ => {}
    }
    Ok(napi::Error::from(obj.to_unknown()))
}
