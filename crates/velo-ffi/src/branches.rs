//! Branches as refs: list, create and move. No working tree is touched.

use std::ffi::c_char;

use serde_json::{json, Value};
use velo_core::commands::{branches, resolve_snapshot_id};
use velo_core::BranchName;

use crate::repo::VeloRepo;
use crate::{emit, ref_arg, run, snapshot_arg, str_arg};

/// Every branch as a JSON array of `{"name", "is_current", "tip"}`, where
/// `tip` is null or `{"id", "message", "created_at", "created_at_ms"}`.
#[no_mangle]
pub unsafe extern "C" fn velo_branches(repo: *const VeloRepo, out_json: *mut *mut c_char) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let list: Vec<Value> = branches::list(repo)?
            .iter()
            .map(|b| {
                json!({
                    "name": b.name.as_str(),
                    "is_current": b.is_current,
                    "tip": b.tip.as_ref().map(|t| json!({
                        "id": t.hash.as_str(),
                        "message": t.message,
                        "created_at": t.created_at.to_rfc3339(),
                        "created_at_ms": t.created_at.timestamp_millis(),
                    })),
                })
            })
            .collect();
        emit(out_json, Value::Array(list))
    })
}

/// Create branch `name` pointing at `at_or_null` (a spec), or, when that is
/// NULL, at the current snapshot.
#[no_mangle]
pub unsafe extern "C" fn velo_branch_create(
    repo: *const VeloRepo,
    name: *const c_char,
    at_or_null: *const c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let name: BranchName = str_arg(name, "name")?.parse()?;
        let at = if at_or_null.is_null() {
            None
        } else {
            Some(resolve_snapshot_id(
                repo,
                str_arg(at_or_null, "at_or_null")?,
            )?)
        };
        branches::create(&repo.write()?, &name, at.as_ref())?;
        Ok(())
    })
}

/// Point branch `name` at snapshot `to` (a spec). An unknown snapshot is
/// `VELO_ERR_NOT_FOUND`.
#[no_mangle]
pub unsafe extern "C" fn velo_branch_set_tip(
    repo: *const VeloRepo,
    name: *const c_char,
    to: *const c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let name: BranchName = str_arg(name, "name")?.parse()?;
        let to = snapshot_arg(repo, to)?;
        branches::set_tip(&repo.write()?, &name, &to)?;
        Ok(())
    })
}
