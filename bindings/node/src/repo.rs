//! The embedder API as a JavaScript class.
//!
//! Every method returns a `Promise`. The work runs in a napi `Task` on the
//! libuv thread pool, where it locks the repository and calls velo-core, which
//! stays synchronous. One `Repo` is one `Arc<Mutex<velo_core::Repo>>`, so
//! concurrent calls on it serialise rather than share a connection.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi::{Env, JsDate, ScopedTask, Task};
use napi_derive::napi;
use velo_core::tree::{Content, FileKind, SaveTree, TreeEntry as CoreEntry};
use velo_core::{
    Author as CoreAuthor, BranchName, Error as CoreError, ObjectHash, Repo as CoreRepo, SnapshotId,
    SnapshotMeta,
};

use crate::errors::to_js;

pub(crate) type CoreResult<T> = std::result::Result<T, CoreError>;
pub(crate) type Shared = Arc<Mutex<CoreRepo>>;
pub(crate) type Meta = BTreeMap<String, BTreeMap<String, String>>;

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
            "'{}' is not a file kind; expected regular, executable or symlink.",
            other
        ))),
    }
}

pub(crate) fn snapshot_id(text: &str) -> CoreResult<SnapshotId> {
    text.parse()
}

pub(crate) fn branch_name(text: &str) -> CoreResult<BranchName> {
    text.parse()
}

type Work<T> = Box<dyn FnOnce(&CoreRepo) -> CoreResult<T> + Send>;

/// A velo call that runs on a libuv worker and resolves to `T`.
pub struct Job<T> {
    repo: Shared,
    run: Option<Work<T>>,
}

impl<T: ToNapiValue + TypeName + Send + 'static> Task for Job<T> {
    type Output = CoreResult<T>;
    type JsValue = T;

    fn compute(&mut self) -> Result<Self::Output> {
        let run = self
            .run
            .take()
            .ok_or_else(|| napi::Error::from_reason("velo job already ran"))?;
        let repo = self
            .repo
            .lock()
            .map_err(|_| napi::Error::from_reason("velo Repo is poisoned by an earlier panic"))?;
        Ok(run(&repo))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        output.map_err(|e| to_js(&env, e))
    }
}

/// `Repo.init` / `Repo.open`: there is no repository to lock yet.
pub struct OpenJob {
    path: PathBuf,
    init: bool,
}

impl Task for OpenJob {
    type Output = CoreResult<CoreRepo>;
    type JsValue = Repo;

    fn compute(&mut self) -> Result<Self::Output> {
        Ok(if self.init {
            CoreRepo::init(&self.path)
        } else {
            CoreRepo::open_and_migrate(&self.path)
        })
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        output
            .map(|r| Repo {
                inner: Arc::new(Mutex::new(r)),
            })
            .map_err(|e| to_js(&env, e))
    }
}

/// A file as recorded in a saved tree.
#[napi(object)]
pub struct TreeFile {
    pub path: String,
    pub object: String,
    /// `'regular'`, `'executable'` or `'symlink'`.
    pub kind: String,
}

/// A string or an explicit `null`, so absent ids read as `null`, not `undefined`.
pub(crate) type Nullable = Either<String, Null>;

pub(crate) fn nullable(value: Option<String>) -> Nullable {
    match value {
        Some(s) => Either::A(s),
        None => Either::B(Null),
    }
}

/// A snapshot's header.
#[napi(object)]
pub struct SnapshotInfo<'env> {
    pub id: String,
    pub message: String,
    pub created_at_ms: i64,
    pub created_at: JsDate<'env>,
    pub branch: String,
    #[napi(ts_type = "string | null")]
    pub parent: Nullable,
    #[napi(ts_type = "string | null")]
    pub merge_parent: Nullable,
    #[napi(ts_type = "string | null")]
    pub tag: Nullable,
}

/// What a worker reads for `snapshot`; the `Date` is made on the JS thread.
pub struct SnapshotData {
    id: String,
    message: String,
    created_at_ms: i64,
    branch: String,
    parent: Option<String>,
    merge_parent: Option<String>,
    tag: Option<String>,
}

impl SnapshotData {
    pub(crate) fn from_entry(e: velo_core::commands::history::Entry) -> Self {
        SnapshotData {
            id: e.hash.as_str().to_string(),
            message: e.message,
            created_at_ms: e.created_at.timestamp_millis(),
            branch: e.branch.as_str().to_string(),
            parent: e.parent.map(|p| p.as_str().to_string()),
            merge_parent: e.merge_parent.map(|p| p.as_str().to_string()),
            tag: e.tag.map(|t| t.as_str().to_string()),
        }
    }

    /// Make the JS object; the `Date` has to be created on the JS thread.
    pub(crate) fn into_info(self, env: &Env) -> Result<SnapshotInfo<'_>> {
        Ok(SnapshotInfo {
            id: self.id,
            message: self.message,
            created_at_ms: self.created_at_ms,
            created_at: env.create_date(self.created_at_ms as f64)?,
            branch: self.branch,
            parent: nullable(self.parent),
            merge_parent: nullable(self.merge_parent),
            tag: nullable(self.tag),
        })
    }
}

/// Like `Job`, but resolving to an object that holds a `Date`.
pub struct SnapshotJob {
    repo: Shared,
    id: String,
}

impl<'task> ScopedTask<'task> for SnapshotJob {
    type Output = CoreResult<SnapshotData>;
    type JsValue = SnapshotInfo<'task>;

    fn compute(&mut self) -> Result<Self::Output> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| napi::Error::from_reason("velo Repo is poisoned by an earlier panic"))?;
        Ok(snapshot_data(&repo, &self.id))
    }

    fn resolve(&mut self, env: &'task Env, output: Self::Output) -> Result<Self::JsValue> {
        output.map_err(|e| to_js(env, e))?.into_info(env)
    }
}

fn snapshot_data(repo: &CoreRepo, id: &str) -> CoreResult<SnapshotData> {
    let e = repo.snapshot(&snapshot_id(id)?)?;
    Ok(SnapshotData {
        id: e.hash.as_str().to_string(),
        message: e.message,
        created_at_ms: e.created_at.timestamp_millis(),
        branch: e.branch.as_str().to_string(),
        parent: e.parent.map(|p| p.as_str().to_string()),
        merge_parent: e.merge_parent.map(|p| p.as_str().to_string()),
        tag: e.tag.map(|t| t.as_str().to_string()),
    })
}

/// Who made a snapshot.
#[napi(object)]
pub struct AuthorInput {
    pub name: String,
    pub email: Option<String>,
}

/// One file of a tree to save. Exactly one of `data` and `object` is given.
#[napi(object)]
pub struct EntryInput {
    pub path: String,
    /// Bytes to store.
    pub data: Option<Either<Buffer, String>>,
    /// An object already in the store, to carry forward without its bytes.
    pub object: Option<String>,
    /// `'regular'` (default), `'executable'` or `'symlink'`.
    pub kind: Option<String>,
}

#[napi(object)]
pub struct SaveTreeInput {
    pub branch: String,
    pub message: String,
    pub entries: Vec<EntryInput>,
    pub parent: Option<String>,
    pub merge_parent: Option<String>,
    pub meta: Option<BTreeMap<String, BTreeMap<String, String>>>,
    pub author: Option<AuthorInput>,
    pub timestamp_ms: Option<i64>,
    /// `[from, to]` pairs of paths this snapshot moved.
    pub renames: Option<Vec<Vec<String>>>,
}

/// An `EntryInput` with the JS-thread-only `Buffer` copied out, so it can
/// cross to the worker. Validation waits for the worker too, so a bad entry
/// rejects like any other velo error.
struct RawEntry {
    path: String,
    data: Option<Vec<u8>>,
    object: Option<String>,
    kind: Option<String>,
}

fn build_entry(raw: RawEntry) -> CoreResult<CoreEntry> {
    let kind = parse_kind(raw.kind.as_deref())?;
    let content = match (raw.data, raw.object) {
        (Some(data), None) => Content::Bytes(data),
        (None, Some(object)) => Content::Stored(object.parse::<ObjectHash>()?),
        _ => {
            return Err(CoreError::invalid(format!(
                "entry '{}' needs exactly one of `data` and `object`.",
                raw.path
            )))
        }
    };
    Ok(CoreEntry {
        path: raw.path,
        content,
        kind,
    })
}

pub(crate) fn build_meta(meta: Option<Meta>) -> CoreResult<SnapshotMeta> {
    let mut snapshot_meta = SnapshotMeta::new();
    for (namespace, keys) in meta.unwrap_or_default() {
        for (key, value) in keys {
            snapshot_meta.set(&namespace, key, value)?;
        }
    }
    Ok(snapshot_meta)
}

pub(crate) fn build_author(author: Option<AuthorInput>) -> CoreResult<Option<CoreAuthor>> {
    author
        .map(|a| match a.email {
            Some(email) => CoreAuthor::with_email(a.name, email),
            None => CoreAuthor::new(a.name),
        })
        .transpose()
}

/// An open velo repository.
#[napi]
pub struct Repo {
    pub(crate) inner: Shared,
}

impl Repo {
    pub(crate) fn job<T: ToNapiValue + TypeName + Send + 'static>(
        &self,
        run: impl FnOnce(&CoreRepo) -> CoreResult<T> + Send + 'static,
    ) -> AsyncTask<Job<T>> {
        AsyncTask::new(Job {
            repo: self.inner.clone(),
            run: Some(Box::new(run)),
        })
    }
}

#[napi]
impl Repo {
    #[napi(ts_return_type = "Promise<Repo>")]
    pub fn init(path: String) -> AsyncTask<OpenJob> {
        AsyncTask::new(OpenJob {
            path: path.into(),
            init: true,
        })
    }

    #[napi(ts_return_type = "Promise<Repo>")]
    pub fn open(path: String) -> AsyncTask<OpenJob> {
        AsyncTask::new(OpenJob {
            path: path.into(),
            init: false,
        })
    }

    /// Save a whole tree as a snapshot and resolve to its id.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn save_tree(&self, input: SaveTreeInput) -> AsyncTask<Job<String>> {
        let SaveTreeInput {
            branch,
            message,
            entries,
            parent,
            merge_parent,
            meta,
            author,
            timestamp_ms,
            renames,
        } = input;
        let entries: Vec<RawEntry> = entries
            .into_iter()
            .map(|e| RawEntry {
                path: e.path,
                data: e.data.map(|d| match d {
                    Either::A(buf) => buf.to_vec(),
                    Either::B(text) => text.into_bytes(),
                }),
                object: e.object,
                kind: e.kind,
            })
            .collect();
        self.job(move |repo| {
            let branch = branch_name(&branch)?;
            let parent = parent.as_deref().map(snapshot_id).transpose()?;
            let merge_parent = merge_parent.as_deref().map(snapshot_id).transpose()?;
            let entries = entries
                .into_iter()
                .map(build_entry)
                .collect::<CoreResult<Vec<_>>>()?;
            let snapshot_meta = build_meta(meta)?;
            let author = build_author(author)?;
            let renames = renames
                .unwrap_or_default()
                .into_iter()
                .map(|pair| match <[String; 2]>::try_from(pair) {
                    Ok([from, to]) => Ok((PathBuf::from(from), PathBuf::from(to))),
                    Err(_) => Err(CoreError::invalid(
                        "each rename must be a [from, to] pair of paths.",
                    )),
                })
                .collect::<CoreResult<Vec<(PathBuf, PathBuf)>>>()?;

            let guard = repo.write()?;
            let id = guard.save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: merge_parent.as_ref(),
                message: &message,
                entries,
                meta: snapshot_meta,
                author: author.as_ref(),
                renames: &renames,
                timestamp_ms,
            })?;
            drop(guard);
            Ok(id.as_str().to_string())
        })
    }

    #[napi(ts_return_type = "Promise<Array<TreeFile>>")]
    pub fn tree_at(&self, id: String) -> AsyncTask<Job<Vec<TreeFile>>> {
        self.job(move |repo| {
            let files = repo.tree_at(&snapshot_id(&id)?)?;
            Ok(files
                .into_iter()
                .map(|f| TreeFile {
                    path: f.path,
                    object: f.object.as_str().to_string(),
                    kind: kind_name(f.kind).to_string(),
                })
                .collect())
        })
    }

    #[napi(ts_return_type = "Promise<Buffer>")]
    pub fn read_file_at(&self, id: String, path: String) -> AsyncTask<Job<Buffer>> {
        self.job(move |repo| {
            Ok(Buffer::from(
                repo.read_file_at(&snapshot_id(&id)?, &path)?.to_vec(),
            ))
        })
    }

    #[napi(ts_return_type = "Promise<Buffer>")]
    pub fn read_object(&self, object: String) -> AsyncTask<Job<Buffer>> {
        self.job(move |repo| {
            let object: ObjectHash = object.parse()?;
            Ok(Buffer::from(repo.read_object(&object)?.to_vec()))
        })
    }

    #[napi(ts_return_type = "Promise<SnapshotInfo>")]
    pub fn snapshot(&self, id: String) -> AsyncTask<SnapshotJob> {
        AsyncTask::new(SnapshotJob {
            repo: self.inner.clone(),
            id,
        })
    }

    #[napi(ts_return_type = "Promise<Record<string, Record<string, string>>>")]
    pub fn snapshot_meta(&self, id: String) -> AsyncTask<Job<Meta>> {
        self.job(move |repo| {
            let meta = repo.snapshot_meta(&snapshot_id(&id)?)?;
            let mut out: Meta = BTreeMap::new();
            for (namespace, key, value) in meta.iter() {
                out.entry(namespace.to_string())
                    .or_default()
                    .insert(key.to_string(), value.to_string());
            }
            Ok(out)
        })
    }

    /// Resolve a snapshot name (id, prefix, branch, tag, ...) to a full id.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn resolve(&self, spec: String) -> AsyncTask<Job<String>> {
        self.job(move |repo| {
            let id = velo_core::commands::resolve_snapshot_id(repo, &spec)?;
            Ok(id.as_str().to_string())
        })
    }

    #[napi(ts_return_type = "Promise<string | null>")]
    pub fn branch_tip(&self, branch: String) -> AsyncTask<Job<Option<String>>> {
        self.job(move |repo| {
            let tip = repo.branch_tip(&branch_name(&branch)?)?;
            Ok(tip.map(|t| t.as_str().to_string()))
        })
    }

    #[napi(ts_return_type = "Promise<bigint>")]
    pub fn head_token(&self) -> AsyncTask<Job<BigInt>> {
        self.job(|repo| Ok(BigInt::from(repo.head_token()?)))
    }
}
