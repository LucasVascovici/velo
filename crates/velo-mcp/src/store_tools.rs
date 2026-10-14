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
            "description": "List the files of a snapshot as {path, object, kind}.",
            "inputSchema": obj_schema(json!({
                "snapshot": {"type": "string", "description": "Snapshot id, prefix, branch or tag."}
            }), json!(["snapshot"]))
        }),
        json!({
            "name": "velo_read_file",
            "description": "Read one file of a snapshot. With utf8 (the default), content that is not valid UTF-8 comes back base64 with encoding 'base64'.",
            "inputSchema": obj_schema(json!({
                "snapshot": {"type": "string"},
                "path": {"type": "string"},
                "encoding": encoding
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
    let id = snapshot_arg(ctx.repo, req_str(args, "snapshot")?)?;
    let files: Vec<Value> = ctx
        .repo
        .tree_at(&id)?
        .iter()
        .map(|f| json!({"path": f.path, "object": f.object.as_str(), "kind": kind_name(f.kind)}))
        .collect();
    Ok(json!(files))
}

pub(crate) fn read_file_tool(ctx: &Context<'_>, args: &Value) -> Result<Value, ToolError> {
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
    if !want_b64 {
        match String::from_utf8(bytes) {
            Ok(text) => return Ok(json!({"content": text, "encoding": "utf8"})),
            Err(e) => {
                return Ok(
                    json!({"content": STANDARD.encode(e.into_bytes()), "encoding": "base64"}),
                )
            }
        }
    }
    Ok(json!({"content": STANDARD.encode(bytes), "encoding": "base64"}))
}
