//! The embedder API as Python classes.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, TryLockError};

use pyo3::exceptions::{PyRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyString};
use velo_core::tree::{
    Change as CoreChange, Content, FileKind, SaveChanges, SaveTree, TreeEntry as CoreEntry,
};
use velo_core::{Author as CoreAuthor, BranchName, ObjectHash, SnapshotId, SnapshotMeta};

use crate::callbacks::{Hooks, Rule};
use crate::errors::{to_py, ReentrantCall, REENTRANT};
use crate::history::{meta_filters, Blame, Branch};
use crate::merge::{parse_resolutions, MergePlan};

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Regular => "regular",
        FileKind::Executable => "executable",
        FileKind::Symlink => "symlink",
    }
}

fn parse_kind(kind: &str) -> PyResult<FileKind> {
    match kind {
        "regular" => Ok(FileKind::Regular),
        "executable" => Ok(FileKind::Executable),
        "symlink" => Ok(FileKind::Symlink),
        other => Err(to_py(velo_core::Error::invalid(format!(
            "'{}' is not a file kind; expected regular, executable or symlink.",
            other
        )))),
    }
}

pub(crate) fn snapshot_id(text: &str) -> PyResult<SnapshotId> {
    text.parse().map_err(to_py)
}

pub(crate) fn branch_name(text: &str) -> PyResult<BranchName> {
    text.parse().map_err(to_py)
}

/// One file of a tree to save.
#[pyclass(frozen, skip_from_py_object, module = "velo")]
#[derive(Clone)]
pub struct TreeEntry {
    inner: CoreEntry,
}

#[pymethods]
impl TreeEntry {
    #[staticmethod]
    fn file(path: String, data: Vec<u8>) -> Self {
        TreeEntry {
            inner: CoreEntry::file(path, data),
        }
    }

    #[staticmethod]
    fn executable(path: String, data: Vec<u8>) -> Self {
        TreeEntry {
            inner: CoreEntry::executable(path, data),
        }
    }

    #[staticmethod]
    fn symlink(path: String, target: String) -> Self {
        TreeEntry {
            inner: CoreEntry::symlink(path, target),
        }
    }

    /// Carry forward an object already in the store, without its bytes.
    #[staticmethod]
    #[pyo3(signature = (path, object, kind = "regular"))]
    fn stored(path: String, object: &str, kind: &str) -> PyResult<Self> {
        let object: ObjectHash = object.parse().map_err(to_py)?;
        Ok(TreeEntry {
            inner: CoreEntry::stored(path, object, parse_kind(kind)?),
        })
    }

    #[getter]
    fn path(&self) -> &str {
        &self.inner.path
    }

    fn __repr__(&self) -> String {
        format!("TreeEntry({:?})", self.inner.path)
    }
}

/// A file as recorded in a saved tree.
#[pyclass(frozen, get_all, module = "velo")]
pub struct TreeFile {
    path: String,
    object: String,
    kind: String,
}

#[pymethods]
impl TreeFile {
    fn __repr__(&self) -> String {
        format!("TreeFile({:?}, {}, {})", self.path, self.object, self.kind)
    }
}

/// Who made a snapshot.
#[pyclass(frozen, module = "velo")]
pub struct Author {
    pub(crate) inner: CoreAuthor,
}

#[pymethods]
impl Author {
    #[new]
    #[pyo3(signature = (name, email = None))]
    pub(crate) fn new(name: String, email: Option<String>) -> PyResult<Self> {
        let inner = match email {
            Some(email) => CoreAuthor::with_email(name, email),
            None => CoreAuthor::new(name),
        }
        .map_err(to_py)?;
        Ok(Author { inner })
    }

    #[getter]
    fn name(&self) -> &str {
        self.inner.name()
    }

    #[getter]
    fn email(&self) -> Option<&str> {
        self.inner.email()
    }
}

/// A timezone-aware `datetime` in UTC for a millisecond timestamp.
pub(crate) fn utc_datetime(py: Python<'_>, ms: i64) -> PyResult<Bound<'_, PyAny>> {
    let datetime = py.import("datetime")?;
    let utc = datetime.getattr("timezone")?.getattr("utc")?;
    datetime
        .getattr("datetime")?
        .call_method1("fromtimestamp", (ms as f64 / 1000.0, utc))
}

/// Build snapshot metadata from the `{namespace: {key: value}}` shape.
pub(crate) fn build_meta(
    meta: Option<BTreeMap<String, BTreeMap<String, String>>>,
) -> PyResult<SnapshotMeta> {
    let mut snapshot_meta = SnapshotMeta::new();
    for (namespace, keys) in meta.unwrap_or_default() {
        for (key, value) in keys {
            snapshot_meta.set(&namespace, key, value).map_err(to_py)?;
        }
    }
    Ok(snapshot_meta)
}

/// A snapshot's header.
#[pyclass(frozen, get_all, module = "velo")]
pub struct Entry {
    id: String,
    message: String,
    created_at_ms: i64,
    branch: String,
    parent: Option<String>,
    merge_parent: Option<String>,
    tag: Option<String>,
}

#[pymethods]
impl Entry {
    /// A timezone-aware `datetime` in UTC.
    #[getter]
    fn created_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        utc_datetime(py, self.created_at_ms)
    }

    #[getter]
    fn is_merge(&self) -> bool {
        self.merge_parent.is_some()
    }

    fn __repr__(&self) -> String {
        format!("Entry({}, {:?})", self.id, self.message)
    }
}

impl From<velo_core::commands::history::Entry> for Entry {
    fn from(e: velo_core::commands::history::Entry) -> Self {
        Entry {
            id: e.hash.as_str().to_string(),
            message: e.message,
            created_at_ms: e.created_at.timestamp_millis(),
            branch: e.branch.as_str().to_string(),
            parent: e.parent.map(|p| p.as_str().to_string()),
            merge_parent: e.merge_parent.map(|p| p.as_str().to_string()),
            tag: e.tag.map(|t| t.as_str().to_string()),
        }
    }
}

fn entry_from_value(path: String, value: &Bound<'_, PyAny>) -> PyResult<CoreEntry> {
    if let Ok(entry) = value.extract::<PyRef<'_, TreeEntry>>() {
        let mut inner = entry.inner.clone();
        inner.path = path;
        Ok(inner)
    } else if let Ok(bytes) = value.cast::<PyBytes>() {
        Ok(CoreEntry::file(path, bytes.as_bytes().to_vec()))
    } else if let Ok(text) = value.cast::<PyString>() {
        Ok(CoreEntry::file(
            path,
            text.extract::<String>()?.into_bytes(),
        ))
    } else {
        Err(PyTypeError::new_err(
            "entry values must be bytes, str or TreeEntry",
        ))
    }
}

fn collect_entries(entries: &Bound<'_, PyAny>) -> PyResult<Vec<CoreEntry>> {
    if let Ok(dict) = entries.cast::<PyDict>() {
        dict.iter()
            .map(|(k, v)| entry_from_value(k.extract::<String>()?, &v))
            .collect()
    } else if let Ok(list) = entries.cast::<PyList>() {
        list.iter()
            .map(|item| {
                let entry: PyRef<'_, TreeEntry> = item.extract().map_err(|_| {
                    PyTypeError::new_err("a list of entries must contain TreeEntry objects")
                })?;
                Ok(entry.inner.clone())
            })
            .collect()
    } else {
        Err(PyTypeError::new_err(
            "entries must be a dict or a list of TreeEntry",
        ))
    }
}

/// One change to apply to a parent's tree with `Repo.save_changes`.
#[pyclass(frozen, skip_from_py_object, module = "velo")]
#[derive(Clone)]
pub struct Change {
    inner: CoreChange,
}

#[pymethods]
impl Change {
    /// Add or replace a file with `data` (bytes or str).
    #[staticmethod]
    #[pyo3(signature = (path, data, kind = "regular"))]
    fn put(path: String, data: &Bound<'_, PyAny>, kind: &str) -> PyResult<Self> {
        let bytes = if let Ok(b) = data.cast::<PyBytes>() {
            b.as_bytes().to_vec()
        } else if let Ok(t) = data.cast::<PyString>() {
            t.extract::<String>()?.into_bytes()
        } else {
            return Err(PyTypeError::new_err("data must be bytes or str"));
        };
        Ok(Change {
            inner: CoreChange::Put(CoreEntry {
                path,
                content: Content::Bytes(bytes),
                kind: parse_kind(kind)?,
            }),
        })
    }

    /// Add or replace a file with an object already in the store.
    #[staticmethod]
    #[pyo3(signature = (path, object, kind = "regular"))]
    fn stored(path: String, object: &str, kind: &str) -> PyResult<Self> {
        let object: ObjectHash = object.parse().map_err(to_py)?;
        Ok(Change {
            inner: CoreChange::Put(CoreEntry::stored(path, object, parse_kind(kind)?)),
        })
    }

    /// Remove a path present in the tree being built.
    #[staticmethod]
    fn delete(path: String) -> Self {
        Change {
            inner: CoreChange::Delete(path),
        }
    }

    /// Move the parent's file at `from_` to `to`, recording the rename edge.
    #[staticmethod]
    fn rename(from_: String, to: String) -> Self {
        Change {
            inner: CoreChange::Rename { from: from_, to },
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            CoreChange::Put(e) => format!("Change.put({:?})", e.path),
            CoreChange::Delete(p) => format!("Change.delete({:?})", p),
            CoreChange::Rename { from, to } => format!("Change.rename({:?}, {:?})", from, to),
            _ => "Change(...)".to_string(),
        }
    }
}

/// The result of `Repo.save_changes`: the new snapshot and the object of each
/// file the changes put.
#[pyclass(frozen, get_all, module = "velo")]
pub struct Saved {
    snapshot: String,
    written: BTreeMap<String, String>,
}

#[pymethods]
impl Saved {
    fn __repr__(&self) -> String {
        format!("Saved({}, {} written)", self.snapshot, self.written.len())
    }
}

fn collect_changes(changes: &Bound<'_, PyAny>) -> PyResult<Vec<CoreChange>> {
    if let Ok(dict) = changes.cast::<PyDict>() {
        dict.iter()
            .map(|(k, v)| {
                let path = k.extract::<String>()?;
                if v.is_none() {
                    Ok(CoreChange::Delete(path))
                } else {
                    Ok(CoreChange::Put(entry_from_value(path, &v)?))
                }
            })
            .collect()
    } else if let Ok(list) = changes.cast::<PyList>() {
        list.iter()
            .map(|item| {
                let change: PyRef<'_, Change> = item.extract().map_err(|_| {
                    PyTypeError::new_err("a list of changes must contain Change objects")
                })?;
                Ok(change.inner.clone())
            })
            .collect()
    } else {
        Err(PyTypeError::new_err(
            "changes must be a dict or a list of Change",
        ))
    }
}

/// An open velo repository, bound to the thread that created it.
///
/// PyO3's own `unsendable` marker enforces this with a Rust panic, which
/// reaches Python as `PanicException` (a `BaseException`), not a catchable
/// `RuntimeError`. So the same rule is applied here by hand: the owner thread is
/// recorded and every call checks it first. The `Mutex` is not for contention;
/// it is what makes the class `Sync` without `unsafe`, since the check above is
/// what actually keeps other threads out.
#[pyclass(module = "velo")]
pub struct Repo {
    owner: std::thread::ThreadId,
    inner: Mutex<velo_core::Repo>,
    pub(crate) hooks: Hooks,
    /// Where the repository was opened, to reopen it with other merge drivers.
    pub(crate) root: PathBuf,
}

/// The core handle, locked. Dropping it releases the lock first and only then
/// delivers queued change events, so an `on_event` callback never runs inside a
/// core call.
pub(crate) struct CoreGuard<'a> {
    guard: Option<MutexGuard<'a, velo_core::Repo>>,
    hooks: &'a Hooks,
}

impl Deref for CoreGuard<'_> {
    type Target = velo_core::Repo;
    fn deref(&self) -> &velo_core::Repo {
        self.guard.as_deref().expect("guard is held until drop")
    }
}

impl DerefMut for CoreGuard<'_> {
    fn deref_mut(&mut self) -> &mut velo_core::Repo {
        self.guard.as_deref_mut().expect("guard is held until drop")
    }
}

impl Drop for CoreGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        self.hooks.flush();
    }
}

impl Repo {
    fn wrap(repo: velo_core::Repo, root: &Path) -> Self {
        let hooks = Hooks::new();
        Repo {
            owner: std::thread::current().id(),
            inner: Mutex::new(repo.listening(hooks.listener())),
            hooks,
            root: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()),
        }
    }

    pub(crate) fn check_owner(&self) -> PyResult<()> {
        if std::thread::current().id() != self.owner {
            return Err(PyRuntimeError::new_err(
                "velo.Repo is unsendable: it can only be used on the thread that created it",
            ));
        }
        Ok(())
    }

    /// Lock the core handle without ever waiting for it.
    ///
    /// The owner-thread check means a second caller can only be on the owner
    /// thread, inside a core call: a merge driver or an event callback calling
    /// back in. Waiting for that lock would deadlock, so it fails instead.
    pub(crate) fn core(&self) -> PyResult<CoreGuard<'_>> {
        self.check_owner()?;
        match self.inner.try_lock() {
            Ok(guard) => Ok(CoreGuard {
                guard: Some(guard),
                hooks: &self.hooks,
            }),
            Err(TryLockError::WouldBlock) => Err(PyErr::new::<ReentrantCall, _>(REENTRANT)),
            Err(TryLockError::Poisoned(_)) => Err(PyRuntimeError::new_err(
                "velo.Repo is poisoned by an earlier panic",
            )),
        }
    }

    /// Replace the core handle with one carrying the merge drivers: the current
    /// rules plus `add`, or none when `clear`.
    fn rebuild(&self, py: Python<'_>, add: Option<(String, Rule)>, clear: bool) -> PyResult<()> {
        let mut guard = self.core()?;
        let (drivers, list) = if clear {
            (velo_core::Drivers::new(), Vec::new())
        } else {
            self.hooks.drivers_with(py, add).map_err(to_py)?
        };
        // The builders consume the handle, so the new one is opened from the
        // same root with the same listener and the old one is dropped.
        let fresh = velo_core::Repo::open_and_migrate(&self.root)
            .map_err(to_py)?
            .listening(self.hooks.listener())
            .merging(drivers);
        *guard = fresh;
        self.hooks.set_rules(list);
        Ok(())
    }
}

#[pymethods]
impl Repo {
    #[staticmethod]
    fn init(path: PathBuf) -> PyResult<Self> {
        Ok(Repo::wrap(
            velo_core::Repo::init(&path).map_err(to_py)?,
            &path,
        ))
    }

    #[staticmethod]
    fn open(path: PathBuf) -> PyResult<Self> {
        Ok(Repo::wrap(
            velo_core::Repo::open_and_migrate(Path::new(&path)).map_err(to_py)?,
            &path,
        ))
    }

    #[pyo3(signature = (*, branch, message, entries, parent = None, merge_parent = None,
        meta = None, author = None, timestamp_ms = None, renames = None))]
    #[allow(clippy::too_many_arguments)]
    fn save_tree(
        &self,
        py: Python<'_>,
        branch: &str,
        message: &str,
        entries: &Bound<'_, PyAny>,
        parent: Option<&str>,
        merge_parent: Option<&str>,
        meta: Option<BTreeMap<String, BTreeMap<String, String>>>,
        author: Option<PyRef<'_, Author>>,
        timestamp_ms: Option<i64>,
        renames: Option<Vec<(String, String)>>,
    ) -> PyResult<String> {
        let branch = branch_name(branch)?;
        let parent = parent.map(snapshot_id).transpose()?;
        let merge_parent = merge_parent.map(snapshot_id).transpose()?;
        let entries = collect_entries(entries)?;
        let snapshot_meta = build_meta(meta)?;
        let renames: Vec<(PathBuf, PathBuf)> = renames
            .unwrap_or_default()
            .into_iter()
            .map(|(from, to)| (PathBuf::from(from), PathBuf::from(to)))
            .collect();

        let author = author.as_ref().map(|a| &a.inner);
        self.check_owner()?;
        let id = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            let guard = match repo.write() {
                Ok(g) => g,
                Err(e) => return Ok(Err(e)),
            };
            Ok(guard.save_tree(SaveTree {
                branch: &branch,
                parent: parent.as_ref(),
                merge_parent: merge_parent.as_ref(),
                message,
                entries,
                meta: snapshot_meta,
                author,
                renames: &renames,
                timestamp_ms,
            }))
        })?;
        Ok(id.map_err(to_py)?.as_str().to_string())
    }

    /// Save `parent` plus `changes` as a snapshot, without sending the rest of
    /// the tree. Returns the snapshot id and the object id of each file put.
    #[pyo3(signature = (*, branch, message, changes, parent = None, merge_parent = None,
        meta = None, author = None, timestamp_ms = None))]
    #[allow(clippy::too_many_arguments)]
    fn save_changes(
        &self,
        py: Python<'_>,
        branch: &str,
        message: &str,
        changes: &Bound<'_, PyAny>,
        parent: Option<&str>,
        merge_parent: Option<&str>,
        meta: Option<BTreeMap<String, BTreeMap<String, String>>>,
        author: Option<PyRef<'_, Author>>,
        timestamp_ms: Option<i64>,
    ) -> PyResult<Saved> {
        let branch = branch_name(branch)?;
        let parent = parent.map(snapshot_id).transpose()?;
        let merge_parent = merge_parent.map(snapshot_id).transpose()?;
        let changes = collect_changes(changes)?;
        let snapshot_meta = build_meta(meta)?;
        let author = author.as_ref().map(|a| &a.inner);
        self.check_owner()?;
        let saved = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            let guard = match repo.write() {
                Ok(g) => g,
                Err(e) => return Ok(Err(e)),
            };
            let mut spec = SaveChanges::new(&branch, parent.as_ref(), message);
            spec.merge_parent = merge_parent.as_ref();
            spec.changes = changes;
            spec.meta = snapshot_meta;
            spec.author = author;
            spec.timestamp_ms = timestamp_ms;
            Ok(guard.save_changes(spec))
        })?;
        let saved = saved.map_err(to_py)?;
        Ok(Saved {
            snapshot: saved.snapshot.as_str().to_string(),
            written: saved
                .written
                .into_iter()
                .map(|(p, o)| (p, o.as_str().to_string()))
                .collect(),
        })
    }

    fn tree_at(&self, py: Python<'_>, id: &str) -> PyResult<Vec<TreeFile>> {
        let id = snapshot_id(id)?;
        self.check_owner()?;
        let files = py.detach(|| -> PyResult<_> { Ok(self.core()?.tree_at(&id)) })?;
        let files = files.map_err(to_py)?;
        Ok(files
            .into_iter()
            .map(|f| TreeFile {
                path: f.path,
                object: f.object.as_str().to_string(),
                kind: kind_name(f.kind).to_string(),
            })
            .collect())
    }

    fn read_file_at<'py>(
        &self,
        py: Python<'py>,
        id: &str,
        path: &str,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let data = self
            .core()?
            .read_file_at(&snapshot_id(id)?, path)
            .map_err(to_py)?;
        Ok(PyBytes::new(py, &data))
    }

    fn read_files_at<'py>(
        &self,
        py: Python<'py>,
        id: &str,
        paths: Vec<String>,
    ) -> PyResult<Vec<(String, Bound<'py, PyBytes>)>> {
        let id = snapshot_id(id)?;
        self.check_owner()?;
        let files = py.detach(|| -> PyResult<_> { Ok(self.core()?.read_files_at(&id, &paths)) })?;
        Ok(files
            .map_err(to_py)?
            .into_iter()
            .map(|(p, data)| (p, PyBytes::new(py, &data)))
            .collect())
    }

    #[pyo3(signature = (id, dest, only=None, ignore=None))]
    fn materialize(
        &self,
        py: Python<'_>,
        id: &str,
        dest: PathBuf,
        only: Option<Vec<String>>,
        ignore: Option<Vec<String>>,
    ) -> PyResult<usize> {
        let id = snapshot_id(id)?;
        let mut scope = velo_core::Scope::new();
        for p in only.iter().flatten() {
            scope = scope.only(p).map_err(to_py)?;
        }
        for p in ignore.iter().flatten() {
            scope = scope.ignore(p).map_err(to_py)?;
        }
        self.check_owner()?;
        let done = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            Ok(velo_core::commands::restore::materialize(
                &repo,
                &id,
                &dest,
                velo_core::commands::restore::MaterializeOptions::default().scope(&scope),
            ))
        })?;
        Ok(done.map_err(to_py)?.files)
    }

    fn read_object<'py>(&self, py: Python<'py>, object: &str) -> PyResult<Bound<'py, PyBytes>> {
        let object: ObjectHash = object.parse().map_err(to_py)?;
        let data = self.core()?.read_object(&object).map_err(to_py)?;
        Ok(PyBytes::new(py, &data))
    }

    fn snapshot(&self, id: &str) -> PyResult<Entry> {
        Ok(self
            .core()?
            .snapshot(&snapshot_id(id)?)
            .map_err(to_py)?
            .into())
    }

    fn snapshot_meta(&self, id: &str) -> PyResult<BTreeMap<String, BTreeMap<String, String>>> {
        let meta = self
            .core()?
            .snapshot_meta(&snapshot_id(id)?)
            .map_err(to_py)?;
        let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (namespace, key, value) in meta.iter() {
            out.entry(namespace.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
        Ok(out)
    }

    fn resolve(&self, spec: &str) -> PyResult<String> {
        let id = velo_core::commands::resolve_snapshot_id(&*self.core()?, spec).map_err(to_py)?;
        Ok(id.as_str().to_string())
    }

    fn branch_tip(&self, branch: &str) -> PyResult<Option<String>> {
        let tip = self
            .core()?
            .branch_tip(&branch_name(branch)?)
            .map_err(to_py)?;
        Ok(tip.map(|t| t.as_str().to_string()))
    }

    fn head_token(&self) -> PyResult<u64> {
        self.core()?.head_token().map_err(to_py)
    }

    /// Snapshots, newest first.
    ///
    /// `from_` takes precedence over `branch` and `all`. With none of the three
    /// the ancestry of the working tree's position is used, which a repository
    /// without a working tree does not have: pass `from_` or `branch` there.
    /// Each `meta` tuple is `(namespace, key, value)` or `(namespace, key)`.
    #[pyo3(signature = (*, from_ = None, branch = None, all = false, paths = None,
        limit = None, meta = None))]
    #[allow(clippy::too_many_arguments)]
    fn history(
        &self,
        py: Python<'_>,
        from_: Option<&str>,
        branch: Option<&str>,
        all: bool,
        paths: Option<Vec<String>>,
        limit: Option<usize>,
        meta: Option<Vec<Vec<String>>>,
    ) -> PyResult<Vec<Entry>> {
        let from = from_.map(snapshot_id).transpose()?;
        let branch = branch.map(branch_name).transpose()?;
        let paths: Vec<PathBuf> = paths
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let meta = meta.unwrap_or_default();
        let filters = meta_filters(&meta)?;
        self.check_owner()?;
        let history = py.detach(|| -> PyResult<_> {
            Ok(velo_core::commands::history::run(
                &*self.core()?,
                velo_core::commands::history::Options {
                    all,
                    branch: branch.as_ref(),
                    from: from.as_ref(),
                    paths: &path_refs,
                    meta: &filters,
                    limit,
                },
            ))
        })?;
        let history = history.map_err(to_py)?;
        Ok(history.entries.into_iter().map(Entry::from).collect())
    }

    /// Snapshots whose metadata satisfies every filter, newest first.
    fn find_snapshots(&self, py: Python<'_>, meta: Vec<Vec<String>>) -> PyResult<Vec<Entry>> {
        let filters = meta_filters(&meta)?;
        self.check_owner()?;
        let found = py.detach(|| -> PyResult<_> { Ok(self.core()?.find_snapshots(&filters)) })?;
        let found = found.map_err(to_py)?;
        Ok(found.into_iter().map(Entry::from).collect())
    }

    /// Attribute each line of `path` to the snapshot that last changed it.
    ///
    /// `lines` is `(first, last)`, 1-based and inclusive.
    #[pyo3(signature = (path, *, at = None, lines = None))]
    fn blame(
        &self,
        py: Python<'_>,
        path: &str,
        at: Option<&str>,
        lines: Option<(usize, usize)>,
    ) -> PyResult<Blame> {
        let at = at.map(snapshot_id).transpose()?;
        let range = match lines {
            Some((first, last)) => {
                if first == 0 || last < first {
                    return Err(to_py(velo_core::Error::invalid(
                        "lines must be (first, last), 1-based and inclusive, with first <= last.",
                    )));
                }
                Some(first..last + 1)
            }
            None => None,
        };
        self.check_owner()?;
        let blame = py.detach(|| -> PyResult<_> {
            Ok(velo_core::commands::blame::run(
                &*self.core()?,
                Path::new(path),
                velo_core::commands::blame::Options {
                    at: at.as_ref(),
                    lines: range,
                    ..Default::default()
                },
            ))
        })?;
        let blame = blame.map_err(to_py)?;
        Ok(Blame::from(blame))
    }

    /// The nearest common ancestor of two snapshots, if they share history.
    fn merge_base(&self, py: Python<'_>, a: &str, b: &str) -> PyResult<Option<String>> {
        let (a, b) = (snapshot_id(a)?, snapshot_id(b)?);
        self.check_owner()?;
        let base = py.detach(|| -> PyResult<_> {
            Ok(velo_core::commands::merge::merge_base(
                &*self.core()?,
                &a,
                &b,
            ))
        })?;
        let base = base.map_err(to_py)?;
        Ok(base.map(|b| b.as_str().to_string()))
    }

    /// What merging `theirs` into `ours` would do, with nothing done.
    ///
    /// The GIL is released around the core call so a merge driver, which may
    /// run on another thread, can take it. A driver that raises makes this
    /// raise the same exception.
    fn merge_plan(&self, py: Python<'_>, ours: &str, theirs: &str) -> PyResult<MergePlan> {
        let (ours, theirs) = (snapshot_id(ours)?, snapshot_id(theirs)?);
        self.check_owner()?;
        self.hooks.take_error();
        let plan = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            Ok(velo_core::commands::merge::plan(&repo, &ours, &theirs))
        })?;
        if let Some(e) = self.hooks.take_error() {
            return Err(e);
        }
        Ok(plan.map_err(to_py)?.into())
    }

    /// Record the merge of `theirs` into `ours` as a snapshot on `branch`.
    ///
    /// `resolutions` maps a path to `"ours"`, `"theirs"`, `None` (delete) or
    /// the bytes to write. A conflict left without one raises `Conflicts`.
    ///
    /// An exception raised by a Python merge driver is re-raised and nothing
    /// is committed. A driver's conflict that has a resolution would not stop
    /// the commit by itself, so with a Python driver and resolutions the merge
    /// is planned first and the exception surfaces before any write; the
    /// driver therefore runs twice.
    #[pyo3(signature = (*, branch, ours, theirs, message, resolutions = None, meta = None,
        author = None, timestamp_ms = None))]
    #[allow(clippy::too_many_arguments)]
    fn merge_commit(
        &self,
        py: Python<'_>,
        branch: &str,
        ours: &str,
        theirs: &str,
        message: &str,
        resolutions: Option<BTreeMap<String, Option<Bound<'_, PyAny>>>>,
        meta: Option<BTreeMap<String, BTreeMap<String, String>>>,
        author: Option<PyRef<'_, Author>>,
        timestamp_ms: Option<i64>,
    ) -> PyResult<String> {
        let branch = branch_name(branch)?;
        let ours = snapshot_id(ours)?;
        let theirs = snapshot_id(theirs)?;
        let resolutions = parse_resolutions(resolutions.unwrap_or_default())?;
        let meta = build_meta(meta)?;
        let author = author.as_ref().map(|a| &a.inner);
        self.check_owner()?;
        self.hooks.take_error();
        if !resolutions.is_empty() && self.hooks.has_custom_driver() {
            py.detach(|| -> PyResult<_> {
                let repo = self.core()?;
                let _ = velo_core::commands::merge::plan(&repo, &ours, &theirs);
                Ok(())
            })?;
            if let Some(e) = self.hooks.take_error() {
                return Err(e);
            }
        }
        let id = py.detach(|| -> PyResult<_> {
            let repo = self.core()?;
            let guard = match repo.write() {
                Ok(g) => g,
                Err(e) => return Ok(Err(e)),
            };
            Ok(velo_core::commands::merge::commit(
                &guard,
                velo_core::commands::merge::MergeCommit {
                    branch: &branch,
                    ours: &ours,
                    theirs: &theirs,
                    resolutions: &resolutions,
                    message,
                    meta,
                    author,
                    timestamp_ms,
                },
            ))
        })?;
        if let Some(e) = self.hooks.take_error() {
            return Err(e);
        }
        Ok(id.map_err(to_py)?.as_str().to_string())
    }

    /// Call `callback(event)` for every change committed through this handle.
    ///
    /// `event` is a dict with `kind` (`"Saved"`, `"Merged"`, `"RefMoved"` or
    /// `"Imported"`) and that event's fields. Callbacks run after the call that
    /// made the change has returned and released the repository, on the calling
    /// thread. Events are in-process only: this handle hears its own writes,
    /// not those of another handle or process. Returns `callback`.
    fn on_event(&self, py: Python<'_>, callback: Py<PyAny>) -> PyResult<Py<PyAny>> {
        self.check_owner()?;
        if !callback.bind(py).is_callable() {
            return Err(PyTypeError::new_err("on_event needs a callable"));
        }
        self.hooks.add_callback(callback.clone_ref(py));
        Ok(callback)
    }

    /// Merge files matching glob `pattern` with `driver`: `"json"`, `"yaml"`,
    /// `"toml"` or a function `(ancestor, ours, theirs) -> str | None`, where
    /// `None` is a conflict. The first matching rule wins.
    ///
    /// A function runs inside the merge, under the repository lock, so it must
    /// not call the repository: that raises `ReentrantCall`. An exception it
    /// raises is re-raised by `merge_plan` or `merge_commit`.
    fn set_merge_driver(
        &self,
        py: Python<'_>,
        pattern: &str,
        driver: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let rule = Rule::parse(driver)?;
        self.rebuild(py, Some((pattern.to_string(), rule)), false)
    }

    /// Go back to line-based merging for every path.
    fn clear_merge_drivers(&self, py: Python<'_>) -> PyResult<()> {
        self.rebuild(py, None, true)
    }

    fn branches(&self) -> PyResult<Vec<Branch>> {
        let list = velo_core::commands::branches::list(&*self.core()?).map_err(to_py)?;
        Ok(list.into_iter().map(Branch::from).collect())
    }

    /// Create `name`, at snapshot `at` or unborn when `at` is `None`.
    #[pyo3(signature = (name, at = None))]
    fn create_branch(&self, name: &str, at: Option<&str>) -> PyResult<()> {
        let name = branch_name(name)?;
        let at = at.map(snapshot_id).transpose()?;
        let repo = self.core()?;
        let guard = repo.write().map_err(to_py)?;
        velo_core::commands::branches::create(&guard, &name, at.as_ref()).map_err(to_py)
    }

    /// Point an existing branch at `to`. Nothing is rewritten or deleted.
    fn set_branch_tip(&self, name: &str, to: &str) -> PyResult<()> {
        let name = branch_name(name)?;
        let to = snapshot_id(to)?;
        let repo = self.core()?;
        let guard = repo.write().map_err(to_py)?;
        velo_core::commands::branches::set_tip(&guard, &name, &to).map_err(to_py)
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Repo>()?;
    m.add_class::<TreeEntry>()?;
    m.add_class::<TreeFile>()?;
    m.add_class::<Change>()?;
    m.add_class::<Saved>()?;
    m.add_class::<Author>()?;
    m.add_class::<Entry>()?;
    crate::history::register(m)?;
    crate::merge::register(m)?;
    Ok(())
}
