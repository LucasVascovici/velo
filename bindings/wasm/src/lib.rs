//! WebAssembly binding for velo-core, over single-file repositories.
//!
//! Everything is synchronous after the repository is open: the core is
//! synchronous, and one tab (or worker) owns the database. Results are plain
//! JavaScript objects whose field names mirror the Node binding. Errors are
//! thrown as `Error` objects whose `code` is the velo `Error` variant name.
//!
//! There is no working tree and no remotes here. Only the store-only API of
//! velo-core works, which is what `Repo::create_file` provides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use js_sys::{Array, BigInt, Date, Object, Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use velo_core::commands::merge::{self, FileAction, MergeCommit, PlannedChange, Resolution};
use velo_core::commands::{blame, history};
use velo_core::error::{InProgress, RefKind};
use velo_core::tree::{Content, FileKind, SaveTree, TreeEntry as CoreEntry};
use velo_core::{
    Author as CoreAuthor, BranchName, Error as CoreError, ObjectHash, Repo as CoreRepo, SnapshotId,
    SnapshotMeta,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

type CoreResult<T> = std::result::Result<T, CoreError>;
type Meta = BTreeMap<String, BTreeMap<String, String>>;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export type FileKind = 'regular' | 'executable' | 'symlink';
export type Meta = Record<string, Record<string, string>>;
export interface AuthorInput { name: string; email?: string }
export interface EntryInput {
  path: string;
  data?: Uint8Array | string;
  object?: string;
  kind?: FileKind;
}
export interface SaveTreeInput {
  branch: string;
  message: string;
  entries: EntryInput[];
  parent?: string;
  mergeParent?: string;
  meta?: Meta;
  author?: AuthorInput;
  timestampMs?: number;
  renames?: [string, string][];
}
export interface MetaFilterInput { namespace: string; key: string; value?: string }
export interface HistoryOptions {
  from?: string;
  branch?: string;
  all?: boolean;
  paths?: string[];
  limit?: number;
  meta?: MetaFilterInput[];
}
export interface BlameOptions { at?: string; startLine?: number; endLine?: number }
export interface MergeCommitInput {
  branch: string;
  ours: string;
  theirs: string;
  message: string;
  resolutions?: Record<string, 'ours' | 'theirs' | null | Uint8Array | string>;
  meta?: Meta;
  author?: AuthorInput;
  timestampMs?: number;
}
export interface TreeFile { path: string; object: string; kind: FileKind }
export interface SnapshotInfo {
  id: string;
  message: string;
  createdAtMs: number;
  createdAt: Date;
  branch: string;
  parent: string | null;
  mergeParent: string | null;
  tag: string | null;
}
export interface LineOriginInfo {
  id: string;
  createdAtMs: number;
  createdAt: Date;
  message: string;
  author: { name: string; email: string | null } | null;
  branch: string;
  path: string;
}
export interface BlameInfo {
  path: string;
  snapshot: string;
  lines: { lineNo: number; text: string; lineCount: number; origin: LineOriginInfo | null }[];
}
export interface PlannedFileInfo {
  path: string;
  action: 'deleted' | 'added' | 'updated' | 'autoMerged' | 'keptOurs' | 'conflicted';
  object: string | null;
  mode: number | null;
  content: Uint8Array | null;
  base: string | null;
  ours: string | null;
  theirs: string | null;
}
export interface MergePlanInfo { base: string | null; files: PlannedFileInfo[] }
export type VeloErrorCode = string;
"#;

// ---------------------------------------------------------------- errors

fn ref_kind(kind: &RefKind) -> &'static str {
    match kind {
        RefKind::Snapshot => "snapshot",
        RefKind::Branch => "branch",
        RefKind::Tag => "tag",
        RefKind::Remote => "remote",
        RefKind::RemoteBranch => "remote_branch",
        RefKind::Stash => "stash",
        RefKind::Path => "path",
        RefKind::Any => "any",
        _ => "unknown",
    }
}

fn in_progress(what: &InProgress) -> &'static str {
    match what {
        InProgress::Merge => "merge",
        InProgress::Rebase => "rebase",
        InProgress::CherryPick => "cherry_pick",
        _ => "unknown",
    }
}

/// The variant name, which becomes the thrown error's `code`.
fn code(err: &CoreError) -> &'static str {
    match err {
        CoreError::NotARepo { .. } => "NotARepo",
        CoreError::AlreadyInitialized { .. } => "AlreadyInitialized",
        CoreError::NestedRepo { .. } => "NestedRepo",
        CoreError::SchemaTooNew { .. } => "SchemaTooNew",
        CoreError::MigrationRequired { .. } => "MigrationRequired",
        CoreError::FormatTooOld { .. } => "FormatTooOld",
        CoreError::Cancelled => "Cancelled",
        CoreError::Locked { .. } => "Locked",
        CoreError::DirtyWorkingTree { .. } => "DirtyWorkingTree",
        CoreError::OperationInProgress { .. } => "OperationInProgress",
        CoreError::NoOperationInProgress { .. } => "NoOperationInProgress",
        CoreError::Conflicts { .. } => "Conflicts",
        CoreError::Diverged { .. } => "Diverged",
        CoreError::NotFastForward { .. } => "NotFastForward",
        CoreError::UnbornBranch { .. } => "UnbornBranch",
        CoreError::NotFound { .. } => "NotFound",
        CoreError::AmbiguousPrefix { .. } => "AmbiguousPrefix",
        CoreError::Compacted { .. } => "Compacted",
        CoreError::Corrupt { .. } => "Corrupt",
        CoreError::MissingObject { .. } => "MissingObject",
        CoreError::UntrustedData { .. } => "UntrustedData",
        CoreError::InvalidInput { .. } => "InvalidInput",
        CoreError::Unsupported { .. } => "Unsupported",
        CoreError::Io(_) => "Io",
        CoreError::Db(_) => "Db",
        // `Error` is non_exhaustive: a variant added later still throws with
        // a recognisable code rather than failing to compile or panicking.
        _ => "Unknown",
    }
}

fn set(obj: &Object, key: &str, value: impl Into<JsValue>) {
    // Setting a property on a fresh plain object cannot fail.
    let _ = Reflect::set(obj, &JsValue::from_str(key), &value.into());
}

fn path_array(paths: &[PathBuf]) -> Array {
    paths
        .iter()
        .map(|p| JsValue::from_str(&p.to_string_lossy()))
        .collect()
}

/// The JS `Error` for a velo error: `code` is the variant name and the
/// variant's fields are attached as properties, so typed errors are not
/// flattened into strings at the boundary.
fn js_err(err: CoreError) -> JsValue {
    let e = js_sys::Error::new(&err.to_string());
    let obj: &Object = e.as_ref();
    set(obj, "code", code(&err));
    match &err {
        CoreError::NotARepo { searched_from } => set(
            obj,
            "searchedFrom",
            searched_from.to_string_lossy().as_ref(),
        ),
        CoreError::AlreadyInitialized { at } => set(obj, "at", at.to_string_lossy().as_ref()),
        CoreError::SchemaTooNew { found, supported }
        | CoreError::MigrationRequired { found, supported }
        | CoreError::FormatTooOld { found, supported } => {
            set(obj, "found", *found as f64);
            set(obj, "supported", *supported as f64);
        }
        CoreError::DirtyWorkingTree { paths } | CoreError::Conflicts { paths } => {
            set(obj, "paths", path_array(paths))
        }
        CoreError::OperationInProgress { what } | CoreError::NoOperationInProgress { what } => {
            set(obj, "what", in_progress(what))
        }
        CoreError::UnbornBranch { branch } => set(obj, "branch", branch.as_str()),
        CoreError::NotFound { kind, name } => {
            set(obj, "kind", ref_kind(kind));
            set(obj, "name", name.as_str());
        }
        CoreError::AmbiguousPrefix { prefix, matches } => {
            set(obj, "prefix", prefix.as_str());
            set(obj, "matches", *matches as f64);
        }
        CoreError::Compacted { id, into } => {
            set(obj, "id", id.as_str());
            set(obj, "into", into.as_str());
        }
        CoreError::MissingObject { hash } => set(obj, "hash", hash.as_str()),
        CoreError::Corrupt { detail }
        | CoreError::UntrustedData { detail }
        | CoreError::InvalidInput { detail }
        | CoreError::Unsupported { detail } => set(obj, "detail", detail.as_str()),
        _ => {}
    }
    e.into()
}

fn lift<T>(r: CoreResult<T>) -> Result<T, JsValue> {
    r.map_err(js_err)
}

// ----------------------------------------------------------- conversions

/// Serialize into plain JS objects, with absent values as `null` (not
/// `undefined`) as in the Node binding.
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    let ser = serde_wasm_bindgen::Serializer::new()
        .serialize_missing_as_null(true)
        .serialize_maps_as_objects(true);
    value
        .serialize(&ser)
        .map_err(|e| js_err(CoreError::invalid(e.to_string())))
}

fn from_js<T: serde::de::DeserializeOwned>(value: JsValue, what: &str) -> CoreResult<T> {
    serde_wasm_bindgen::from_value(value)
        .map_err(|e| CoreError::invalid(format!("bad {what}: {e}")))
}

fn ser_bytes<S: serde::Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(b) => s.serialize_bytes(b),
        None => s.serialize_none(),
    }
}

fn ser_date<S: serde::Serializer>(ms: &i64, s: S) -> Result<S::Ok, S::Error> {
    let date = Date::new(&JsValue::from_f64(*ms as f64));
    serde_wasm_bindgen::preserve::serialize(&date, s)
}

fn snapshot_id(text: &str) -> CoreResult<SnapshotId> {
    text.parse()
}

fn branch_name(text: &str) -> CoreResult<BranchName> {
    text.parse()
}

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Regular => "regular",
        FileKind::Executable => "executable",
        FileKind::Symlink => "symlink",
    }
}

fn parse_kind(kind: Option<&str>) -> CoreResult<FileKind> {
    match kind {
        None | Some("regular") => Ok(FileKind::Regular),
        Some("executable") => Ok(FileKind::Executable),
        Some("symlink") => Ok(FileKind::Symlink),
        Some(other) => Err(CoreError::invalid(format!(
            "'{other}' is not a file kind; expected regular, executable or symlink."
        ))),
    }
}

#[derive(Deserialize)]
struct AuthorInput {
    name: String,
    email: Option<String>,
}

fn build_author(author: Option<AuthorInput>) -> CoreResult<Option<CoreAuthor>> {
    author
        .map(|a| match a.email {
            Some(email) => CoreAuthor::with_email(a.name, email),
            None => CoreAuthor::new(a.name),
        })
        .transpose()
}

fn build_meta(meta: Option<Meta>) -> CoreResult<SnapshotMeta> {
    let mut snapshot_meta = SnapshotMeta::new();
    for (namespace, keys) in meta.unwrap_or_default() {
        for (key, value) in keys {
            snapshot_meta.set(&namespace, key, value)?;
        }
    }
    Ok(snapshot_meta)
}

fn prop(obj: &JsValue, key: &str) -> JsValue {
    Reflect::get(obj, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn opt_string(obj: &JsValue, key: &str) -> CoreResult<Option<String>> {
    let v = prop(obj, key);
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    v.as_string()
        .map(Some)
        .ok_or_else(|| CoreError::invalid(format!("`{key}` must be a string.")))
}

/// Parse `entries` by hand: a `Uint8Array` must stay bytes, which serde's
/// self-describing model would not guarantee next to a string alternative.
fn parse_entries(input: &JsValue) -> CoreResult<Vec<CoreEntry>> {
    let entries = prop(input, "entries");
    if !Array::is_array(&entries) {
        return Err(CoreError::invalid("`entries` must be an array."));
    }
    Array::from(&entries)
        .iter()
        .map(|e| {
            let path = opt_string(&e, "path")?
                .ok_or_else(|| CoreError::invalid("every entry needs a `path`."))?;
            let data = prop(&e, "data");
            let data: Option<Vec<u8>> = if data.is_undefined() || data.is_null() {
                None
            } else if let Some(text) = data.as_string() {
                Some(text.into_bytes())
            } else if data.is_instance_of::<Uint8Array>() {
                Some(Uint8Array::new(&data).to_vec())
            } else {
                return Err(CoreError::invalid(format!(
                    "entry '{path}': `data` must be a Uint8Array or a string."
                )));
            };
            let object = opt_string(&e, "object")?;
            let kind = parse_kind(opt_string(&e, "kind")?.as_deref())?;
            let content = match (data, object) {
                (Some(data), None) => Content::Bytes(data),
                (None, Some(object)) => Content::Stored(object.parse::<ObjectHash>()?),
                _ => {
                    return Err(CoreError::invalid(format!(
                        "entry '{path}' needs exactly one of `data` and `object`."
                    )))
                }
            };
            Ok(CoreEntry {
                path,
                content,
                kind,
            })
        })
        .collect()
}

// --------------------------------------------------------------- results

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeFile {
    path: String,
    object: String,
    kind: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotInfo {
    id: String,
    message: String,
    created_at_ms: i64,
    #[serde(serialize_with = "ser_date")]
    created_at: i64,
    branch: String,
    parent: Option<String>,
    merge_parent: Option<String>,
    tag: Option<String>,
}

fn snapshot_info(e: history::Entry) -> SnapshotInfo {
    let ms = e.created_at.timestamp_millis();
    SnapshotInfo {
        id: e.hash.as_str().to_string(),
        message: e.message,
        created_at_ms: ms,
        created_at: ms,
        branch: e.branch.as_str().to_string(),
        parent: e.parent.map(|p| p.as_str().to_string()),
        merge_parent: e.merge_parent.map(|p| p.as_str().to_string()),
        tag: e.tag.map(|t| t.as_str().to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveInput {
    branch: String,
    message: String,
    parent: Option<String>,
    merge_parent: Option<String>,
    meta: Option<Meta>,
    author: Option<AuthorInput>,
    timestamp_ms: Option<i64>,
    renames: Option<Vec<Vec<String>>>,
}

#[derive(Deserialize)]
struct MetaFilterInput {
    namespace: String,
    key: String,
    value: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct HistoryOptions {
    from: Option<String>,
    branch: Option<String>,
    all: Option<bool>,
    paths: Option<Vec<String>>,
    limit: Option<u32>,
    meta: Option<Vec<MetaFilterInput>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct BlameOptions {
    at: Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
}

#[derive(Serialize)]
struct AuthorInfo {
    name: String,
    email: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LineOriginInfo {
    id: String,
    created_at_ms: i64,
    #[serde(serialize_with = "ser_date")]
    created_at: i64,
    message: String,
    author: Option<AuthorInfo>,
    branch: String,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BlameLineInfo {
    line_no: u32,
    text: String,
    line_count: u32,
    origin: Option<LineOriginInfo>,
}

#[derive(Serialize)]
struct BlameInfo {
    path: String,
    snapshot: String,
    lines: Vec<BlameLineInfo>,
}

#[derive(Serialize)]
struct PlannedFileInfo {
    path: String,
    action: &'static str,
    object: Option<String>,
    mode: Option<i64>,
    #[serde(serialize_with = "ser_bytes")]
    content: Option<Vec<u8>>,
    base: Option<String>,
    ours: Option<String>,
    theirs: Option<String>,
}

#[derive(Serialize)]
struct MergePlanInfo {
    base: Option<String>,
    files: Vec<PlannedFileInfo>,
}

fn action_name(action: FileAction) -> &'static str {
    match action {
        FileAction::Deleted => "deleted",
        FileAction::Added => "added",
        FileAction::Updated => "updated",
        FileAction::AutoMerged => "autoMerged",
        FileAction::KeptOurs => "keptOurs",
        FileAction::Conflicted => "conflicted",
    }
}

fn plan_info(plan: merge::MergePlan) -> MergePlanInfo {
    let id = |h: &ObjectHash| h.as_str().to_string();
    let files = plan
        .files
        .into_iter()
        .map(|f| {
            let mut info = PlannedFileInfo {
                path: f.path,
                action: action_name(f.change.action()),
                object: None,
                mode: None,
                content: None,
                base: None,
                ours: None,
                theirs: None,
            };
            match f.change {
                PlannedChange::Take { object, mode, .. } => {
                    info.object = Some(id(&object));
                    info.mode = Some(mode);
                }
                PlannedChange::AutoMerge { content, mode } => {
                    info.content = Some(content);
                    info.mode = Some(mode);
                }
                PlannedChange::Conflict { base, ours, theirs } => {
                    info.base = base.as_ref().map(id);
                    info.ours = ours.as_ref().map(id);
                    info.theirs = theirs.as_ref().map(id);
                }
                _ => {}
            }
            info
        })
        .collect();
    MergePlanInfo {
        base: plan.base.map(|b| b.as_str().to_string()),
        files,
    }
}

fn parse_resolutions(input: &JsValue) -> CoreResult<Vec<(String, Resolution)>> {
    let resolutions = prop(input, "resolutions");
    if resolutions.is_undefined() || resolutions.is_null() {
        return Ok(Vec::new());
    }
    if !resolutions.is_object() {
        return Err(CoreError::invalid("`resolutions` must be an object."));
    }
    Object::entries(resolutions.unchecked_ref::<Object>())
        .iter()
        .map(|pair| {
            let pair = Array::from(&pair);
            let path = pair.get(0).as_string().unwrap_or_default();
            let value = pair.get(1);
            let resolution = if value.is_null() {
                Resolution::Delete
            } else if let Some(text) = value.as_string() {
                match text.as_str() {
                    "ours" => Resolution::Ours,
                    "theirs" => Resolution::Theirs,
                    _ => Resolution::Content(text.into_bytes()),
                }
            } else if value.is_instance_of::<Uint8Array>() {
                Resolution::Content(Uint8Array::new(&value).to_vec())
            } else {
                return Err(CoreError::invalid(format!(
                    "resolution for '{path}' must be 'ours', 'theirs', null, a string or a Uint8Array."
                )));
            };
            Ok((path, resolution))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeInput {
    branch: String,
    ours: String,
    theirs: String,
    message: String,
    meta: Option<Meta>,
    author: Option<AuthorInput>,
    timestamp_ms: Option<i64>,
}

// ------------------------------------------------------------------ Repo

/// An open single-file velo repository.
#[wasm_bindgen]
pub struct Repo {
    inner: CoreRepo,
}

/// Create the repository at `name`, or open it when it already exists.
fn create_or_open(name: &str, exists: bool) -> CoreResult<CoreRepo> {
    let path = Path::new(name);
    if exists {
        CoreRepo::open_file_and_migrate(path)
    } else {
        CoreRepo::create_file(path)
    }
}

#[wasm_bindgen]
impl Repo {
    /// Create a repository in the default in-memory VFS. It lives as long as
    /// the module instance; use a distinct `name` per repository.
    #[wasm_bindgen(js_name = createInMemory)]
    pub fn create_in_memory(name: &str) -> Result<Repo, JsValue> {
        let inner = lift(CoreRepo::create_file(Path::new(name)))?;
        Ok(Repo { inner })
    }

    /// Reopen a repository created earlier by `createInMemory` under `name`.
    #[wasm_bindgen(js_name = openInMemory)]
    pub fn open_in_memory(name: &str) -> Result<Repo, JsValue> {
        let inner = lift(CoreRepo::open_file_and_migrate(Path::new(name)))?;
        Ok(Repo { inner })
    }

    /// Create or open a repository stored in the browser's origin-private
    /// file system. Installs the OPFS sahpool VFS on first use, which is only
    /// possible inside a dedicated Web Worker; elsewhere this rejects with
    /// code `'Unsupported'`.
    #[wasm_bindgen(js_name = openPersistent)]
    pub async fn open_persistent(name: String) -> Result<Repo, JsValue> {
        let exists = persistent::install_and_probe(&name).await?;
        let inner = lift(create_or_open(&name, exists))?;
        Ok(Repo { inner })
    }

    /// Save a whole tree as a snapshot and return its id.
    #[wasm_bindgen(js_name = saveTree)]
    pub fn save_tree(&self, input: JsValue) -> Result<String, JsValue> {
        lift(self.save_tree_inner(input))
    }

    #[wasm_bindgen(js_name = treeAt)]
    pub fn tree_at(&self, id: &str) -> Result<JsValue, JsValue> {
        let files = lift(snapshot_id(id).and_then(|id| self.inner.tree_at(&id)))?;
        let files: Vec<TreeFile> = files
            .into_iter()
            .map(|f| TreeFile {
                path: f.path,
                object: f.object.as_str().to_string(),
                kind: kind_name(f.kind),
            })
            .collect();
        to_js(&files)
    }

    #[wasm_bindgen(js_name = readFileAt)]
    pub fn read_file_at(&self, id: &str, path: &str) -> Result<Uint8Array, JsValue> {
        let bytes = lift(snapshot_id(id).and_then(|id| self.inner.read_file_at(&id, path)))?;
        Ok(Uint8Array::from(bytes.as_slice()))
    }

    pub fn snapshot(&self, id: &str) -> Result<JsValue, JsValue> {
        let entry = lift(snapshot_id(id).and_then(|id| self.inner.snapshot(&id)))?;
        to_js(&snapshot_info(entry))
    }

    #[wasm_bindgen(js_name = snapshotMeta)]
    pub fn snapshot_meta(&self, id: &str) -> Result<JsValue, JsValue> {
        let meta = lift(snapshot_id(id).and_then(|id| self.inner.snapshot_meta(&id)))?;
        let mut out: Meta = BTreeMap::new();
        for (namespace, key, value) in meta.iter() {
            out.entry(namespace.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
        to_js(&out)
    }

    /// Snapshots newest first, filtered by `options`.
    pub fn history(&self, options: JsValue) -> Result<JsValue, JsValue> {
        let entries = lift(self.history_inner(options))?;
        to_js(&entries)
    }

    /// Attribute each line of `path` to the snapshot that last changed it.
    pub fn blame(&self, path: &str, options: JsValue) -> Result<JsValue, JsValue> {
        let info = lift(self.blame_inner(path, options))?;
        to_js(&info)
    }

    /// The nearest snapshot that is an ancestor of both, or `null`.
    #[wasm_bindgen(js_name = mergeBase)]
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>, JsValue> {
        let base = lift(
            snapshot_id(a)
                .and_then(|a| Ok((a, snapshot_id(b)?)))
                .and_then(|(a, b)| merge::merge_base(&self.inner, &a, &b)),
        )?;
        Ok(base.map(|s| s.as_str().to_string()))
    }

    /// What merging `theirs` into `ours` would do, without doing it.
    #[wasm_bindgen(js_name = mergePlan)]
    pub fn merge_plan(&self, ours: &str, theirs: &str) -> Result<JsValue, JsValue> {
        let plan = lift(
            snapshot_id(ours)
                .and_then(|o| Ok((o, snapshot_id(theirs)?)))
                .and_then(|(o, t)| merge::plan(&self.inner, &o, &t)),
        )?;
        to_js(&plan_info(plan))
    }

    /// Record the merge as a snapshot and return its id. Throws with code
    /// `'Conflicts'` and `paths` when a conflict has no resolution.
    #[wasm_bindgen(js_name = mergeCommit)]
    pub fn merge_commit(&self, input: JsValue) -> Result<String, JsValue> {
        lift(self.merge_commit_inner(input))
    }

    #[wasm_bindgen(js_name = branchTip)]
    pub fn branch_tip(&self, name: &str) -> Result<Option<String>, JsValue> {
        let tip = lift(branch_name(name).and_then(|b| self.inner.branch_tip(&b)))?;
        Ok(tip.map(|t| t.as_str().to_string()))
    }

    /// A token that changes whenever the repository's branches or snapshots do.
    #[wasm_bindgen(js_name = headToken)]
    pub fn head_token(&self) -> Result<BigInt, JsValue> {
        Ok(BigInt::from(lift(self.inner.head_token())?))
    }
}

impl Repo {
    fn save_tree_inner(&self, input: JsValue) -> CoreResult<String> {
        let entries = parse_entries(&input)?;
        let i: SaveInput = from_js(input, "saveTree input")?;
        let branch = branch_name(&i.branch)?;
        let parent = i.parent.as_deref().map(snapshot_id).transpose()?;
        let merge_parent = i.merge_parent.as_deref().map(snapshot_id).transpose()?;
        let meta = build_meta(i.meta)?;
        let author = build_author(i.author)?;
        let renames = i
            .renames
            .unwrap_or_default()
            .into_iter()
            .map(|pair| match <[String; 2]>::try_from(pair) {
                Ok([from, to]) => Ok((PathBuf::from(from), PathBuf::from(to))),
                Err(_) => Err(CoreError::invalid(
                    "each rename must be a [from, to] pair of paths.",
                )),
            })
            .collect::<CoreResult<Vec<(PathBuf, PathBuf)>>>()?;
        let guard = self.inner.write()?;
        let id = guard.save_tree(SaveTree {
            branch: &branch,
            parent: parent.as_ref(),
            merge_parent: merge_parent.as_ref(),
            message: &i.message,
            entries,
            meta,
            author: author.as_ref(),
            renames: &renames,
            timestamp_ms: i.timestamp_ms,
        })?;
        Ok(id.as_str().to_string())
    }

    fn history_inner(&self, options: JsValue) -> CoreResult<Vec<SnapshotInfo>> {
        let o: HistoryOptions =
            from_js::<Option<HistoryOptions>>(options, "history options")?.unwrap_or_default();
        let from = o.from.as_deref().map(snapshot_id).transpose()?;
        let branch = o.branch.as_deref().map(branch_name).transpose()?;
        let paths: Vec<PathBuf> = o
            .paths
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let filters: Vec<history::MetaFilter<'_>> = o
            .meta
            .iter()
            .flatten()
            .map(|m| match &m.value {
                Some(v) => history::MetaFilter::equals(&m.namespace, &m.key, v),
                None => history::MetaFilter::has(&m.namespace, &m.key),
            })
            .collect();
        let h = history::run(
            &self.inner,
            history::Options {
                all: o.all.unwrap_or(false),
                branch: branch.as_ref(),
                from: from.as_ref(),
                paths: &path_refs,
                meta: &filters,
                limit: o.limit.map(|n| n as usize),
            },
        )?;
        Ok(h.entries.into_iter().map(snapshot_info).collect())
    }

    fn blame_inner(&self, file: &str, options: JsValue) -> CoreResult<BlameInfo> {
        let o: BlameOptions =
            from_js::<Option<BlameOptions>>(options, "blame options")?.unwrap_or_default();
        let at = o.at.as_deref().map(snapshot_id).transpose()?;
        // JS counts lines from 1 and includes the end; core's window is half-open.
        let lines = match (o.start_line, o.end_line) {
            (None, None) => None,
            (start, end) => {
                let start = start.unwrap_or(1) as usize;
                let end = end.map_or(usize::MAX, |e| (e as usize).saturating_add(1));
                Some(start..end)
            }
        };
        let b = blame::run(
            &self.inner,
            Path::new(file),
            blame::Options {
                at: at.as_ref(),
                lines,
                ..Default::default()
            },
        )?;
        Ok(BlameInfo {
            path: b.path.to_string_lossy().into_owned(),
            snapshot: b.snapshot.as_str().to_string(),
            lines: b
                .lines
                .into_iter()
                .map(|l| BlameLineInfo {
                    line_no: l.line_no as u32,
                    text: l.text,
                    line_count: l.line_count as u32,
                    origin: l.origin.map(|o| {
                        let ms = o.created_at.timestamp_millis();
                        LineOriginInfo {
                            id: o.hash.as_str().to_string(),
                            created_at_ms: ms,
                            created_at: ms,
                            message: o.message,
                            author: o.author.map(|a| AuthorInfo {
                                name: a.name().to_string(),
                                email: a.email().map(str::to_string),
                            }),
                            branch: o.branch.as_str().to_string(),
                            path: o.path.to_string_lossy().into_owned(),
                        }
                    }),
                })
                .collect(),
        })
    }

    fn merge_commit_inner(&self, input: JsValue) -> CoreResult<String> {
        let resolutions = parse_resolutions(&input)?;
        let i: MergeInput = from_js(input, "mergeCommit input")?;
        let branch = branch_name(&i.branch)?;
        let ours = snapshot_id(&i.ours)?;
        let theirs = snapshot_id(&i.theirs)?;
        let meta = build_meta(i.meta)?;
        let author = build_author(i.author)?;
        let guard = self.inner.write()?;
        let id = merge::commit(
            &guard,
            MergeCommit {
                branch: &branch,
                ours: &ours,
                theirs: &theirs,
                resolutions: &resolutions,
                message: &i.message,
                meta,
                author: author.as_ref(),
                timestamp_ms: i.timestamp_ms,
            },
        )?;
        Ok(id.as_str().to_string())
    }
}

// ------------------------------------------------------------ persistence

#[cfg(target_arch = "wasm32")]
mod persistent {
    use std::cell::RefCell;

    use super::*;
    use sqlite_wasm_vfs::sahpool::{install, OpfsSAHPoolCfg, OpfsSAHPoolUtil};

    thread_local! {
        /// Installed once per module instance; the util answers `exists`.
        static POOL: RefCell<Option<OpfsSAHPoolUtil>> = const { RefCell::new(None) };
    }

    /// Only a dedicated worker has the synchronous OPFS handles sahpool needs.
    fn in_dedicated_worker() -> bool {
        Reflect::get(
            &js_sys::global(),
            &JsValue::from_str("DedicatedWorkerGlobalScope"),
        )
        .map(|v| v.is_function())
        .unwrap_or(false)
    }

    /// Install the OPFS VFS as the default (once) and report whether `name`
    /// already exists in it. `std::fs` cannot answer that on wasm.
    pub async fn install_and_probe(name: &str) -> Result<bool, JsValue> {
        if !in_dedicated_worker() {
            return Err(js_err(CoreError::Unsupported {
                detail: "openPersistent needs a dedicated Web Worker with OPFS.".into(),
            }));
        }
        if POOL.with(|p| p.borrow().is_none()) {
            let util = install::<sqlite_wasm_rs::WasmOsCallback>(&OpfsSAHPoolCfg::default(), true)
                .await
                .map_err(|e| {
                    js_err(CoreError::Unsupported {
                        detail: format!("could not install the OPFS VFS: {e}"),
                    })
                })?;
            POOL.with(|p| *p.borrow_mut() = Some(util));
        }
        POOL.with(|p| {
            let pool = p.borrow();
            let util = pool.as_ref().expect("installed above");
            // SQLite may hand the VFS the name with or without a leading slash.
            let lookup = |n: &str| {
                util.exists(n)
                    .map_err(|e| js_err(CoreError::invalid(format!("OPFS lookup failed: {e}"))))
            };
            Ok(lookup(name)? || lookup(&format!("/{}", name.trim_start_matches('/')))?)
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod persistent {
    use super::*;

    /// The OPFS VFS exists only when built for wasm.
    pub async fn install_and_probe(_name: &str) -> Result<bool, JsValue> {
        Err(js_err(CoreError::Unsupported {
            detail: "openPersistent is only available in a WebAssembly build.".into(),
        }))
    }
}
