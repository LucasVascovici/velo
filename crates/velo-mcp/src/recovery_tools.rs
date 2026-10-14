//! Tools an agent needs to recover on its own.
//!
//! Undo and redo step the current branch back and forward, tags name a
//! snapshot worth finding again, `velo_conflicts` shows a stopped merge as data
//! and `velo_find` queries by metadata. None of them rewrites history that was
//! pushed, and none takes a force argument: the safety stance is the tool
//! surface, so squash, rebase, compact, retention and force-push are not here.

use serde_json::{json, Value};
use velo_core::commands::{history, redo, resolve, tag, undo};
use velo_core::{BranchName, TagName};
use velo_merge::Decision;

use crate::tools::{bad, obj_schema, opt_str, req_str, snapshot_arg, Context, ToolError};

/// Tools that need a working tree, with what to call in store-only mode.
pub(crate) const NEEDS_WORKING_TREE: [(&str, &str); 3] = [
    ("velo_undo", "velo_history"),
    ("velo_redo", "velo_history"),
    ("velo_conflicts", "velo_tree_at"),
];

/// The schemas of the recovery tools.
pub(crate) fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "velo_undo",
            "description": "Undo the last snapshot on the current branch: it is shelved, not destroyed, and velo_redo brings it back. Refuses when the working tree has unsaved changes or another snapshot builds on it; there is no force.",
            "inputSchema": obj_schema(json!({}), json!([]))
        }),
        json!({
            "name": "velo_redo",
            "description": "Bring back the snapshot the last velo_undo shelved on the current branch.",
            "inputSchema": obj_schema(json!({}), json!([]))
        }),
        json!({
            "name": "velo_tag",
            "description": "List, create or delete tags. 'create' tags 'snapshot' (default the current position) and refuses a name that is taken; there is no overwrite.",
            "inputSchema": obj_schema(json!({
                "action": {"type": "string", "enum": ["list", "create", "delete"]},
                "name": {"type": "string", "description": "Tag name; required for create and delete."},
                "snapshot": {"type": "string", "description": "Snapshot spec to tag."}
            }), json!(["action"]))
        }),
        json!({
            "name": "velo_conflicts",
            "description": "The conflicts of a merge that stopped, as data: per path the ancestor, ours and theirs objects, and each hunk with its sides and decision. Empty when nothing is in conflict.",
            "inputSchema": obj_schema(json!({}), json!([]))
        }),
        json!({
            "name": "velo_find",
            "description": "Find snapshots by metadata, newest first. Every filter must match. Use it to find what a run or an evaluation produced.",
            "inputSchema": obj_schema(json!({
                "filters": {"type": "array", "minItems": 1,
                            "description": "[namespace, key, value] triples; give [namespace, key] to match any value.",
                            "items": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 3}},
                "branch": {"type": "string", "description": "Only snapshots reachable from this branch."}
            }), json!(["filters"]))
        }),
    ]
}

pub(crate) fn undo_tool(ctx: &Context<'_>) -> Result<Value, ToolError> {
    let guard = ctx.write()?;
    let outcome = undo::run(&guard);
    drop(guard);
    let o = outcome?;
    Ok(json!({
        "undone": o.snapshot,
        "message": o.message,
        "now_at": o.now_at,
        "cleared_working_tree": o.cleared_working_tree(),
    }))
}

pub(crate) fn redo_tool(ctx: &Context<'_>) -> Result<Value, ToolError> {
    let guard = ctx.write()?;
    let outcome = redo::run(&guard);
    drop(guard);
    let o = outcome?;
    Ok(json!({"restored": o.snapshot, "message": o.message}))
}

pub(crate) fn tag_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let action = req_str(args, "action")?;
    let name = opt_str(args, "name")?;
    let need_name = || name.ok_or_else(|| bad(format!("'name' is required for '{action}'")));
    match action {
        "list" => {
            let tags: Vec<Value> = tag::list(ctx.repo)?
                .iter()
                .map(|t| {
                    json!({"name": t.name.as_str(), "snapshot": t.snapshot.as_str(),
                           "message": t.message})
                })
                .collect();
            Ok(json!({"tags": tags}))
        }
        "create" => {
            let name: TagName = need_name()?.parse()?;
            let at = opt_str(args, "snapshot")?
                .map(|s| snapshot_arg(ctx.repo, s))
                .transpose()?;
            let guard = ctx.write()?;
            // `force` is never true here, whatever the caller asks.
            let outcome = tag::create(&guard, &name, at.as_ref(), false);
            drop(guard);
            let c = outcome?;
            Ok(json!({"created": c.name.as_str(), "snapshot": c.snapshot.as_str()}))
        }
        "delete" => {
            let name: TagName = need_name()?.parse()?;
            let guard = ctx.write()?;
            let outcome = tag::delete(&guard, &name);
            drop(guard);
            outcome?;
            Ok(json!({"deleted": name.as_str()}))
        }
        other => Err(bad(format!(
            "unknown action '{other}'; use list, create or delete"
        ))),
    }
}

fn decision_name(d: &Decision) -> &'static str {
    match d {
        Decision::Ours => "ours",
        Decision::Theirs => "theirs",
        Decision::BothOursFirst => "both_ours_first",
        Decision::BothTheirsFirst => "both_theirs_first",
        _ => "manual",
    }
}

/// The same document `velo conflicts --json` prints.
pub(crate) fn conflicts_tool(ctx: &Context<'_>) -> Result<Value, ToolError> {
    let repo = ctx.repo;
    let source = resolve::merge_source(repo);
    let mut files = Vec::new();
    for cf in resolve::list_conflicts(repo)? {
        let missing = resolve::missing_clean_changes(repo, &cf)?;
        let s = resolve::open_session(repo, cf)?;
        let hunks: Vec<Value> = s
            .hunks
            .iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "ancestor_start": h.ancestor_start,
                    "ancestor_end": h.ancestor_end,
                    "context_before": h.context_before,
                    "ours": h.ours,
                    "theirs": h.theirs,
                    "context_after": h.context_after,
                    "decision": h.decision.as_ref().map(decision_name),
                })
            })
            .collect();
        files.push(json!({
            "path": s.file.path,
            "ancestor": s.file.ancestor_hash,
            "ours": s.file.our_hash,
            "theirs": s.file.their_hash,
            "hunks": hunks,
            "clean_changes_missing": missing.len(),
            "clean_changes_missing_lines": missing,
        }));
    }
    Ok(json!({"source": source, "files": files}))
}

pub(crate) fn find_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let items = match args.get("filters") {
        Some(Value::Array(a)) if !a.is_empty() => a,
        _ => {
            return Err(bad(
                "'filters' must be a non-empty array of [namespace, key, value]",
            ))
        }
    };
    let mut wanted: Vec<(String, String, Option<String>)> = Vec::new();
    for item in items {
        let parts: Option<Vec<&str>> = item
            .as_array()
            .map(|a| a.iter().map(Value::as_str).collect())
            .unwrap_or(None);
        match parts.as_deref() {
            Some([ns, key]) => wanted.push(((*ns).into(), (*key).into(), None)),
            Some([ns, key, value]) => {
                wanted.push(((*ns).into(), (*key).into(), Some((*value).into())))
            }
            _ => {
                return Err(bad(
                    "each filter must be [namespace, key, value] or [namespace, key], all strings",
                ))
            }
        }
    }
    let filters: Vec<history::MetaFilter<'_>> = wanted
        .iter()
        .map(|(ns, k, v)| match v {
            Some(v) => history::MetaFilter::equals(ns, k, v),
            None => history::MetaFilter::has(ns, k),
        })
        .collect();
    let branch = opt_str(args, "branch")?
        .map(|b| b.parse::<BranchName>())
        .transpose()?;
    let entries = match &branch {
        None => ctx.repo.find_snapshots(&filters)?,
        Some(b) => {
            history::run(
                ctx.repo,
                history::Options {
                    branch: Some(b),
                    meta: &filters,
                    ..Default::default()
                },
            )?
            .entries
        }
    };
    let mut out = Vec::new();
    for e in &entries {
        let mut o = json!({
            "snapshot": e.hash.as_str(),
            "message": e.message,
            "created_at": e.created_at.to_rfc3339(),
            "branch": e.branch.as_str(),
        });
        if let Some(run) = ctx.repo.snapshot_meta(&e.hash)?.get("mcp", "run") {
            o["run"] = json!(run);
        }
        out.push(o);
    }
    Ok(json!({"snapshots": out}))
}
