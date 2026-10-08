//! The tool surface: schemas and handlers.
//!
//! Handlers take and return `serde_json::Value`. There is deliberately no
//! `force` argument anywhere: an agent that hits a dirty tree is told so and
//! must save or discard on purpose, never by a flag.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use velo_core::commands::{
    blame, branches, diff, history, merge, resolve_snapshot_id, restore, save, status, switch,
};
use velo_core::{Author, BranchName, Error, Repo, SnapshotId, SnapshotMeta};

/// Namespaces an agent may not write: `velo` is reserved by the core, `mcp`
/// carries the server's own run identity and would otherwise be forgeable.
const REFUSED_NAMESPACES: [&str; 2] = ["velo", "mcp"];

/// Why a tool call failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum ToolError {
    /// The arguments were wrong: a JSON-RPC invalid-params error.
    Params(String),
    /// velo refused or failed: a tool result with `isError: true`.
    Velo(Error),
}

impl From<Error> for ToolError {
    fn from(e: Error) -> Self {
        ToolError::Velo(e)
    }
}

fn bad(msg: impl Into<String>) -> ToolError {
    ToolError::Params(msg.into())
}

/// What a handler needs to know about the caller.
pub struct Context<'a> {
    pub repo: &'a Repo,
    pub run: &'a str,
    pub client: &'a str,
    /// Author override from the environment, as `(name, email)`.
    pub author: Option<&'a (String, Option<String>)>,
}

/// The variant name of a velo error, taken from its `Debug` form so that new
/// variants are named correctly without this crate being updated.
pub fn variant_name(e: &Error) -> String {
    format!("{e:?}")
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// The `tools/list` payload.
pub fn list() -> Value {
    let obj = |props: Value, required: Value| json!({"type": "object", "properties": props, "required": required});
    let strs = json!({"type": "array", "items": {"type": "string"}});
    json!([
        {
            "name": "velo_save",
            "description": "Save a snapshot of the working tree. Records this run's identity in metadata.",
            "inputSchema": obj(json!({
                "message": {"type": "string", "description": "Snapshot message."},
                "paths": strs,
                "meta": {"type": "object", "description": "Extra metadata as {namespace: {key: value}}. The 'velo' and 'mcp' namespaces are reserved.",
                         "additionalProperties": {"type": "object", "additionalProperties": {"type": "string"}}},
                "tool_call_id": {"type": "string", "description": "Your id for this tool call, recorded as mcp/tool_call_id."}
            }), json!(["message"]))
        },
        {
            "name": "velo_restore",
            "description": "Restore the working tree (or some paths) to a snapshot. Refuses if unsaved changes would be overwritten; there is no force.",
            "inputSchema": obj(json!({
                "snapshot": {"type": "string", "description": "Snapshot id, prefix, branch or tag."},
                "paths": strs
            }), json!(["snapshot"]))
        },
        {
            "name": "velo_status",
            "description": "Show the branch, position and unsaved changes.",
            "inputSchema": obj(json!({}), json!([]))
        },
        {
            "name": "velo_diff",
            "description": "Diff two snapshots, or a snapshot against the working tree. Defaults: from = current position, to = working tree.",
            "inputSchema": obj(json!({
                "from": {"type": "string"},
                "to": {"type": "string"},
                "paths": strs
            }), json!([]))
        },
        {
            "name": "velo_history",
            "description": "List snapshots, newest first, optionally filtered by metadata.",
            "inputSchema": obj(json!({
                "limit": {"type": "integer", "minimum": 1, "default": 20},
                "branch": {"type": "string"},
                "from": {"type": "string"},
                "paths": strs,
                "meta": {"type": "array", "items": obj(json!({
                    "namespace": {"type": "string"},
                    "key": {"type": "string"},
                    "value": {"type": "string", "description": "Omit to match any value."}
                }), json!(["namespace", "key"]))}
            }), json!([]))
        },
        {
            "name": "velo_metadata",
            "description": "Show the author and all metadata recorded on a snapshot.",
            "inputSchema": obj(json!({"snapshot": {"type": "string"}}), json!(["snapshot"]))
        },
        {
            "name": "velo_branch",
            "description": "List, create or switch branches; use one branch per attempt. 'create' makes the branch (at 'at', default the current position) without switching. 'switch' moves to the branch and creates it if it does not exist, as the CLI does; it refuses on a dirty tree (DirtyWorkingTree) and has no force.",
            "inputSchema": obj(json!({
                "action": {"type": "string", "enum": ["list", "create", "switch"]},
                "name": {"type": "string", "description": "Branch name; required for create and switch."},
                "at": {"type": "string", "description": "Snapshot spec to create the branch at."}
            }), json!(["action"]))
        },
        {
            "name": "velo_merge_plan",
            "description": "Plan merging 'source' into the current branch. Writes nothing. Conflicts carry the three texts (omitted for binary content). Then call velo_merge_apply, with resolutions for every conflict.",
            "inputSchema": obj(json!({
                "source": {"type": "string", "description": "Snapshot id, prefix, branch or tag to merge in."}
            }), json!(["source"]))
        },
        {
            "name": "velo_merge_apply",
            "description": "Apply a merge of 'source' into the current branch and record it as a two-parent snapshot. Requires a clean working tree (DirtyWorkingTree otherwise). The plan is recomputed, so nothing can be stale; unresolved conflicts return Conflicts. Updates the working tree to the merge. Records this run in metadata.",
            "inputSchema": obj(json!({
                "source": {"type": "string"},
                "message": {"type": "string", "description": "Merge snapshot message."},
                "resolutions": {"type": "object", "description": "Per conflicted path: \"ours\", \"theirs\", \"delete\" or {\"content\": \"text\"}.",
                               "additionalProperties": {}},
                "tool_call_id": {"type": "string", "description": "Your id for this tool call, recorded as mcp/tool_call_id."}
            }), json!(["source", "message"]))
        },
        {
            "name": "velo_blame",
            "description": "Attribute each line of a file to the snapshot that last changed it; origin.run names the agent run that saved it.",
            "inputSchema": obj(json!({
                "path": {"type": "string"},
                "at": {"type": "string", "description": "Snapshot spec; default the current position."},
                "start_line": {"type": "integer", "minimum": 1},
                "end_line": {"type": "integer", "minimum": 1, "description": "Inclusive."}
            }), json!(["path"]))
        }
    ])
}

/// Dispatch one tool call.
pub fn call(ctx: &Context<'_>, name: &str, args: &Value) -> Result<Value, ToolError> {
    if !args.is_object() {
        return Err(bad("arguments must be an object"));
    }
    match name {
        "velo_save" => save_tool(ctx, args),
        "velo_restore" => restore_tool(ctx, args),
        "velo_status" => status_tool(ctx),
        "velo_diff" => diff_tool(ctx, args),
        "velo_history" => history_tool(ctx, args),
        "velo_metadata" => metadata_tool(ctx, args),
        "velo_branch" => branch_tool(ctx, args),
        "velo_merge_plan" => merge_plan_tool(ctx, args),
        "velo_merge_apply" => merge_apply_tool(ctx, args),
        "velo_blame" => blame_tool(ctx, args),
        other => Err(bad(format!("unknown tool: {other}"))),
    }
}

// ── Argument helpers ─────────────────────────────────────────────────────────

fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(bad(format!("'{key}' must be a string"))),
    }
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    opt_str(args, key)?.ok_or_else(|| bad(format!("missing required argument '{key}'")))
}

fn paths_arg(args: &Value) -> Result<Vec<PathBuf>, ToolError> {
    match args.get("paths") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(PathBuf::from)
                    .ok_or_else(|| bad("'paths' must be an array of strings"))
            })
            .collect(),
        Some(_) => Err(bad("'paths' must be an array of strings")),
    }
}

fn path_refs(paths: &[PathBuf]) -> Vec<&Path> {
    paths.iter().map(PathBuf::as_path).collect()
}

fn snapshot_arg(repo: &Repo, spec: &str) -> Result<SnapshotId, ToolError> {
    Ok(resolve_snapshot_id(repo, spec)?)
}

// ── Write-tool identity ──────────────────────────────────────────────────────

fn author(ctx: &Context<'_>) -> Result<Author, ToolError> {
    Ok(match ctx.author {
        Some((name, Some(email))) => Author::with_email(name.clone(), email.clone())?,
        Some((name, None)) => Author::new(name.clone())?,
        None => Author::new(ctx.client)?,
    })
}

/// Agent-supplied metadata plus the server's own `mcp` identity.
///
/// The refusal of `velo` and `mcp` happens here, before velo is called, so it
/// reads as the caller's mistake (invalid params) rather than a velo failure.
fn build_meta(ctx: &Context<'_>, tool: &str, args: &Value) -> Result<SnapshotMeta, ToolError> {
    let mut meta = SnapshotMeta::new();
    match args.get("meta") {
        None | Some(Value::Null) => {}
        Some(Value::Object(namespaces)) => {
            for (ns, keys) in namespaces {
                if ns.is_empty() || REFUSED_NAMESPACES.contains(&ns.as_str()) {
                    return Err(bad(format!(
                        "metadata namespace '{ns}' is not writable by agents; use your own"
                    )));
                }
                let keys = keys
                    .as_object()
                    .ok_or_else(|| bad("'meta' must be {namespace: {key: value}}"))?;
                for (k, v) in keys {
                    let v = v
                        .as_str()
                        .ok_or_else(|| bad("metadata values must be strings"))?;
                    meta.set(ns.as_str(), k.as_str(), v)
                        .map_err(invalid_to_params)?;
                }
            }
        }
        Some(_) => return Err(bad("'meta' must be {namespace: {key: value}}")),
    }
    let mut own = |k: &str, v: &str| meta.set("mcp", k, v).map_err(ToolError::Velo);
    own("run", ctx.run)?;
    own("tool", tool)?;
    own("client", ctx.client)?;
    if let Some(id) = opt_str(args, "tool_call_id")? {
        own("tool_call_id", id)?;
    }
    Ok(meta)
}

fn invalid_to_params(e: Error) -> ToolError {
    match e {
        Error::InvalidInput { detail } => ToolError::Params(detail),
        other => ToolError::Velo(other),
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

fn save_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let message = req_str(args, "message")?;
    let paths = paths_arg(args)?;
    let refs = path_refs(&paths);
    let meta = build_meta(ctx, "velo_save", args)?;
    let author = author(ctx)?;
    let guard = ctx.repo.write()?;
    let outcome = save::run(
        &guard,
        Some(message),
        save::Options {
            paths: &refs,
            author: Some(&author),
            meta,
            ..Default::default()
        },
    );
    drop(guard);
    Ok(match outcome? {
        save::Outcome::Saved(r) => {
            let branch = fs::read_to_string(ctx.repo.root().join(".velo/HEAD"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "main".into());
            json!({
                "snapshot": r.hash.as_str(),
                "branch": branch,
                "files_changed": r.new_count + r.modified_count + r.deleted_count,
                "new": r.new_count,
                "modified": r.modified_count,
                "deleted": r.deleted_count,
            })
        }
        _ => json!({"saved": false}),
    })
}

fn restore_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let spec = req_str(args, "snapshot")?;
    let paths = paths_arg(args)?;
    let refs = path_refs(&paths);
    let id = snapshot_arg(ctx.repo, spec)?;
    let guard = ctx.repo.write()?;
    // `force` is never true here, whatever the caller asks.
    let outcome = restore::run(
        &guard,
        &id,
        restore::Options {
            force: false,
            paths: &refs,
            ..Default::default()
        },
    );
    drop(guard);
    Ok(match outcome? {
        restore::Outcome::AlreadyThere { snapshot } => {
            json!({"result": "already_there", "snapshot": snapshot})
        }
        restore::Outcome::Restored {
            snapshot,
            branch,
            message,
            files,
            ghosts_removed,
            ..
        } => json!({"result": "restored", "snapshot": snapshot, "branch": branch,
                    "message": message, "files": files, "removed": ghosts_removed}),
        restore::Outcome::RestoredPaths {
            snapshot, files, ..
        } => {
            json!({"result": "restored_paths", "snapshot": snapshot, "files": files})
        }
        restore::Outcome::NoMatchingPaths { snapshot } => {
            json!({"result": "no_matching_paths", "snapshot": snapshot})
        }
    })
}

fn status_tool(ctx: &Context<'_>) -> Result<Value, ToolError> {
    let s = status::run(ctx.repo, &[])?;
    Ok(json!({
        "branch": s.branch,
        "position": s.position,
        "position_message": s.position_message,
        "new_files": s.new_files,
        "modified": s.modified,
        "deleted": s.deleted,
        "conflicts": s.conflicts,
    }))
}

fn tagged(prefix: &str, text: &str) -> String {
    format!("{prefix}{text}")
}

fn hunks_json(hunks: &[diff::Hunk]) -> Value {
    hunks
        .iter()
        .map(|h| {
            let lines: Vec<String> = h
                .lines
                .iter()
                .map(|l| match l.tag {
                    diff::LineTag::Added => tagged("+", &l.text),
                    diff::LineTag::Removed => tagged("-", &l.text),
                    diff::LineTag::Context => tagged(" ", &l.text),
                })
                .collect();
            json!({"old_start": h.old_start, "old_count": h.old_count,
                   "new_start": h.new_start, "new_count": h.new_count, "lines": lines})
        })
        .collect()
}

fn diff_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let paths = paths_arg(args)?;
    let refs = path_refs(&paths);
    let from = match opt_str(args, "from")? {
        Some(spec) => snapshot_arg(ctx.repo, spec)?,
        None => {
            let position = status::run(ctx.repo, &[])?.position;
            let position = position.ok_or_else(|| {
                bad("no current position to diff from; pass 'from' or save first")
            })?;
            snapshot_arg(ctx.repo, &position)?
        }
    };
    let to = opt_str(args, "to")?
        .map(|spec| snapshot_arg(ctx.repo, spec))
        .transpose()?;
    let d = diff::between(ctx.repo, &from, to.as_ref(), &refs)?;
    let files: Vec<Value> = d
        .files
        .iter()
        .map(|f| match &f.change {
            diff::FileChange::Added { lines } => {
                let lines: Vec<String> = lines.iter().map(|l| tagged("+", l)).collect();
                json!({"path": f.path, "change": "added", "lines": lines})
            }
            diff::FileChange::Deleted => json!({"path": f.path, "change": "deleted"}),
            diff::FileChange::Modified { hunks } => {
                json!({"path": f.path, "change": "modified", "hunks": hunks_json(hunks)})
            }
            diff::FileChange::BinaryChanged { .. } => {
                json!({"path": f.path, "change": "binary"})
            }
            diff::FileChange::Renamed { from, hunks } => {
                json!({"path": f.path, "change": "renamed", "from": from,
                       "hunks": hunks_json(hunks)})
            }
        })
        .collect();
    Ok(json!({
        "from": from.as_str(),
        "to": to.as_ref().map(|t| t.as_str()),
        "files": files,
    }))
}

fn history_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let limit = match args.get("limit") {
        None | Some(Value::Null) => 20,
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| bad("'limit' must be a positive integer"))? as usize,
    };
    let branch = opt_str(args, "branch")?
        .map(|b| b.parse::<BranchName>())
        .transpose()?;
    let from = opt_str(args, "from")?
        .map(|s| snapshot_arg(ctx.repo, s))
        .transpose()?;
    let paths = paths_arg(args)?;
    let refs = path_refs(&paths);

    // `MetaFilter` borrows, so the owned strings must outlive the call.
    let mut wanted: Vec<(String, String, Option<String>)> = Vec::new();
    match args.get("meta") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for item in items {
                let ns = req_str(item, "namespace")?;
                let key = req_str(item, "key")?;
                let value = opt_str(item, "value")?;
                wanted.push((ns.into(), key.into(), value.map(String::from)));
            }
        }
        Some(_) => return Err(bad("'meta' must be an array of filters")),
    }
    let filters: Vec<history::MetaFilter<'_>> = wanted
        .iter()
        .map(|(ns, k, v)| match v {
            Some(v) => history::MetaFilter::equals(ns, k, v),
            None => history::MetaFilter::has(ns, k),
        })
        .collect();

    let h = history::run(
        ctx.repo,
        history::Options {
            branch: branch.as_ref(),
            from: from.as_ref(),
            paths: &refs,
            meta: &filters,
            limit: Some(limit),
            ..Default::default()
        },
    )?;
    let mut entries = Vec::new();
    for e in &h.entries {
        let mut o = Map::new();
        o.insert("snapshot".into(), json!(e.hash.as_str()));
        o.insert("message".into(), json!(e.message));
        o.insert("created_at".into(), json!(e.created_at.to_rfc3339()));
        o.insert("branch".into(), json!(e.branch.as_str()));
        o.insert(
            "parent".into(),
            json!(e.parent.as_ref().map(|p| p.as_str())),
        );
        o.insert(
            "merge_parent".into(),
            json!(e.merge_parent.as_ref().map(|p| p.as_str())),
        );
        o.insert("tag".into(), json!(e.tag.as_ref().map(|t| t.as_str())));
        if let Some(run) = ctx.repo.snapshot_meta(&e.hash)?.get("mcp", "run") {
            o.insert("run".into(), json!(run));
        }
        entries.push(Value::Object(o));
    }
    Ok(json!({"entries": entries}))
}

fn metadata_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let id = snapshot_arg(ctx.repo, req_str(args, "snapshot")?)?;
    let meta = ctx.repo.snapshot_meta(&id)?;
    let mut namespaces: Map<String, Value> = Map::new();
    for (ns, key, value) in meta.iter() {
        namespaces
            .entry(ns.to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("just inserted an object")
            .insert(key.to_string(), json!(value));
    }
    let author = meta.author().map(|a| {
        let mut o = Map::new();
        o.insert("name".into(), json!(a.name()));
        if let Some(e) = a.email() {
            o.insert("email".into(), json!(e));
        }
        Value::Object(o)
    });
    Ok(json!({"snapshot": id.as_str(), "author": author, "meta": namespaces}))
}

// ── Branches, merging, blame ─────────────────────────────────────────────────

fn branch_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let action = req_str(args, "action")?;
    let name = opt_str(args, "name")?;
    let need_name = || name.ok_or_else(|| bad(format!("'name' is required for '{action}'")));
    match action {
        "list" => {
            let list: Vec<Value> = branches::list(ctx.repo)?
                .iter()
                .map(|b| {
                    json!({"name": b.name.as_str(), "current": b.is_current,
                           "tip": b.tip.as_ref().map(|t| json!({
                               "snapshot": t.hash.as_str(), "message": t.message,
                               "created_at": t.created_at.to_rfc3339()}))})
                })
                .collect();
            Ok(json!({"branches": list}))
        }
        "create" => {
            let name = need_name()?.parse::<BranchName>()?;
            let at = opt_str(args, "at")?
                .map(|s| snapshot_arg(ctx.repo, s))
                .transpose()?;
            let guard = ctx.repo.write()?;
            let outcome = branches::create(&guard, &name, at.as_ref());
            drop(guard);
            outcome?;
            Ok(json!({"created": name.as_str(), "at": at.as_ref().map(|a| a.as_str())}))
        }
        "switch" => {
            let name = need_name()?;
            let guard = ctx.repo.write()?;
            // `force` is never true here, whatever the caller asks.
            let outcome = switch::run(&guard, name, false);
            drop(guard);
            Ok(match outcome? {
                switch::Outcome::AlreadyOn { branch } => {
                    json!({"result": "already_on", "branch": branch})
                }
                switch::Outcome::Switched { branch, at } => {
                    json!({"result": "switched", "branch": branch, "at": at})
                }
                switch::Outcome::StartedUnborn {
                    branch,
                    existing,
                    inherits,
                } => json!({"result": "started_unborn", "branch": branch,
                            "existing": existing, "inherits": inherits}),
            })
        }
        other => Err(bad(format!(
            "unknown action '{other}'; use list, create or switch"
        ))),
    }
}

/// The current branch and its tip, which a merge needs as `ours`.
fn current_tip(repo: &Repo) -> Result<(BranchName, SnapshotId), ToolError> {
    let current = branches::list(repo)?
        .into_iter()
        .find(|b| b.is_current)
        .ok_or_else(|| bad("no current branch"))?;
    match current.tip {
        Some(t) => Ok((current.name, t.hash)),
        None => Err(bad(format!(
            "branch '{}' has no snapshots yet; save before merging",
            current.name
        ))),
    }
}

fn action_name(a: merge::FileAction) -> &'static str {
    match a {
        merge::FileAction::Deleted => "deleted",
        merge::FileAction::Added => "added",
        merge::FileAction::Updated => "updated",
        merge::FileAction::AutoMerged => "auto_merged",
        merge::FileAction::KeptOurs => "kept_ours",
        merge::FileAction::Conflicted => "conflicted",
    }
}

fn merge_plan_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let spec = req_str(args, "source")?;
    let theirs = snapshot_arg(ctx.repo, spec)?;
    let (_, ours) = current_tip(ctx.repo)?;
    let plan = merge::plan(ctx.repo, &ours, &theirs)?;
    let text = |h: &Option<velo_core::ObjectHash>| -> Result<Option<String>, ToolError> {
        let Some(h) = h else { return Ok(None) };
        // Binary content is omitted: it would be noise, and unusable as text.
        Ok(String::from_utf8(ctx.repo.read_object(h)?).ok())
    };
    let mut files = Vec::new();
    for f in &plan.files {
        let mut o = Map::new();
        o.insert("path".into(), json!(f.path));
        o.insert("action".into(), json!(action_name(f.change.action())));
        if let merge::PlannedChange::Conflict { base, ours, theirs } = &f.change {
            let mut c = Map::new();
            for (k, h) in [
                ("base_text", base),
                ("ours_text", ours),
                ("theirs_text", theirs),
            ] {
                if let Some(t) = text(h)? {
                    c.insert(k.into(), json!(t));
                }
            }
            o.insert("conflict".into(), Value::Object(c));
        }
        files.push(Value::Object(o));
    }
    let clean = plan.is_clean();
    let apply_hint = if clean {
        format!(
            "Call velo_merge_apply {{\"source\": \"{spec}\", \"message\": \"<message>\"}} on a clean working tree."
        )
    } else {
        let paths: Vec<String> = plan
            .conflicts()
            .map(|f| format!("\"{}\": ...", f.path))
            .collect();
        format!(
            "Call velo_merge_apply {{\"source\": \"{spec}\", \"message\": \"<message>\", \"resolutions\": {{{}}}}} on a clean working tree; each resolution is \"ours\", \"theirs\", \"delete\" or {{\"content\": \"<text>\"}}.",
            paths.join(", ")
        )
    };
    Ok(json!({
        "base": plan.base.as_ref().map(|b| b.as_str()),
        "ours": ours.as_str(),
        "theirs": theirs.as_str(),
        "clean": clean,
        "files": files,
        "apply_hint": apply_hint,
    }))
}

fn resolutions_arg(args: &Value) -> Result<Vec<(String, merge::Resolution)>, ToolError> {
    let obj = match args.get("resolutions") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Object(o)) => o,
        Some(_) => return Err(bad("'resolutions' must be {path: resolution}")),
    };
    let mut out = Vec::new();
    for (path, r) in obj {
        let res = match r {
            Value::String(s) => match s.as_str() {
                "ours" => merge::Resolution::Ours,
                "theirs" => merge::Resolution::Theirs,
                "delete" => merge::Resolution::Delete,
                other => {
                    return Err(bad(format!(
                        "resolution for '{path}' must be ours, theirs, delete or {{content}}, not '{other}'"
                    )))
                }
            },
            Value::Object(o) => match o.get("content") {
                Some(Value::String(c)) => merge::Resolution::Content(c.clone().into_bytes()),
                _ => {
                    return Err(bad(format!(
                        "resolution for '{path}' needs a string 'content'"
                    )))
                }
            },
            _ => return Err(bad(format!("invalid resolution for '{path}'"))),
        };
        out.push((path.clone(), res));
    }
    Ok(out)
}

fn merge_apply_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let spec = req_str(args, "source")?;
    let message = req_str(args, "message")?;
    let resolutions = resolutions_arg(args)?;
    let meta = build_meta(ctx, "velo_merge_apply", args)?;
    let author = author(ctx)?;
    let theirs = snapshot_arg(ctx.repo, spec)?;
    let guard = ctx.repo.write()?;
    let result = (|| -> Result<Value, ToolError> {
        let s = status::run(ctx.repo, &[])?;
        if !s.is_clean() {
            let paths = s
                .new_files
                .iter()
                .chain(&s.modified)
                .chain(&s.deleted)
                .chain(&s.conflicts)
                .map(PathBuf::from)
                .collect();
            return Err(Error::DirtyWorkingTree { paths }.into());
        }
        let (branch, ours) = current_tip(ctx.repo)?;
        let plan = merge::plan(ctx.repo, &ours, &theirs)?;
        let id = merge::commit(
            &guard,
            merge::MergeCommit {
                branch: &branch,
                ours: &ours,
                theirs: &theirs,
                resolutions: &resolutions,
                message,
                meta,
                author: Some(&author),
                timestamp_ms: None,
            },
        )?;
        // The tree was clean and the new snapshot is a superset of the decided
        // changes, so this only brings the tree and position to the merge.
        restore::run(&guard, &id, restore::Options::default())?;
        Ok(
            json!({"snapshot": id.as_str(), "merged_from": theirs.as_str(),
                  "files_changed": plan.files.len()}),
        )
    })();
    drop(guard);
    result
}

fn blame_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let path = req_str(args, "path")?;
    let at = opt_str(args, "at")?
        .map(|s| snapshot_arg(ctx.repo, s))
        .transpose()?;
    let line = |key: &str| -> Result<Option<usize>, ToolError> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => Ok(Some(
                v.as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| bad(format!("'{key}' must be a positive integer")))?
                    as usize,
            )),
        }
    };
    let (start, end) = (line("start_line")?, line("end_line")?);
    let lines = if start.is_some() || end.is_some() {
        // 1-based inclusive in, half-open out.
        Some(start.unwrap_or(1)..end.map_or(usize::MAX, |e| e.saturating_add(1)))
    } else {
        None
    };
    let b = blame::run(
        ctx.repo,
        Path::new(path),
        blame::Options {
            at: at.as_ref(),
            lines,
            ..Default::default()
        },
    )?;
    // One metadata read per distinct origin snapshot, not per line.
    let mut runs: std::collections::HashMap<String, Option<String>> = Default::default();
    let mut out = Vec::new();
    for l in &b.lines {
        let origin = match &l.origin {
            None => Value::Null,
            Some(o) => {
                let key = o.hash.as_str().to_string();
                if !runs.contains_key(&key) {
                    let run = ctx
                        .repo
                        .snapshot_meta(&o.hash)?
                        .get("mcp", "run")
                        .map(String::from);
                    runs.insert(key.clone(), run);
                }
                json!({
                    "snapshot": o.hash.as_str(),
                    "message": o.message,
                    "author": o.author.as_ref().map(|a| a.name()),
                    "branch": o.branch.as_str(),
                    "created_at": o.created_at.to_rfc3339(),
                    "run": runs[&key],
                })
            }
        };
        out.push(json!({"line_no": l.line_no, "text": l.text, "origin": origin}));
    }
    Ok(json!({"path": path, "snapshot": b.snapshot.as_str(), "lines": out}))
}
