//! The repository handle: init, open, free, and the cheap ref lookups.

use std::ffi::c_char;
use std::path::Path;

use velody_core::commands::resolve_snapshot_id;
use velody_core::{BranchName, Repo};

use crate::error::Failure;
use crate::{mut_arg, ref_arg, run, str_arg, to_c_string};

/// An open repository. Opaque to C.
pub struct VelodyRepo {
    pub(crate) repo: Repo,
}

fn hand_out(out: *mut *mut VelodyRepo, repo: Repo) -> Result<(), Failure> {
    // SAFETY: `out` is the caller's out-parameter, NULL or writable; `mut_arg`
    // rejects NULL.
    let slot = unsafe { mut_arg(out, "out")? };
    *slot = Box::into_raw(Box::new(VelodyRepo { repo }));
    Ok(())
}

/// Create a repository at `path` and open it. On failure `*out` is untouched.
///
/// # Safety
///
/// Every pointer argument must be NULL or valid as the crate-level contract in
/// `include/velody.h` states: strings are NUL-terminated, handles and buffers came
/// from this library and are not used after being freed, and out-parameters point
/// to writable storage.
#[no_mangle]
pub unsafe extern "C" fn velody_repo_init(path: *const c_char, out: *mut *mut VelodyRepo) -> i32 {
    run(|| {
        let path = str_arg(path, "path")?;
        hand_out(out, Repo::init(Path::new(path))?)
    })
}

/// Open the repository at `path`. On failure `*out` is untouched.
///
/// # Safety
///
/// Every pointer argument must be NULL or valid as the crate-level contract in
/// `include/velody.h` states: strings are NUL-terminated, handles and buffers came
/// from this library and are not used after being freed, and out-parameters point
/// to writable storage.
#[no_mangle]
pub unsafe extern "C" fn velody_repo_open(path: *const c_char, out: *mut *mut VelodyRepo) -> i32 {
    run(|| {
        let path = str_arg(path, "path")?;
        hand_out(out, Repo::open(Path::new(path))?)
    })
}

/// Close a repository. NULL is a no-op.
///
/// # Safety
///
/// Every pointer argument must be NULL or valid as the crate-level contract in
/// `include/velody.h` states: strings are NUL-terminated, handles and buffers came
/// from this library and are not used after being freed, and out-parameters point
/// to writable storage.
#[no_mangle]
pub unsafe extern "C" fn velody_repo_free(repo: *mut VelodyRepo) {
    if !repo.is_null() {
        // A panic in Drop must not unwind into C either.
        let _ =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(Box::from_raw(repo))));
    }
}

/// The newest snapshot id on `branch`, written to `*out_id`.
///
/// A branch with no snapshots yet is not an error: `*out_id` is set to NULL and
/// `0` is returned. Free a non-NULL id with `velody_string_free`.
///
/// # Safety
///
/// Every pointer argument must be NULL or valid as the crate-level contract in
/// `include/velody.h` states: strings are NUL-terminated, handles and buffers came
/// from this library and are not used after being freed, and out-parameters point
/// to writable storage.
#[no_mangle]
pub unsafe extern "C" fn velody_branch_tip(
    repo: *const VelodyRepo,
    branch: *const c_char,
    out_id: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let branch: BranchName = str_arg(branch, "branch")?.parse()?;
        let slot = mut_arg(out_id, "out_id")?;
        *slot = match repo.branch_tip(&branch)? {
            Some(id) => to_c_string(id.into_string())?,
            None => std::ptr::null_mut(),
        };
        Ok(())
    })
}

/// Resolve a tag, branch, id or unique id prefix to a full snapshot id.
///
/// # Safety
///
/// Every pointer argument must be NULL or valid as the crate-level contract in
/// `include/velody.h` states: strings are NUL-terminated, handles and buffers came
/// from this library and are not used after being freed, and out-parameters point
/// to writable storage.
#[no_mangle]
pub unsafe extern "C" fn velody_resolve_snapshot(
    repo: *const VelodyRepo,
    spec: *const c_char,
    out_id: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let id = resolve_snapshot_id(repo, str_arg(spec, "spec")?)?;
        *mut_arg(out_id, "out_id")? = to_c_string(id.into_string())?;
        Ok(())
    })
}
