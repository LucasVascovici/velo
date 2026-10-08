//! Merge plan types and the resolution parser.

use std::collections::BTreeMap;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use velo_core::commands::apply::FileAction;
use velo_core::commands::merge::{self as core_merge, PlannedChange, Resolution};

use crate::errors::to_py;

fn action_name(action: FileAction) -> &'static str {
    match action {
        FileAction::Deleted => "deleted",
        FileAction::Added => "added",
        FileAction::Updated => "updated",
        FileAction::AutoMerged => "auto_merged",
        FileAction::KeptOurs => "kept_ours",
        FileAction::Conflicted => "conflicted",
    }
}

/// `"ours"`, `"theirs"`, `None` or bytes, per path.
pub fn parse_resolutions(
    given: BTreeMap<String, Option<Bound<'_, PyAny>>>,
) -> PyResult<Vec<(String, Resolution)>> {
    given
        .into_iter()
        .map(|(path, value)| {
            let resolution = match value {
                None => Resolution::Delete,
                Some(v) if v.is_none() => Resolution::Delete,
                Some(v) => {
                    if let Ok(bytes) = v.cast::<PyBytes>() {
                        Resolution::Content(bytes.as_bytes().to_vec())
                    } else {
                        match v.extract::<String>().ok().as_deref() {
                            Some("ours") => Resolution::Ours,
                            Some("theirs") => Resolution::Theirs,
                            _ => {
                                return Err(to_py(velo_core::Error::invalid(format!(
                                    "resolution for '{}' must be 'ours', 'theirs', None or bytes.",
                                    path
                                ))))
                            }
                        }
                    }
                }
            };
            Ok((path, resolution))
        })
        .collect()
}

/// One path a merge would touch.
#[pyclass(frozen, skip_from_py_object, module = "velo")]
#[derive(Clone)]
pub struct PlannedFile {
    path: String,
    action: &'static str,
    object: Option<String>,
    mode: Option<i64>,
    content: Option<Vec<u8>>,
    base: Option<String>,
    ours: Option<String>,
    theirs: Option<String>,
}

#[pymethods]
impl PlannedFile {
    #[getter]
    fn path(&self) -> &str {
        &self.path
    }

    /// One of `deleted`, `added`, `updated`, `auto_merged`, `kept_ours`, `conflicted`.
    #[getter]
    fn action(&self) -> &str {
        self.action
    }

    /// The object taken from theirs, for `added` and `updated`.
    #[getter]
    fn object(&self) -> Option<&str> {
        self.object.as_deref()
    }

    #[getter]
    fn mode(&self) -> Option<i64> {
        self.mode
    }

    /// The merged bytes, for `auto_merged`.
    #[getter]
    fn content<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.content.as_deref().map(|c| PyBytes::new(py, c))
    }

    /// For `conflicted`: the object ids of each side, `None` where absent.
    #[getter]
    fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    #[getter]
    fn ours(&self) -> Option<&str> {
        self.ours.as_deref()
    }

    #[getter]
    fn theirs(&self) -> Option<&str> {
        self.theirs.as_deref()
    }

    fn __repr__(&self) -> String {
        format!("PlannedFile({:?}, {})", self.path, self.action)
    }
}

/// Everything a merge would do.
#[pyclass(frozen, skip_from_py_object, module = "velo")]
pub struct MergePlan {
    base: Option<String>,
    files: Vec<PlannedFile>,
}

#[pymethods]
impl MergePlan {
    #[getter]
    fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    #[getter]
    fn files(&self) -> Vec<PlannedFile> {
        self.files.clone()
    }

    #[getter]
    fn is_clean(&self) -> bool {
        self.conflicts().is_empty()
    }

    #[getter]
    fn conflicts(&self) -> Vec<PlannedFile> {
        self.files
            .iter()
            .filter(|f| f.action == "conflicted")
            .cloned()
            .collect()
    }

    fn __repr__(&self) -> String {
        format!("MergePlan({} files)", self.files.len())
    }
}

impl From<core_merge::MergePlan> for MergePlan {
    fn from(plan: core_merge::MergePlan) -> Self {
        let id = |o: &velo_core::ObjectHash| o.as_str().to_string();
        let files = plan
            .files
            .into_iter()
            .map(|f| {
                let mut out = PlannedFile {
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
                        out.object = Some(id(&object));
                        out.mode = Some(mode);
                    }
                    PlannedChange::AutoMerge { content, mode } => {
                        out.content = Some(content);
                        out.mode = Some(mode);
                    }
                    PlannedChange::Conflict { base, ours, theirs } => {
                        out.base = base.as_ref().map(id);
                        out.ours = ours.as_ref().map(id);
                        out.theirs = theirs.as_ref().map(id);
                    }
                    _ => {}
                }
                out
            })
            .collect();
        MergePlan {
            base: plan.base.map(|b| b.as_str().to_string()),
            files,
        }
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PlannedFile>()?;
    m.add_class::<MergePlan>()?;
    Ok(())
}
