//! The repository handle: init, open, free, and the cheap ref lookups.

use std::ffi::c_char;
use std::path::Path;

use velo_core::commands::resolve_snapshot_id;
use velo_core::{BranchName, Repo};

use crate::error::Failure;
use crate::{mut_arg, ref_arg, run, str_arg, to_c_string};

/// An open repository. Opaque to C.
pub struct VeloRepo {
    pub(crate) repo: Repo,
}

fn hand_out(out: *mut *mut VeloRepo, repo: Repo) -> Result<(), Failure> {
    let slot = unsafe { mut_arg(out, "out")? };
    *slot = Box::into_raw(Box::new(VeloRepo { repo }));
    Ok(())
}

/// Create a repository at `path` and open it. On failure `*out` is untouched.
#[no_mangle]
pub unsafe extern "C" fn velo_repo_init(path: *const c_char, out: *mut *mut VeloRepo) -> i32 {
    run(|| {
        let path = str_arg(path, "path")?;
        hand_out(out, Repo::init(Path::new(path))?)
    })
}

/// Open the repository at `path`. On failure `*out` is untouched.
#[no_mangle]
pub unsafe extern "C" fn velo_repo_open(path: *const c_char, out: *mut *mut VeloRepo) -> i32 {
    run(|| {
        let path = str_arg(path, "path")?;
        hand_out(out, Repo::open(Path::new(path))?)
    })
}

/// Close a repository. NULL is a no-op.
#[no_mangle]
pub unsafe extern "C" fn velo_repo_free(repo: *mut VeloRepo) {
    if !repo.is_null() {
        // A panic in Drop must not unwind into C either.
        let _ =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(Box::from_raw(repo))));
    }
}

/// The newest snapshot id on `branch`, written to `*out_id`.
///
/// A branch with no snapshots yet is not an error: `*out_id` is set to NULL and
/// `0` is returned. Free a non-NULL id with `velo_string_free`.
#[no_mangle]
pub unsafe extern "C" fn velo_branch_tip(
    repo: *const VeloRepo,
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
#[no_mangle]
pub unsafe extern "C" fn velo_resolve_snapshot(
    repo: *const VeloRepo,
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
