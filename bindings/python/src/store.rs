//! Retention, compaction, tags and fsck: the store-only maintenance an
//! embedder needs.
//!
//! A second `#[pymethods]` block on `Repo`, kept apart from `repo.rs` so the
//! two grow independently. Reports are plain dicts and lists: they are read
//! once and thrown away, so a class per report would only add surface.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use velo_core::commands::compact::{self, Protected};
use velo_core::commands::fsck::{self, Cruft};
use velo_core::commands::retention::{self, Policy, Tier};
use velo_core::commands::tag;
use velo_core::{SnapshotId, TagName};

use crate::errors::to_py;
use crate::repo::{branch_name, snapshot_id, Repo};

/// A tag and what it points at.
#[pyclass(frozen, get_all, module = "velo")]
pub struct Tag {
    name: String,
    snapshot: String,
    /// The tagged snapshot's message, `None` once that snapshot is gone.
    message: Option<String>,
}

#[pymethods]
impl Tag {
    fn __repr__(&self) -> String {
        format!("Tag({:?}, {})", self.name, self.snapshot)
    }
}

/// The variant name of a `Debug`-printed finding, e.g. `MissingObject`.
fn kind_of(finding: &dyn std::fmt::Debug) -> String {
    let text = format!("{:?}", finding);
    text.split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_string()
}

fn protected_name(why: &Protected) -> String {
    match why {
        Protected::Tagged => "tagged".into(),
        Protected::Merge => "merge".into(),
        Protected::ParentOfOutside => "parent_of_outside".into(),
        Protected::Published => "published".into(),
        Protected::CheckedOut => "checked_out".into(),
        other => other.to_string(),
    }
}

fn compaction_into<'py>(
    py: Python<'py>,
    out: &Bound<'py, PyDict>,
    done: Option<&compact::Compaction>,
) -> PyResult<()> {
    let squashed = PyList::empty(py);
    let rewritten = PyList::empty(py);
    if let Some(done) = done {
        for s in &done.squashed {
            let d = PyDict::new(py);
            d.set_item("into", s.into.as_str())?;
            let removed: Vec<&str> = s.removed.iter().map(|r| r.as_str()).collect();
            d.set_item("removed", removed)?;
            squashed.append(d)?;
        }
        for (old, new) in &done.rewritten {
            rewritten.append((old.as_str(), new.as_str()))?;
        }
    }
    out.set_item("squashed", squashed)?;
    out.set_item("rewritten", rewritten)?;
    Ok(())
}

fn findings<'py>(
    py: Python<'py>,
    items: &[Cruft],
    describe: impl Fn(&Cruft) -> String,
) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty(py);
    for c in items {
        let d = PyDict::new(py);
        d.set_item("kind", kind_of(c))?;
        d.set_item("detail", describe(c))?;
        list.append(d)?;
    }
    Ok(list)
}

#[pymethods]
impl Repo {
    /// Thin `branch` by a retention policy.
    ///
    /// `tiers` is a list of `(older_than_ms, keep_one_per_ms)`, ascending by
    /// age; `None` is the default policy. Returns `branch`, `examined`,
    /// `removed`, `dry_run`, `squashed`, `rewritten` and `protected`.
    #[pyo3(signature = (branch, tiers = None, now_ms = None, dry_run = false))]
    fn retention<'py>(
        &self,
        py: Python<'py>,
        branch: &str,
        tiers: Option<Vec<(i64, i64)>>,
        now_ms: Option<i64>,
        dry_run: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let branch = branch_name(branch)?;
        let policy = match tiers {
            Some(tiers) => Policy {
                tiers: tiers
                    .into_iter()
                    .map(|(older_than_ms, keep_one_per_ms)| Tier {
                        older_than_ms,
                        keep_one_per_ms,
                    })
                    .collect(),
            },
            None => Policy::default(),
        };
        let repo = self.core()?;
        let guard = repo.write().map_err(to_py)?;
        let report = retention::apply(
            &guard,
            &branch,
            &policy,
            retention::Options {
                now_ms,
                dry_run,
                ..Default::default()
            },
        )
        .map_err(to_py)?;
        drop(guard);
        let out = PyDict::new(py);
        out.set_item("branch", report.branch.as_str())?;
        out.set_item("examined", report.examined)?;
        out.set_item("removed", report.removed)?;
        out.set_item("dry_run", dry_run)?;
        compaction_into(py, &out, report.compaction.as_ref())?;
        let protected = PyList::empty(py);
        for (id, why) in &report.protected {
            let d = PyDict::new(py);
            d.set_item("id", id.as_str())?;
            d.set_item("reason", protected_name(why))?;
            protected.append(d)?;
        }
        out.set_item("protected", protected)?;
        Ok(out)
    }

    /// Squash each `(oldest, newest)` range of `branch`'s first-parent chain.
    ///
    /// Returns `branch`, `dry_run`, `squashed` (each `{into, removed}`) and
    /// `rewritten` (`(old, new)` pairs). Afterwards an old id raises
    /// `Compacted`, whose `into` names its replacement.
    #[pyo3(signature = (branch, ranges, dry_run = false))]
    fn compact<'py>(
        &self,
        py: Python<'py>,
        branch: &str,
        ranges: Vec<(String, String)>,
        dry_run: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let branch = branch_name(branch)?;
        let ids: Vec<(SnapshotId, SnapshotId)> = ranges
            .iter()
            .map(|(a, b)| Ok((snapshot_id(a)?, snapshot_id(b)?)))
            .collect::<PyResult<_>>()?;
        let ranges: Vec<compact::Range<'_>> = ids
            .iter()
            .map(|(oldest, newest)| compact::Range { oldest, newest })
            .collect();
        let repo = self.core()?;
        let guard = repo.write().map_err(to_py)?;
        let done = compact::run(
            &guard,
            &branch,
            &ranges,
            compact::Options {
                dry_run,
                ..Default::default()
            },
        )
        .map_err(to_py)?;
        drop(guard);
        let out = PyDict::new(py);
        out.set_item("branch", done.branch.as_str())?;
        out.set_item("dry_run", done.dry_run)?;
        compaction_into(py, &out, Some(&done))?;
        Ok(out)
    }

    /// Every tag, ordered by name.
    fn tags(&self) -> PyResult<Vec<Tag>> {
        let tags = tag::list(&*self.core()?).map_err(to_py)?;
        Ok(tags
            .into_iter()
            .map(|t| Tag {
                name: t.name.as_str().to_string(),
                snapshot: t.snapshot.as_str().to_string(),
                message: t.message,
            })
            .collect())
    }

    /// Tag snapshot `at` (a name, id or prefix), or the current position when
    /// `None`. Fails if the name is taken unless `force`. Returns the id.
    #[pyo3(signature = (name, at = None, force = false))]
    fn create_tag(&self, name: &str, at: Option<&str>, force: bool) -> PyResult<String> {
        let name: TagName = name.parse().map_err(to_py)?;
        let repo = self.core()?;
        let at = at
            .map(|spec| velo_core::commands::resolve_snapshot_id(&repo, spec))
            .transpose()
            .map_err(to_py)?;
        let guard = repo.write().map_err(to_py)?;
        let made = tag::create(&guard, &name, at.as_ref(), force).map_err(to_py)?;
        Ok(made.snapshot.as_str().to_string())
    }

    fn delete_tag(&self, name: &str) -> PyResult<()> {
        let name: TagName = name.parse().map_err(to_py)?;
        let repo = self.core()?;
        let guard = repo.write().map_err(to_py)?;
        tag::delete(&guard, &name).map_err(to_py)
    }

    /// Verify the repository. Returns `problems` (damage), `cruft` (harmless
    /// leftovers) and `repaired`, each a list of `{kind, detail}`. With
    /// `repair`, cruft is cleaned up; damage is only ever reported.
    #[pyo3(signature = (repair = false))]
    fn fsck<'py>(&self, py: Python<'py>, repair: bool) -> PyResult<Bound<'py, PyDict>> {
        let repo = self.core()?;
        let report = if repair {
            let guard = repo.write().map_err(to_py)?;
            fsck::repair(&guard)
        } else {
            fsck::check(&repo)
        }
        .map_err(to_py)?;
        let out = PyDict::new(py);
        let problems = PyList::empty(py);
        for p in &report.problems {
            let d = PyDict::new(py);
            d.set_item("kind", kind_of(p))?;
            d.set_item("detail", p.to_string())?;
            problems.append(d)?;
        }
        out.set_item("problems", problems)?;
        out.set_item("cruft", findings(py, &report.cruft, Cruft::describe)?)?;
        out.set_item(
            "repaired",
            findings(py, &report.repaired, Cruft::describe_repaired)?,
        )?;
        Ok(out)
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Tag>()?;
    Ok(())
}
