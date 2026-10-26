//! One conversion from `velody_core::Error` to a JavaScript `Error`.
//!
//! The rejection's `code` is the variant name and the variant's fields are
//! attached as properties, so the typed errors of velody-core are not flattened
//! into strings at the boundary.

use napi::bindgen_prelude::*;
use napi::Env;
use napi_derive::napi;
use velody_core::error::{InProgress, RefKind};
use velody_core::Error;

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
        Error::InvalidInput { detail } if detail == crate::callbacks::REENTRANT => "Reentrant",
        Error::InvalidInput { detail } if detail.starts_with(crate::callbacks::DRIVER_FAILED) => {
            "DriverFailed"
        }
        // `variant_name` matches every variant exhaustively, so no variant can
        // fall through to a generic code.
        _ => err.variant_name(),
    }
}

/// `[variant, code]` for one sample of every core error. Hidden: it exists so a
/// test can prove no variant falls through to a generic code.
#[cfg_attr(test, allow(dead_code))]
#[napi(js_name = "_errorCodes")]
pub fn error_codes() -> Vec<Vec<String>> {
    Error::samples()
        .iter()
        .map(|e| vec![e.variant_name().to_string(), code(e).to_string()])
        .collect()
}

/// Build the JS `Error` for a velody error and return it as a napi error that
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
    // A refused re-entrant call and a failed JS driver travel as `InvalidInput`
    // with a marked detail; their message is the detail itself.
    let message = match err {
        Error::InvalidInput { detail } if code(err) != "InvalidInput" => detail
            .strip_prefix(crate::callbacks::DRIVER_FAILED)
            .unwrap_or(detail)
            .to_string(),
        _ => err.to_string(),
    };
    let mut obj: Object = env
        .create_error(napi::Error::new(Status::GenericFailure, message))?
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
        Error::WouldOverwriteEdits { paths: p } => obj.set("paths", paths(p))?,
        Error::MissingCleanChanges {
            path: p,
            missing,
            sample,
        } => {
            obj.set("path", path(p))?;
            obj.set("missing", *missing as f64)?;
            obj.set("sample", sample.clone())?;
        }
        Error::SnapshotHasDependents {
            snapshot,
            branches,
            dependents,
        } => {
            obj.set("snapshot", snapshot.as_str())?;
            obj.set("dependents", *dependents as f64)?;
            obj.set(
                "branches",
                branches
                    .iter()
                    .map(|(b, n)| vec![b.as_str().to_string(), n.to_string()])
                    .collect::<Vec<_>>(),
            )?;
        }
        Error::RemoteRefAbsent {
            remote,
            branch,
            snapshot,
        } => {
            obj.set("remote", remote.as_str())?;
            obj.set("branch", branch.as_str())?;
            obj.set("snapshot", snapshot.as_str())?;
        }
        Error::NothingToUndo { branch } | Error::NothingToRedo { branch } => {
            obj.set("branch", branch.as_str())?;
        }
        Error::RedoBranchMoved {
            branch,
            snapshot,
            tip,
        } => {
            obj.set("branch", branch.as_str())?;
            obj.set("snapshot", snapshot.as_str())?;
            obj.set("tip", tip.as_ref().map(|t| t.as_str().to_string()))?;
        }
        Error::UnsafePath { path: p, problem } => {
            obj.set("path", p.as_str())?;
            obj.set("problem", problem.to_string())?;
        }
        Error::LimitExceeded { limit, max, found } => {
            obj.set("limit", limit.to_string())?;
            obj.set("max", *max as f64)?;
            obj.set("found", found.map(|n| n as f64))?;
        }
        Error::AccessDenied { remote, detail } => {
            obj.set("remote", remote.as_str())?;
            obj.set("detail", detail.as_str())?;
        }
        Error::Locked { held_by, .. } => obj.set("heldBy", held_by.map(f64::from))?,
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
        | Error::Unsupported { detail } => obj.set("detail", detail.as_str())?,
        Error::InvalidInput { detail } if code(err) == "InvalidInput" => {
            obj.set("detail", detail.as_str())?
        }
        _ => {}
    }
    Ok(napi::Error::from(obj.to_unknown()))
}
