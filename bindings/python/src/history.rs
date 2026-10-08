//! History, blame and branch result types, and the metadata-filter parser.

use pyo3::prelude::*;
use velo_core::commands::blame as core_blame;
use velo_core::commands::branches as core_branches;
use velo_core::commands::history::MetaFilter;

use crate::errors::to_py;
use crate::repo::{utc_datetime, Author};

/// Turn `(namespace, key, value)` / `(namespace, key)` tuples into filters.
pub fn meta_filters(meta: &[Vec<String>]) -> PyResult<Vec<MetaFilter<'_>>> {
    meta.iter()
        .map(|t| match t.as_slice() {
            [namespace, key, value] => Ok(MetaFilter::equals(namespace, key, value)),
            [namespace, key] => Ok(MetaFilter::has(namespace, key)),
            _ => Err(to_py(velo_core::Error::invalid(
                "a meta filter is (namespace, key, value) or (namespace, key).",
            ))),
        })
        .collect()
}

/// The snapshot a line came from.
#[pyclass(frozen, skip_from_py_object, module = "velo")]
#[derive(Clone)]
pub struct LineOrigin {
    id: String,
    created_at_ms: i64,
    message: String,
    author: Option<(String, Option<String>)>,
    branch: String,
    path: String,
}

#[pymethods]
impl LineOrigin {
    #[getter]
    fn id(&self) -> &str {
        &self.id
    }

    #[getter]
    fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    /// A timezone-aware `datetime` in UTC.
    #[getter]
    fn created_at<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        utc_datetime(py, self.created_at_ms)
    }

    #[getter]
    fn message(&self) -> &str {
        &self.message
    }

    #[getter]
    fn author(&self) -> PyResult<Option<Author>> {
        self.author
            .as_ref()
            .map(|(name, email)| Author::new(name.clone(), email.clone()))
            .transpose()
    }

    #[getter]
    fn branch(&self) -> &str {
        &self.branch
    }

    /// The file's path at that snapshot; differs from `Blame.path` across a rename.
    #[getter]
    fn path(&self) -> &str {
        &self.path
    }

    fn __repr__(&self) -> String {
        format!("LineOrigin({}, {:?})", self.id, self.message)
    }
}

/// One attributed unit of the file.
#[pyclass(frozen, skip_from_py_object, get_all, module = "velo")]
#[derive(Clone)]
pub struct BlameLine {
    line_no: usize,
    text: String,
    line_count: usize,
    origin: Option<LineOrigin>,
}

#[pymethods]
impl BlameLine {
    fn __repr__(&self) -> String {
        format!("BlameLine({}, {:?})", self.line_no, self.text)
    }
}

/// Attribution for a file.
#[pyclass(frozen, skip_from_py_object, get_all, module = "velo")]
pub struct Blame {
    path: String,
    snapshot: String,
    lines: Vec<BlameLine>,
}

#[pymethods]
impl Blame {
    fn __repr__(&self) -> String {
        format!("Blame({:?}, {} lines)", self.path, self.lines.len())
    }
}

impl From<core_blame::Blame> for Blame {
    fn from(b: core_blame::Blame) -> Self {
        Blame {
            path: b.path.to_string_lossy().into_owned(),
            snapshot: b.snapshot.as_str().to_string(),
            lines: b
                .lines
                .into_iter()
                .map(|l| BlameLine {
                    line_no: l.line_no,
                    text: l.text,
                    line_count: l.line_count,
                    origin: l.origin.map(|o| LineOrigin {
                        id: o.hash.as_str().to_string(),
                        created_at_ms: o.created_at.timestamp_millis(),
                        message: o.message,
                        author: o
                            .author
                            .map(|a| (a.name().to_string(), a.email().map(str::to_string))),
                        branch: o.branch.as_str().to_string(),
                        path: o.path.to_string_lossy().into_owned(),
                    }),
                })
                .collect(),
        }
    }
}

/// A branch and where it stands.
#[pyclass(frozen, skip_from_py_object, get_all, module = "velo")]
pub struct Branch {
    name: String,
    is_current: bool,
    /// Tip snapshot id; `None` for an unborn branch.
    tip: Option<String>,
    tip_message: Option<String>,
    tip_created_at_ms: Option<i64>,
}

#[pymethods]
impl Branch {
    fn __repr__(&self) -> String {
        format!("Branch({:?}, tip={:?})", self.name, self.tip)
    }
}

impl From<core_branches::Branch> for Branch {
    fn from(b: core_branches::Branch) -> Self {
        Branch {
            name: b.name.as_str().to_string(),
            is_current: b.is_current,
            tip: b.tip.as_ref().map(|t| t.hash.as_str().to_string()),
            tip_message: b.tip.as_ref().map(|t| t.message.clone()),
            tip_created_at_ms: b.tip.as_ref().map(|t| t.created_at.timestamp_millis()),
        }
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<LineOrigin>()?;
    m.add_class::<BlameLine>()?;
    m.add_class::<Blame>()?;
    m.add_class::<Branch>()?;
    Ok(())
}
