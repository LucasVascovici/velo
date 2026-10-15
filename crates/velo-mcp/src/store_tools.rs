//! Tools that work on trees by id, with no working tree on disk.
//!
//! This is the embedder half of the core API (`save_tree`, `tree_at`,
//! `read_object`) exposed as tools. An agent that has no checkout sends the
//! content in the call and gets snapshot ids back. These tools are listed in
//! both modes: a client with a checkout may want to write a tree it never
//! materialised, too.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use velo_core::tree::{FileKind, SaveTree, TreeEntry};
use velo_core::{BranchName, SnapshotId};

use crate::tools::{
    author, bad, build_meta, obj_schema, opt_str, req_str, snapshot_arg, Context, ToolError,
};

/// Tools that need a working tree, with what to call in store-only mode.
pub(crate) const NEEDS_WORKING_TREE: [(&str, &str); 5] = [
    ("velo_save", "velo_save_tree"),
    ("velo_restore", "velo_read_file or velo_tree_at"),
    ("velo_status", "velo_tree_at or velo_history"),
    ("velo_diff", "velo_tree_at"),
    ("velo_merge_apply", "velo_save_tree with merge_parent"),
];

/// Files `velo_tree_at` returns when the call gives no `limit`.
pub(crate) const TREE_DEFAULT_LIMIT: usize = 5000;
/// Bytes `velo_read_file` returns when the call gives no `max_bytes`.
pub(crate) const READ_DEFAULT_MAX_BYTES: usize = 1024 * 1024;

/// An optional integer argument of at least `min`.
fn usize_arg(args: &Value, key: &str, min: usize) -> Result<Option<usize>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= min as u64)
            .map(|n| Some(n as usize))
            .ok_or_else(|| bad(format!("'{key}' must be an integer of at least {min}"))),
    }
}

/// The schemas of the store tools.
pub(crate) fn list() -> Vec<Value> {
    let encoding = json!({"type": "string", "enum": ["utf8", "base64"], "default": "utf8"});
    vec![
        json!({
            "name": "velo_save_tree",
            "description": "Record a snapshot from content in this call, with no working tree. 'files' maps paths to content (null deletes the path). By default the files are laid over the tree of 'base' (default: the parent), so you send only what changed; with replace=true 'files' is the whole tree. Returns the snapshot id. Records this run in metadata.",
            "inputSchema": obj_schema(json!({
                "branch": {"type": "string", "description": "Branch to record the snapshot on."},
                "message": {"type": "string"},
                "files": {"type": "object", "description": "{path: content or null}.",
                          "additionalProperties": {"type": ["string", "null"]}},
                "encoding": {"type": "string", "enum": ["utf8", "base64"], "default": "utf8",
                             "description": "How every value in 'files' is encoded."},
                "base": {"type": "string", "description": "Snapshot spec whose tree 'files' is laid over. Default: the parent."},
                "parent": {"type": "string", "description": "Snapshot spec this one follows. Default: the branch tip; on an unborn branch, a root snapshot."},
                "replace": {"type": "boolean", "description": "True means 'files' is the whole tree and 'base' is ignored."},
                "merge_parent": {"type": "string", "description": "Second parent, when recording a merge."},
                "meta": {"type": "object", "description": "Extra metadata as {namespace: {key: value}}. The 'velo' and 'mcp' namespaces are reserved.",
                         "additionalProperties": {"type": "object", "additionalProperties": {"type": "string"}}},
                "tool_call_id": {"type": "string", "description": "Your id for this tool call, recorded as mcp/tool_call_id."}
            }), json!(["branch", "message", "files"]))
        }),
        json!({
            "name": "velo_tree_at",
            "description": "List the files of a snapshot as {files: [{path, object, kind}], total, truncated}. A large tree comes back in pages of 'limit' files; use 'offset' for the next page.",
            "inputSchema": obj_schema(json!({
                "snapshot": {"type": "string", "description": "Snapshot id, prefix, branch or tag."},
                "limit": {"type": "integer", "minimum": 1, "default": TREE_DEFAULT_LIMIT, "description": "Most files to return."},
                "offset": {"type": "integer", "minimum": 0, "default": 0, "description": "Files to skip, in path order."}
            }), json!(["snapshot"]))
        }),
        json!({
            "name": "velo_read_file",
            "description": "Read one file of a snapshot. With utf8 (the default), content that is not valid UTF-8 comes back base64 with encoding 'base64'. At most 'max_bytes' bytes (default 1 MiB) are returned; a longer file has truncated: true and a hint to read on with 'offset'.",
            "inputSchema": obj_schema(json!({
                "snapshot": {"type": "string"},
                "path": {"type": "string"},
                "encoding": encoding,
                "max_bytes": {"type": "integer", "minimum": 1, "default": READ_DEFAULT_MAX_BYTES, "description": "Most bytes of the file to return."},
                "offset": {"type": "integer", "minimum": 0, "default": 0, "description": "Byte offset to start at; use the next_offset of a truncated result."}
            }), json!(["snapshot", "path"]))
        }),
    ]
}

fn kind_name(k: FileKind) -> &'static str {
    match k {
        FileKind::Regular => "regular",
        FileKind::Executable => "executable",
        FileKind::Symlink => "symlink",
    }
}

/// True for base64, false for utf8.
fn encoding_arg(args: &Value) -> Result<bool, ToolError> {
    match opt_str(args, "encoding")? {
        None | Some("utf8") => Ok(false),
        Some("base64") => Ok(true),
        Some(other) => Err(bad(format!(
            "unknown encoding '{other}'; use utf8 or base64"
        ))),
    }
}

fn spec_arg(ctx: &Context<'_>, args: &Value, key: &str) -> Result<Option<SnapshotId>, ToolError> {
    opt_str(args, key)?
        .map(|s| snapshot_arg(ctx.repo, s))
        .transpose()
}

pub(crate) fn save_tree_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let branch: BranchName = req_str(args, "branch")?.parse()?;
    let message = req_str(args, "message")?;
    let is_b64 = encoding_arg(args)?;
    let replace = match args.get("replace") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad("'replace' must be a boolean")),
    };
    let files = args
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("'files' must be an object of {path: content or null}"))?;

    // Decode everything before touching the store, so a bad value writes nothing.
    let mut provided: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    let mut deleted: Vec<String> = Vec::new();
    for (path, v) in files {
        match v {
            Value::Null => deleted.push(path.clone()),
            Value::String(s) if is_b64 => {
                let bytes = STANDARD
                    .decode(s)
                    .map_err(|e| bad(format!("'{path}' is not valid base64: {e}")))?;
                provided.insert(path, bytes);
            }
            Value::String(s) => {
                provided.insert(path, s.clone().into_bytes());
            }
            _ => return Err(bad(format!("'{path}' must be a string or null"))),
        }
    }

    let meta = build_meta(ctx, "velo_save_tree", args)?;
    let author = author(ctx)?;
    let parent = match spec_arg(ctx, args, "parent")? {
        Some(p) => Some(p),
        None => ctx.repo.branch_tip(&branch)?,
    };
    let merge_parent = spec_arg(ctx, args, "merge_parent")?;
    let base = if replace {
        None
    } else {
        spec_arg(ctx, args, "base")?.or_else(|| parent.clone())
    };

    let mut tree: BTreeMap<String, TreeEntry> = BTreeMap::new();
    if let Some(base) = &base {
        for f in ctx.repo.tree_at(base)? {
            tree.insert(f.path.clone(), TreeEntry::stored(f.path, f.object, f.kind));
        }
    }
    let changed: Vec<String> = provided.keys().map(|p| p.to_string()).collect();
    for (path, bytes) in provided {
        tree.insert(path.to_string(), TreeEntry::file(path, bytes));
    }
    // A JSON object has one value per key, so a path is never both given and
    // deleted; the order of these two steps does not matter.
    for path in &deleted {
        tree.remove(path);
    }

    let guard = ctx.write()?;
    let id = guard.save_tree(SaveTree {
        branch: &branch,
        parent: parent.as_ref(),
        merge_parent: merge_parent.as_ref(),
        message,
        entries: tree.into_values().collect(),
        meta,
        author: Some(&author),
        renames: &[],
        timestamp_ms: None,
    });
    drop(guard);
    let id = id?;
    deleted.sort();
    Ok(json!({
        "snapshot": id.as_str(),
        "parent": parent.as_ref().map(|p| p.as_str()),
        "changed": changed,
        "deleted": deleted,
    }))
}

pub(crate) fn tree_at_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let limit = usize_arg(args, "limit", 1)?.unwrap_or(TREE_DEFAULT_LIMIT);
    let offset = usize_arg(args, "offset", 0)?.unwrap_or(0);
    let id = snapshot_arg(ctx.repo, req_str(args, "snapshot")?)?;
    let all = ctx.repo.tree_at(&id)?;
    let total = all.len();
    let files: Vec<Value> = all
        .iter()
        .skip(offset)
        .take(limit)
        .map(|f| json!({"path": f.path, "object": f.object.as_str(), "kind": kind_name(f.kind)}))
        .collect();
    let truncated = offset.saturating_add(limit) < total;
    let mut out = json!({"files": files, "total": total, "truncated": truncated});
    if truncated {
        let next = offset + limit;
        out["next_offset"] = json!(next);
        out["hint"] = json!(format!(
            "Showing files {}..{next} of {total}. Call velo_tree_at again with \"offset\": {next} for the next page.",
            offset + 1
        ));
    }
    Ok(out)
}

pub(crate) fn read_file_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
    let max = usize_arg(args, "max_bytes", 1)?.unwrap_or(READ_DEFAULT_MAX_BYTES);
    let offset = usize_arg(args, "offset", 0)?.unwrap_or(0);
    let id = snapshot_arg(ctx.repo, req_str(args, "snapshot")?)?;
    let path = req_str(args, "path")?;
    let want_b64 = encoding_arg(args)?;
    let file = ctx
        .repo
        .tree_at(&id)?
        .into_iter()
        .find(|f| f.path == path)
        .ok_or_else(|| bad(format!("no file '{path}' in snapshot {}", id.as_str())))?;
    let bytes = ctx.repo.read_object(&file.object)?;
    let size = bytes.len();
    if offset > size {
        return Err(bad(format!(
            "'offset' {offset} is past the end of '{path}' ({size} bytes)"
        )));
    }
    let mut end = offset.saturating_add(max).min(size);
    // Text is cut on a character boundary so a page is still valid UTF-8.
    let text = if want_b64 {
        None
    } else {
        std::str::from_utf8(&bytes).ok()
    };
    if let Some(t) = text {
        if !t.is_char_boundary(offset) {
            return Err(bad(format!(
                "'offset' {offset} is inside a UTF-8 character of '{path}'; use the next_offset of the previous page"
            )));
        }
        while end > offset && !t.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && offset < size {
            return Err(bad(format!(
                "'max_bytes' {max} is smaller than the character at offset {offset}; raise it"
            )));
        }
    }
    let truncated = end < size;
    let mut out = match text {
        Some(t) => json!({"content": &t[offset..end], "encoding": "utf8"}),
        None => json!({"content": STANDARD.encode(&bytes[offset..end]), "encoding": "base64"}),
    };
    out["size"] = json!(size);
    out["offset"] = json!(offset);
    out["truncated"] = json!(truncated);
    if truncated {
        out["next_offset"] = json!(end);
        out["hint"] = json!(format!(
            "Returned bytes {offset}..{end} of {size}. Call velo_read_file again with \"offset\": {end} for the rest, or raise 'max_bytes'."
        ));
    }
    Ok(out)
}
