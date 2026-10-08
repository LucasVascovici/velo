//! Error codes and the per-thread "last error" slot.
//!
//! C has no typed errors, so every fallible call returns an `int32_t`: `0` for
//! success, otherwise a code from the table below, with the human-readable text
//! available from [`velo_last_error_message`]. The codes mirror
//! `velo_core::Error` one to one, in declaration order, so a caller can branch
//! on *what* went wrong exactly as a Rust embedder matches on the variant.

use std::cell::RefCell;
use std::ffi::{c_char, CString};

use velo_core::Error;

pub const VELO_OK: i32 = 0;
pub const VELO_ERR_NOT_A_REPO: i32 = 1;
pub const VELO_ERR_ALREADY_INITIALIZED: i32 = 2;
pub const VELO_ERR_NESTED_REPO: i32 = 3;
pub const VELO_ERR_SCHEMA_TOO_NEW: i32 = 4;
pub const VELO_ERR_MIGRATION_REQUIRED: i32 = 5;
pub const VELO_ERR_FORMAT_TOO_OLD: i32 = 6;
pub const VELO_ERR_CANCELLED: i32 = 7;
pub const VELO_ERR_LOCKED: i32 = 8;
pub const VELO_ERR_DIRTY_WORKING_TREE: i32 = 9;
pub const VELO_ERR_OPERATION_IN_PROGRESS: i32 = 10;
pub const VELO_ERR_NO_OPERATION_IN_PROGRESS: i32 = 11;
pub const VELO_ERR_CONFLICTS: i32 = 12;
pub const VELO_ERR_DIVERGED: i32 = 13;
pub const VELO_ERR_NOT_FAST_FORWARD: i32 = 14;
pub const VELO_ERR_UNBORN_BRANCH: i32 = 15;
pub const VELO_ERR_NOT_FOUND: i32 = 16;
pub const VELO_ERR_AMBIGUOUS_PREFIX: i32 = 17;
pub const VELO_ERR_CORRUPT: i32 = 18;
pub const VELO_ERR_MISSING_OBJECT: i32 = 19;
pub const VELO_ERR_UNTRUSTED_DATA: i32 = 20;
pub const VELO_ERR_INVALID_INPUT: i32 = 21;
pub const VELO_ERR_UNSUPPORTED: i32 = 22;
pub const VELO_ERR_IO: i32 = 23;
pub const VELO_ERR_DB: i32 = 24;
/// The snapshot was squashed or re-minted by compaction. The message names the
/// live id that replaced it.
pub const VELO_ERR_COMPACTED: i32 = 25;

/// A required pointer argument was NULL.
pub const VELO_ERR_NULL_ARGUMENT: i32 = 100;
/// A string argument was not valid UTF-8.
pub const VELO_ERR_INVALID_UTF8: i32 = 101;
/// Rust panicked inside the call. Unwinding across the C boundary is undefined
/// behaviour, so the panic is caught and reported here instead.
pub const VELO_ERR_PANIC: i32 = 102;
/// A JSON argument did not parse, or lacked a required field.
pub const VELO_ERR_INVALID_JSON: i32 = 103;
/// A result could not be handed to C (it contained a NUL byte).
pub const VELO_ERR_INVALID_OUTPUT: i32 = 104;
/// A `velo_core::Error` variant this build of the binding does not know.
pub const VELO_ERR_UNKNOWN: i32 = 255;

/// The code for `error`.
///
/// The catch-all arm exists because `Error` is `#[non_exhaustive]`: a variant
/// added to the core must not stop this crate compiling, and must not be
/// reported as success.
pub fn code_of(error: &Error) -> i32 {
    match error {
        Error::NotARepo { .. } => VELO_ERR_NOT_A_REPO,
        Error::AlreadyInitialized { .. } => VELO_ERR_ALREADY_INITIALIZED,
        Error::NestedRepo { .. } => VELO_ERR_NESTED_REPO,
        Error::SchemaTooNew { .. } => VELO_ERR_SCHEMA_TOO_NEW,
        Error::MigrationRequired { .. } => VELO_ERR_MIGRATION_REQUIRED,
        Error::FormatTooOld { .. } => VELO_ERR_FORMAT_TOO_OLD,
        Error::Cancelled => VELO_ERR_CANCELLED,
        Error::Locked { .. } => VELO_ERR_LOCKED,
        Error::DirtyWorkingTree { .. } => VELO_ERR_DIRTY_WORKING_TREE,
        Error::OperationInProgress { .. } => VELO_ERR_OPERATION_IN_PROGRESS,
        Error::NoOperationInProgress { .. } => VELO_ERR_NO_OPERATION_IN_PROGRESS,
        Error::Conflicts { .. } => VELO_ERR_CONFLICTS,
        Error::Diverged { .. } => VELO_ERR_DIVERGED,
        Error::NotFastForward { .. } => VELO_ERR_NOT_FAST_FORWARD,
        Error::UnbornBranch { .. } => VELO_ERR_UNBORN_BRANCH,
        Error::NotFound { .. } => VELO_ERR_NOT_FOUND,
        Error::AmbiguousPrefix { .. } => VELO_ERR_AMBIGUOUS_PREFIX,
        Error::Compacted { .. } => VELO_ERR_COMPACTED,
        Error::Corrupt { .. } => VELO_ERR_CORRUPT,
        Error::MissingObject { .. } => VELO_ERR_MISSING_OBJECT,
        Error::UntrustedData { .. } => VELO_ERR_UNTRUSTED_DATA,
        Error::InvalidInput { .. } => VELO_ERR_INVALID_INPUT,
        Error::Unsupported { .. } => VELO_ERR_UNSUPPORTED,
        Error::Io(_) => VELO_ERR_IO,
        Error::Db(_) => VELO_ERR_DB,
        #[allow(unreachable_patterns)]
        _ => VELO_ERR_UNKNOWN,
    }
}

/// A failure on its way to becoming a return code plus a stored message.
#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub message: String,
}

impl Failure {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Failure {
            code,
            message: message.into(),
        }
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        let mut message = error.to_string();
        // The core text gives only a count; an embedder has no working tree to
        // look at, so the message must name the files itself.
        if let Error::Conflicts { paths } = &error {
            if !paths.is_empty() {
                let names: Vec<_> = paths.iter().map(|p| p.display().to_string()).collect();
                message = format!("{message}: {}", names.join(", "));
            }
        }
        Failure {
            code: code_of(&error),
            message,
        }
    }
}

thread_local! {
    /// Per thread, like `errno`: two threads using two handles must not read
    /// each other's failures.
    static LAST: RefCell<Option<(i32, CString)>> = const { RefCell::new(None) };
}

/// Record `failure` for this thread and return its code.
pub(crate) fn record(failure: Failure) -> i32 {
    // A message with a NUL cannot be a C string; replace rather than lose it.
    let text = CString::new(failure.message.replace('\0', "\u{fffd}")).unwrap_or_default();
    LAST.with(|slot| *slot.borrow_mut() = Some((failure.code, text)));
    failure.code
}

/// The message of the most recent failure on the calling thread, or NULL if
/// none has happened.
///
/// The pointer is owned by the library and stays valid until the next failing
/// call on the same thread. Do not free it, and copy it if it must outlive that.
/// Successful calls leave it untouched, as `errno` is.
#[no_mangle]
pub extern "C" fn velo_last_error_message() -> *const c_char {
    LAST.with(|slot| match &*slot.borrow() {
        Some((_, text)) => text.as_ptr(),
        None => std::ptr::null(),
    })
}

/// The code of the most recent failure on the calling thread, or `0` if none.
#[no_mangle]
pub extern "C" fn velo_last_error_code() -> i32 {
    LAST.with(|slot| slot.borrow().as_ref().map_or(VELO_OK, |(code, _)| *code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use velo_core::error::{InProgress, RefKind};

    #[test]
    fn every_variant_maps_to_its_documented_code() {
        let p = || PathBuf::from("x");
        let s = || "x".to_string();
        let live: velo_core::SnapshotId = "a".repeat(64).parse().expect("a valid id");
        let cases: Vec<(Error, i32)> = vec![
            (Error::NotARepo { searched_from: p() }, 1),
            (Error::AlreadyInitialized { at: p() }, 2),
            (Error::NestedRepo { outer: p() }, 3),
            (
                Error::SchemaTooNew {
                    found: 9,
                    supported: 1,
                },
                4,
            ),
            (
                Error::MigrationRequired {
                    found: 1,
                    supported: 2,
                },
                5,
            ),
            (
                Error::FormatTooOld {
                    found: 1,
                    supported: 2,
                },
                6,
            ),
            (Error::Cancelled, 7),
            (Error::Locked { held_by: None }, 8),
            (Error::DirtyWorkingTree { paths: vec![] }, 9),
            (
                Error::OperationInProgress {
                    what: InProgress::Merge,
                },
                10,
            ),
            (
                Error::NoOperationInProgress {
                    what: InProgress::Merge,
                },
                11,
            ),
            (Error::Conflicts { paths: vec![] }, 12),
            (
                Error::Diverged {
                    branch: s(),
                    ahead: 1,
                    behind: 1,
                },
                13,
            ),
            (
                Error::NotFastForward {
                    branch: s(),
                    remote: s(),
                },
                14,
            ),
            (Error::UnbornBranch { branch: s() }, 15),
            (
                Error::NotFound {
                    kind: RefKind::Any,
                    name: s(),
                },
                16,
            ),
            (
                Error::AmbiguousPrefix {
                    prefix: s(),
                    matches: 2,
                },
                17,
            ),
            (Error::Corrupt { detail: s() }, 18),
            (Error::MissingObject { hash: s() }, 19),
            (Error::UntrustedData { detail: s() }, 20),
            (Error::InvalidInput { detail: s() }, 21),
            (Error::Unsupported { detail: s() }, 22),
            (Error::Io(std::io::Error::other("x")), 23),
            (Error::Db(rusqlite::Error::QueryReturnedNoRows), 24),
            (
                Error::Compacted {
                    id: s(),
                    into: live,
                },
                25,
            ),
        ];
        assert_eq!(cases.len(), 25, "one case per variant");
        for (error, code) in cases {
            assert_eq!(code_of(&error), code, "{error:?}");
        }
    }

    #[test]
    fn compacted_message_names_the_live_id() {
        let live: velo_core::SnapshotId = "b".repeat(64).parse().expect("a valid id");
        let failure = Failure::from(Error::Compacted {
            id: "old".into(),
            into: live.clone(),
        });
        assert_eq!(failure.code, VELO_ERR_COMPACTED);
        assert!(
            failure.message.contains(live.as_str()),
            "{}",
            failure.message
        );
    }
}
