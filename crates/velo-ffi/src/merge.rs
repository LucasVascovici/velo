//! Three-way merge over snapshots: find the base, plan, and record the result.
//! All of it works on stored trees; no working tree is read or written.

use std::ffi::c_char;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Map, Value};
use velo_core::commands::merge::{self, MergeCommit, PlannedChange, Resolution};
use velo_core::commands::resolve_snapshot_id;
use velo_core::{BranchName, ObjectHash};

use crate::repo::VeloRepo;
use crate::tree::{parse_author, parse_meta, parse_timestamp};
use crate::{
    bad_json, emit, mut_arg, ref_arg, req_str, run, snapshot_arg, str_arg, to_c_string, Res,
};

/// The nearest common ancestor of snapshots `a` and `b` (specs), written to
/// `*out_id`. Histories that share nothing are not an error: `*out_id` is
/// NULL. Free a non-NULL id with `velo_string_free`.
#[no_mangle]
pub unsafe extern "C" fn velo_merge_base(
    repo: *const VeloRepo,
    a: *const c_char,
    b: *const c_char,
    out_id: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let slot = mut_arg(out_id, "out_id")?;
        let a = snapshot_arg(repo, a)?;
        let b = snapshot_arg(repo, b)?;
        *slot = match merge::merge_base(repo, &a, &b)? {
            Some(id) => to_c_string(id.into_string())?,
            None => std::ptr::null_mut(),
        };
        Ok(())
    })
}

fn object_json(hash: &Option<ObjectHash>) -> Value {
    hash.as_ref().map_or(Value::Null, |h| json!(h.as_str()))
}

/// What merging `theirs` into `ours` would do, as JSON:
/// `{"base": id|null, "files": [{"path", "action", ...}]}`.
///
/// `action` is `deleted`, `added`, `updated`, `auto_merged`, `kept_ours` or
/// `conflicted`. `added` and `updated` carry `object` and `mode`;
/// `auto_merged` carries `content_base64` and `mode`; `conflicted` carries
/// `base`, `ours` and `theirs`, each an object id or null when that side has
/// no such file. Nothing is written.
#[no_mangle]
pub unsafe extern "C" fn velo_merge_plan(
    repo: *const VeloRepo,
    ours: *const c_char,
    theirs: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let ours = snapshot_arg(repo, ours)?;
        let theirs = snapshot_arg(repo, theirs)?;
        let plan = merge::plan(repo, &ours, &theirs)?;
        let files: Vec<Value> = plan
            .files
            .iter()
            .map(|f| {
                let mut file = Map::new();
                file.insert("path".into(), json!(f.path));
                let (action, extra) = match &f.change {
                    PlannedChange::Delete => ("deleted", json!({})),
                    PlannedChange::Take {
                        object,
                        mode,
                        is_new,
                    } => (
                        if *is_new { "added" } else { "updated" },
                        json!({"object": object.as_str(), "mode": mode}),
                    ),
                    PlannedChange::AutoMerge { content, mode } => (
                        "auto_merged",
                        json!({"content_base64": STANDARD.encode(content), "mode": mode}),
                    ),
                    PlannedChange::KeepOurs => ("kept_ours", json!({})),
                    PlannedChange::Conflict { base, ours, theirs } => (
                        "conflicted",
                        json!({
                            "base": object_json(base),
                            "ours": object_json(ours),
                            "theirs": object_json(theirs),
                        }),
                    ),
                };
                file.insert("action".into(), json!(action));
                if let Value::Object(extra) = extra {
                    file.extend(extra);
                }
                Value::Object(file)
            })
            .collect();
        emit(
            out_json,
            json!({
                "base": plan.base.as_ref().map(|b| b.as_str()),
                "files": files,
            }),
        )
    })
}

fn parse_resolution(value: &Value) -> Res<Resolution> {
    match value {
        Value::String(s) => match s.as_str() {
            "ours" => Ok(Resolution::Ours),
            "theirs" => Ok(Resolution::Theirs),
            "delete" => Ok(Resolution::Delete),
            other => Err(bad_json(format!(
                "`{other}` is not a resolution (ours, theirs, delete or {{\"content_base64\"}})"
            ))),
        },
        Value::Object(o) => {
            let text = req_str(o, "content_base64")?;
            let bytes = STANDARD
                .decode(text)
                .map_err(|e| bad_json(format!("`content_base64` is not base64: {e}")))?;
            Ok(Resolution::Content(bytes))
        }
        _ => Err(bad_json("a resolution must be a string or an object")),
    }
}

/// Record a merge of `theirs` into `ours` as a new snapshot on a branch, and
/// write its id to `*out_id`.
///
/// `spec_json` is an object with required `branch`, `ours` and `theirs` (the
/// last two are specs) and `message`, and optional `resolutions`
/// (`{path: "ours" | "theirs" | "delete" | {"content_base64": text}}`),
/// `meta` (`{namespace: {key: value}}`), `author` (`{"name", "email"?}`) and
/// `timestamp_ms`. A conflicted path with no resolution fails with
/// `VELO_ERR_CONFLICTS`, whose message lists the paths.
#[no_mangle]
pub unsafe extern "C" fn velo_merge_commit(
    repo: *const VeloRepo,
    spec_json: *const c_char,
    out_id: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let out = mut_arg(out_id, "out_id")?;
        let doc = match serde_json::from_str(str_arg(spec_json, "spec_json")?).map_err(bad_json)? {
            Value::Object(doc) => doc,
            _ => return Err(bad_json("expected a JSON object")),
        };
        let branch: BranchName = req_str(&doc, "branch")?.parse()?;
        let ours = resolve_snapshot_id(repo, req_str(&doc, "ours")?)?;
        let theirs = resolve_snapshot_id(repo, req_str(&doc, "theirs")?)?;
        let message = req_str(&doc, "message")?;
        let resolutions: Vec<(String, Resolution)> = match doc.get("resolutions") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(map)) => map
                .iter()
                .map(|(path, v)| Ok((path.clone(), parse_resolution(v)?)))
                .collect::<Res<_>>()?,
            Some(_) => return Err(bad_json("`resolutions` must be an object")),
        };
        let meta = parse_meta(&doc)?;
        let author = parse_author(&doc)?;
        let timestamp_ms = parse_timestamp(&doc)?;

        let id = merge::commit(
            &repo.write()?,
            MergeCommit {
                branch: &branch,
                ours: &ours,
                theirs: &theirs,
                resolutions: &resolutions,
                message,
                meta,
                author: author.as_ref(),
                timestamp_ms,
            },
        )?;
        *out = to_c_string(id.into_string())?;
        Ok(())
    })
}
