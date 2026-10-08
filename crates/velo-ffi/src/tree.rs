//! Building a tree, saving it, and reading trees and snapshots back.
//!
//! A snapshot is a whole tree, so `velo_save_tree` takes a `VeloTree` holding
//! every file. Everything else about the save — branch, parent, message, author,
//! metadata, renames, timestamp — arrives as one JSON document, which keeps the
//! signature stable as options are added.

use std::ffi::c_char;
use std::path::PathBuf;

use serde_json::{json, Map, Value};
use velo_core::commands::resolve_snapshot_id;
use velo_core::tree::{FileKind, SaveTree, TreeEntry};
use velo_core::{Author, BranchName, ObjectHash, SnapshotId, SnapshotMeta};

use crate::error::{Failure, VELO_ERR_INVALID_INPUT};
use crate::repo::VeloRepo;
use crate::{
    bad_json, emit, mut_arg, opt_str, ref_arg, req_str, run, snapshot_arg, str_arg, to_c_string,
    Res,
};

/// `kind` for an ordinary file.
pub const VELO_KIND_REGULAR: i32 = 0;
/// `kind` for a file that should be executable when written out.
pub const VELO_KIND_EXECUTABLE: i32 = 1;
/// `kind` for a symbolic link; its content is the target path.
pub const VELO_KIND_SYMLINK: i32 = 2;

/// The complete contents of a snapshot being assembled. Opaque to C.
pub struct VeloTree {
    entries: Vec<TreeEntry>,
}

fn kind_of(kind: i32) -> Res<FileKind> {
    match kind {
        VELO_KIND_REGULAR => Ok(FileKind::Regular),
        VELO_KIND_EXECUTABLE => Ok(FileKind::Executable),
        VELO_KIND_SYMLINK => Ok(FileKind::Symlink),
        other => Err(Failure::new(
            VELO_ERR_INVALID_INPUT,
            format!("{other} is not a file kind (0 regular, 1 executable, 2 symlink)."),
        )),
    }
}

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Regular => "regular",
        FileKind::Executable => "executable",
        FileKind::Symlink => "symlink",
    }
}

/// Create an empty tree. Free it with `velo_tree_free`.
#[no_mangle]
pub unsafe extern "C" fn velo_tree_new(out: *mut *mut VeloTree) -> i32 {
    run(|| {
        *mut_arg(out, "out")? = Box::into_raw(Box::new(VeloTree {
            entries: Vec::new(),
        }));
        Ok(())
    })
}

/// Release a tree. NULL is a no-op.
#[no_mangle]
pub unsafe extern "C" fn velo_tree_free(tree: *mut VeloTree) {
    if !tree.is_null() {
        let _ =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(Box::from_raw(tree))));
    }
}

/// Add a file whose bytes are `data[0..len]`. The bytes are copied, so the
/// caller keeps ownership of its buffer. `data` may be NULL only when `len` is 0.
#[no_mangle]
pub unsafe extern "C" fn velo_tree_add_file(
    tree: *mut VeloTree,
    path: *const c_char,
    data: *const u8,
    len: usize,
    kind: i32,
) -> i32 {
    run(|| {
        let tree = mut_arg(tree, "tree")?;
        let path = str_arg(path, "path")?;
        let kind = kind_of(kind)?;
        let bytes = if len == 0 {
            Vec::new()
        } else {
            ref_arg(data, "data")?;
            std::slice::from_raw_parts(data, len).to_vec()
        };
        let mut entry = TreeEntry::file(path, bytes);
        entry.kind = kind;
        tree.entries.push(entry);
        Ok(())
    })
}

/// Add a file by naming an object the store already holds (the `object` field
/// of `velo_tree_at`), so an unchanged file costs nothing to carry forward.
#[no_mangle]
pub unsafe extern "C" fn velo_tree_add_stored(
    tree: *mut VeloTree,
    path: *const c_char,
    object: *const c_char,
    kind: i32,
) -> i32 {
    run(|| {
        let tree = mut_arg(tree, "tree")?;
        let path = str_arg(path, "path")?;
        let object: ObjectHash = str_arg(object, "object")?.parse()?;
        tree.entries
            .push(TreeEntry::stored(path, object, kind_of(kind)?));
        Ok(())
    })
}

/// The `meta` member of an options document: `{namespace: {key: value}}`.
pub(crate) fn parse_meta(doc: &Map<String, Value>) -> Res<SnapshotMeta> {
    let mut meta = SnapshotMeta::new();
    match doc.get("meta") {
        None | Some(Value::Null) => {}
        Some(Value::Object(namespaces)) => {
            for (namespace, keys) in namespaces {
                let keys = keys
                    .as_object()
                    .ok_or_else(|| bad_json("`meta` must map namespace -> key -> string"))?;
                for (key, val) in keys {
                    let val = val
                        .as_str()
                        .ok_or_else(|| bad_json("`meta` values must be strings"))?;
                    meta.set(namespace.as_str(), key.as_str(), val)?;
                }
            }
        }
        Some(_) => return Err(bad_json("`meta` must be an object")),
    }
    Ok(meta)
}

/// The `author` member of an options document: `{"name", "email"?}`.
pub(crate) fn parse_author(doc: &Map<String, Value>) -> Res<Option<Author>> {
    Ok(match doc.get("author") {
        None | Some(Value::Null) => None,
        Some(Value::Object(a)) => {
            let name = req_str(a, "name").map_err(|_| bad_json("`author.name` is required"))?;
            Some(match opt_str(a, "email")? {
                Some(email) => Author::with_email(name, email)?,
                None => Author::new(name)?,
            })
        }
        Some(_) => return Err(bad_json("`author` must be an object")),
    })
}

/// The `timestamp_ms` member of an options document.
pub(crate) fn parse_timestamp(doc: &Map<String, Value>) -> Res<Option<i64>> {
    match doc.get("timestamp_ms") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            Ok(Some(v.as_i64().ok_or_else(|| {
                bad_json("`timestamp_ms` must be an integer")
            })?))
        }
    }
}

/// Everything `SaveTree` borrows, owned, so the borrows can be taken afterwards.
struct Options {
    branch: BranchName,
    parent: Option<SnapshotId>,
    merge_parent: Option<SnapshotId>,
    message: String,
    meta: SnapshotMeta,
    author: Option<Author>,
    renames: Vec<(PathBuf, PathBuf)>,
    timestamp_ms: Option<i64>,
}

fn parse_options(repo: &velo_core::Repo, text: &str) -> Res<Options> {
    let value: Value = serde_json::from_str(text).map_err(bad_json)?;
    let doc = value
        .as_object()
        .ok_or_else(|| bad_json("expected a JSON object"))?;

    let spec = |key: &str| -> Res<Option<SnapshotId>> {
        match opt_str(doc, key)? {
            Some(s) => Ok(Some(resolve_snapshot_id(repo, s)?)),
            None => Ok(None),
        }
    };

    let meta = parse_meta(doc)?;
    let author = parse_author(doc)?;

    let mut renames = Vec::new();
    match doc.get("renames") {
        None | Some(Value::Null) => {}
        Some(Value::Array(pairs)) => {
            for pair in pairs {
                match pair.as_array().map(Vec::as_slice) {
                    Some([Value::String(from), Value::String(to)]) => {
                        renames.push((PathBuf::from(from), PathBuf::from(to)))
                    }
                    _ => return Err(bad_json("`renames` must be [[from, to], ...]")),
                }
            }
        }
        Some(_) => return Err(bad_json("`renames` must be an array")),
    }

    let timestamp_ms = parse_timestamp(doc)?;

    Ok(Options {
        branch: req_str(doc, "branch")?.parse()?,
        parent: spec("parent")?,
        merge_parent: spec("merge_parent")?,
        message: req_str(doc, "message")?.to_string(),
        meta,
        author,
        renames,
        timestamp_ms,
    })
}

/// Save `tree` as a new snapshot and write its id to `*out_id`.
///
/// `options_json` is an object with required `branch` and `message`, and
/// optional `parent` and `merge_parent` (any spec: id, prefix, tag or branch),
/// `author` (`{"name", "email"?}`), `meta` (`{namespace: {key: value}}`),
/// `renames` (`[[from, to], ...]`) and `timestamp_ms` (integer; omit for now).
///
/// The working tree and `.velo/PARENT` are never touched. The tree is not
/// consumed: the caller still frees it.
#[no_mangle]
pub unsafe extern "C" fn velo_save_tree(
    repo: *const VeloRepo,
    tree: *const VeloTree,
    options_json: *const c_char,
    out_id: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let tree = ref_arg(tree, "tree")?;
        let o = parse_options(repo, str_arg(options_json, "options_json")?)?;
        let out = mut_arg(out_id, "out_id")?;
        let id = repo.write()?.save_tree(SaveTree {
            branch: &o.branch,
            parent: o.parent.as_ref(),
            merge_parent: o.merge_parent.as_ref(),
            message: &o.message,
            entries: tree.entries.clone(),
            meta: o.meta,
            author: o.author.as_ref(),
            renames: &o.renames,
            timestamp_ms: o.timestamp_ms,
        })?;
        *out = to_c_string(id.into_string())?;
        Ok(())
    })
}

/// The files of a snapshot, as a JSON array of `{"path", "object", "kind"}`
/// in path order, where `kind` is `regular`, `executable` or `symlink`.
#[no_mangle]
pub unsafe extern "C" fn velo_tree_at(
    repo: *const VeloRepo,
    spec: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let id = snapshot_arg(repo, spec)?;
        let files: Vec<Value> = repo
            .tree_at(&id)?
            .into_iter()
            .map(|f| {
                json!({
                    "path": f.path,
                    "object": f.object.as_str(),
                    "kind": kind_name(f.kind),
                })
            })
            .collect();
        emit(out_json, Value::Array(files))
    })
}

/// The bytes of `path` as of a snapshot, written to `*out_data` with their
/// length in `*out_len`. Free with `velo_bytes_free(*out_data, *out_len)`.
#[no_mangle]
pub unsafe extern "C" fn velo_read_file_at(
    repo: *const VeloRepo,
    spec: *const c_char,
    path: *const c_char,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let path = str_arg(path, "path")?;
        let data_slot = mut_arg(out_data, "out_data")?;
        let len_slot = mut_arg(out_len, "out_len")?;
        let id = snapshot_arg(repo, spec)?;
        let bytes = repo.read_file_at(&id, path)?.into_boxed_slice();
        *len_slot = bytes.len();
        *data_slot = Box::into_raw(bytes) as *mut u8;
        Ok(())
    })
}

/// One snapshot as JSON: `{"id", "message", "created_at" (RFC 3339),
/// "created_at_ms", "branch", "parent", "merge_parent", "tag"}`. Absent
/// parents and tag are `null`.
#[no_mangle]
pub unsafe extern "C" fn velo_snapshot(
    repo: *const VeloRepo,
    spec: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let id = snapshot_arg(repo, spec)?;
        let e = repo.snapshot(&id)?;
        emit(out_json, crate::history::entry_json(&e))
    })
}

/// A snapshot's metadata as JSON, `{namespace: {key: value}}`, including the
/// reserved `velo` namespace that records the author.
#[no_mangle]
pub unsafe extern "C" fn velo_snapshot_meta(
    repo: *const VeloRepo,
    spec: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    run(|| {
        let repo = &ref_arg(repo, "repo")?.repo;
        let id = snapshot_arg(repo, spec)?;
        let meta = repo.snapshot_meta(&id)?;
        let mut doc = Map::new();
        for (namespace, key, value) in meta.iter() {
            doc.entry(namespace.to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("just inserted as an object")
                .insert(key.to_string(), Value::String(value.to_string()));
        }
        emit(out_json, Value::Object(doc))
    })
}
