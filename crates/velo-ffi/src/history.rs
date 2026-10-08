//! History, metadata search and blame: reading how a repository got here.

use std::ffi::c_char;
use std::ops::Range;
use std::path::Path;

use serde_json::{json, Map, Value};
use velo_core::commands::history::{self, EmptyReason, MetaFilter};
use velo_core::commands::{blame, resolve_snapshot_id};
use velo_core::{BranchName, SnapshotId};

use crate::error::{Failure, VELO_ERR_INVALID_INPUT};
use crate::repo::VeloRepo;
use crate::{bad_json, emit, opt_str, ref_arg, run, str_arg, Res};

/// One snapshot as JSON, the shape shared by `velo_snapshot`, `velo_history`
/// and `velo_find_snapshots`.
pub(crate) fn entry_json(e: &history::Entry) -> Value {
    json!({
        "id": e.hash.as_str(),
        "message": e.message,
        "created_at": e.created_at.to_rfc3339(),
        "created_at_ms": e.created_at.timestamp_millis(),
        "branch": e.branch.as_str(),
        "parent": e.parent.as_ref().map(|p| p.as_str()),
        "merge_parent": e.merge_parent.as_ref().map(|p| p.as_str()),
        "tag": e.tag.as_ref().map(|t| t.as_str()),
    })
}

/// A `{"namespace","key","value"|null}` filter, owned so `MetaFilter` can
/// borrow from it afterwards.
struct OwnedFilter {
    namespace: String,
    key: String,
    value: Option<String>,
}

impl OwnedFilter {
    fn as_filter(&self) -> MetaFilter<'_> {
        match &self.value {
            Some(v) => MetaFilter::equals(&self.namespace, &self.key, v),
            None => MetaFilter::has(&self.namespace, &self.key),
        }
    }
}

fn parse_filters(value: &Value) -> Res<Vec<OwnedFilter>> {
    let items = value
        .as_array()
        .ok_or_else(|| bad_json("metadata filters must be an array"))?;
    items
        .iter()
        .map(|item| {
            let doc = item
                .as_object()
                .ok_or_else(|| bad_json("each metadata filter must be an object"))?;
            let need = |key: &str| {
                opt_str(doc, key)?
                    .map(str::to_string)
                    .ok_or_else(|| bad_json(format!("a metadata filter needs `{key}`")))
            };
            Ok(OwnedFilter {
                namespace: need("namespace")?,
                key: need("key")?,
                value: opt_str(doc, "value")?.map(str::to_string),
            })
        })
        .collect()
}

fn parse_object(text: &str) -> Res<Map<String, Value>> {
    match serde_json::from_str(text).map_err(bad_json)? {
        Value::Object(doc) => Ok(doc),
        _ => Err(bad_json("expected a JSON object")),
    }
}

fn opt_count(doc: &Map<String, Value>, key: &str) -> Res<Option<usize>> {
    match doc.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| bad_json(format!("`{key}` must be a non-negative integer"))),
    }
}

fn empty_name(reason: &EmptyReason) -> &'static str {
    match reason {
        EmptyReason::UnbornBranch { .. } => "unborn_branch",
        EmptyReason::NoSnapshots => "no_snapshots",
        EmptyReason::NoSnapshotsTouching { .. } => "no_snapshots_touching",
        EmptyReason::NoSnapshotsMatching => "no_snapshots_matching",
        // `EmptyReason` is non-exhaustive: a newer core may add reasons.
        _ => "other",
    }
}

/// Snapshot history as JSON: `{"entries": [Entry, ...], "empty": reason|null}`,
/// newest first, each entry shaped like `velo_snapshot`'s.
///
/// `options_json` is an object whose members are all optional: `from` (a
/// spec; walk its ancestry), `branch` (a branch name), `all` (every branch),
/// `paths` (only snapshots touching one of these files), `limit` (integer)
/// and `meta` (`[{"namespace","key","value"|null}, ...]`; a null or missing
/// value means "the key is set to anything").
///
/// With none of `from`, `branch` or `all`, history follows `.velo/PARENT`,
/// which a store-only embedder does not have. Pass `from` or `branch`.
///
/// `empty` is `unborn_branch`, `no_snapshots`, `no_snapshots_touching`,
/// `no_snapshots_matching` or `other`, and is null when entries were found.
#[no_mangle]
pub unsafe extern "C" fn velo_history(
    repo: *const VeloRepo,
    options_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let doc = parse_object(str_arg(options_json, "options_json")?)?;

        let from: Option<SnapshotId> = match opt_str(&doc, "from")? {
            Some(s) => Some(resolve_snapshot_id(repo, s)?),
            None => None,
        };
        let branch: Option<BranchName> = match opt_str(&doc, "branch")? {
            Some(s) => Some(s.parse()?),
            None => None,
        };
        let all = match doc.get("all") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(bad_json("`all` must be a boolean")),
        };
        let paths: Vec<&Path> = match doc.get("paths") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|p| {
                    p.as_str()
                        .map(Path::new)
                        .ok_or_else(|| bad_json("`paths` must be an array of strings"))
                })
                .collect::<Res<_>>()?,
            Some(_) => return Err(bad_json("`paths` must be an array")),
        };
        let limit = opt_count(&doc, "limit")?;
        let owned = match doc.get("meta") {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => parse_filters(v)?,
        };
        let meta: Vec<MetaFilter<'_>> = owned.iter().map(OwnedFilter::as_filter).collect();

        let result = history::run(
            repo,
            history::Options {
                all,
                branch: branch.as_ref(),
                from: from.as_ref(),
                paths: &paths,
                meta: &meta,
                limit,
            },
        )?;
        emit(
            out_json,
            json!({
                "entries": result.entries.iter().map(entry_json).collect::<Vec<_>>(),
                "empty": result.empty.as_ref().map(empty_name),
            }),
        )
    })
}

/// Snapshots matching every metadata filter, newest first, as a JSON array of
/// entries. `filters_json` is `[{"namespace","key","value"|null}, ...]`; an
/// empty array is `VELO_ERR_INVALID_INPUT`, because it would match everything.
#[no_mangle]
pub unsafe extern "C" fn velo_find_snapshots(
    repo: *const VeloRepo,
    filters_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let value: Value =
            serde_json::from_str(str_arg(filters_json, "filters_json")?).map_err(bad_json)?;
        let owned = parse_filters(&value)?;
        let filters: Vec<MetaFilter<'_>> = owned.iter().map(OwnedFilter::as_filter).collect();
        let entries = repo.find_snapshots(&filters)?;
        emit(
            out_json,
            Value::Array(entries.iter().map(entry_json).collect()),
        )
    })
}

/// Who wrote each line of `path`, as JSON:
/// `{"path", "snapshot", "lines": [{"line_no", "line_count", "text", "origin"}]}`.
///
/// `origin` is null for a line that could not be attributed, otherwise
/// `{"id", "created_at", "created_at_ms", "message", "author", "branch",
/// "path"}` with `author` null or `{"name", "email"}`, and `path` the name the
/// file had when the line was written.
///
/// `options_json` is an object with optional `at` (a spec; default is the
/// current branch tip), `start_line` and `end_line` (1-based, inclusive; a
/// missing `end_line` runs to the end of the file).
#[no_mangle]
pub unsafe extern "C" fn velo_blame(
    repo: *const VeloRepo,
    path: *const c_char,
    options_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let path = str_arg(path, "path")?;
        let doc = parse_object(str_arg(options_json, "options_json")?)?;
        let at = match opt_str(&doc, "at")? {
            Some(s) => Some(resolve_snapshot_id(repo, s)?),
            None => None,
        };
        let start = opt_count(&doc, "start_line")?;
        let end = opt_count(&doc, "end_line")?;
        let lines: Option<Range<usize>> = match (start, end) {
            (None, None) => None,
            (start, end) => {
                let start = start.unwrap_or(1);
                if start == 0 || end == Some(0) {
                    return Err(Failure::new(
                        VELO_ERR_INVALID_INPUT,
                        "line numbers are 1-based.",
                    ));
                }
                // JSON is inclusive; the core range is half-open.
                Some(start..end.map_or(usize::MAX, |e| e.saturating_add(1)))
            }
        };

        let result = blame::run(
            repo,
            Path::new(path),
            blame::Options {
                at: at.as_ref(),
                lines,
                ..Default::default()
            },
        )?;
        let lines: Vec<Value> = result
            .lines
            .iter()
            .map(|l| {
                let origin = l.origin.as_ref().map(|o| {
                    json!({
                        "id": o.hash.as_str(),
                        "created_at": o.created_at.to_rfc3339(),
                        "created_at_ms": o.created_at.timestamp_millis(),
                        "message": o.message,
                        "author": o.author.as_ref().map(|a| json!({
                            "name": a.name(),
                            "email": a.email(),
                        })),
                        "branch": o.branch.as_str(),
                        "path": o.path.to_string_lossy(),
                    })
                });
                json!({
                    "line_no": l.line_no,
                    "line_count": l.line_count,
                    "text": l.text,
                    "origin": origin,
                })
            })
            .collect();
        emit(
            out_json,
            json!({
                "path": result.path.to_string_lossy(),
                "snapshot": result.snapshot.as_str(),
                "lines": lines,
            }),
        )
    })
}
