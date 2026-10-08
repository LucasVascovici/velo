//! A C ABI over the embeddable half of `velo-core`.
//!
//! The surface is deliberately the *store* half of the API — `Repo`,
//! `save_tree`, `tree_at`, snapshot and metadata reads. Nothing here touches a
//! working tree: a binding that needs one is wrapping the wrong half.
//!
//! # Conventions
//!
//! - **Handles are opaque.** `VeloRepo` and `VeloTree` are created by a
//!   `velo_*_open` / `velo_*_new` call and released by the matching `*_free`.
//!   A handle is not thread-safe; use one per thread, or serialise externally.
//! - **Errors are a code plus a message.** Every fallible function returns an
//!   `int32_t`, `0` on success. On failure the text is available from
//!   [`velo_last_error_message`] on the same thread. See [`error`] for the table.
//! - **Results come back through out-parameters** so the return value is free to
//!   carry the code. Strings are NUL-terminated UTF-8 freed with
//!   [`velo_string_free`]; byte buffers are freed with [`velo_bytes_free`].
//! - **Structured results are JSON**, so the header stays small and a new field
//!   is not an ABI break.
//! - **Panics never cross the boundary.** They are caught and reported as
//!   `VELO_ERR_PANIC`.
//!
//! The C declarations live in `include/velo.h`; a test checks that every
//! exported symbol is declared there.

// Every exported function is `unsafe` because it takes raw pointers, and the
// contract is the same for all of them and stated once, above and in the header:
// pointers are NULL-checked, strings are NUL-terminated, and handles come from
// this library and are not used after being freed.
#![allow(clippy::missing_safety_doc)]

pub mod error;
mod repo;
mod tree;

use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

pub use error::*;
pub use repo::*;
pub use tree::*;

use error::record;

pub(crate) type Res<T> = Result<T, Failure>;

/// Run `body`, turning an error or a panic into a recorded failure and a code.
pub(crate) fn run(body: impl FnOnce() -> Res<()>) -> i32 {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(())) => VELO_OK,
        Ok(Err(failure)) => record(failure),
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            record(Failure::new(VELO_ERR_PANIC, format!("panic: {what}")))
        }
    }
}

fn null_arg(name: &str) -> Failure {
    Failure::new(
        VELO_ERR_NULL_ARGUMENT,
        format!("`{name}` must not be NULL."),
    )
}

/// A required string argument.
pub(crate) unsafe fn str_arg<'a>(ptr: *const c_char, name: &str) -> Res<&'a str> {
    if ptr.is_null() {
        return Err(null_arg(name));
    }
    CStr::from_ptr(ptr).to_str().map_err(|_| {
        Failure::new(
            VELO_ERR_INVALID_UTF8,
            format!("`{name}` is not valid UTF-8."),
        )
    })
}

/// A required handle argument.
pub(crate) unsafe fn ref_arg<'a, T>(ptr: *const T, name: &str) -> Res<&'a T> {
    ptr.as_ref().ok_or_else(|| null_arg(name))
}

/// A required mutable handle argument.
pub(crate) unsafe fn mut_arg<'a, T>(ptr: *mut T, name: &str) -> Res<&'a mut T> {
    ptr.as_mut().ok_or_else(|| null_arg(name))
}

/// Move `text` to the heap as a C string for the caller to free.
pub(crate) fn to_c_string(text: String) -> Res<*mut c_char> {
    CString::new(text).map(CString::into_raw).map_err(|_| {
        Failure::new(
            VELO_ERR_INVALID_OUTPUT,
            "the result contains a NUL byte and cannot be returned as a C string.",
        )
    })
}

/// Free a string returned by this library. NULL is a no-op.
#[no_mangle]
pub unsafe extern "C" fn velo_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(CString::from_raw(text));
    }
}

/// Free a buffer returned by this library, given the length it came with.
/// NULL is a no-op.
#[no_mangle]
pub unsafe extern "C" fn velo_bytes_free(data: *mut u8, len: usize) {
    if !data.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)));
    }
}
