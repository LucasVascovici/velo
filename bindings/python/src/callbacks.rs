//! Change events and merge drivers, with the rule that keeps both safe.
//!
//! # Threading
//!
//! A merge driver runs **inside** a core call, under the repository lock, and
//! `merge_plan` / `merge_commit` release the GIL around that call, so the
//! driver may run on a rayon worker as well as on the calling thread. It takes
//! the GIL itself (`Python::attach`) to call the Python function.
//!
//! # Re-entrancy
//!
//! A driver that called `repo.something()` would need the lock its own caller
//! holds. [`Repo::core`](crate::repo::Repo) therefore never blocks on the lock:
//! a busy handle fails at once with `ReentrantCall`.
//!
//! Events are different by construction. The core listener only queues them;
//! the queue is drained when the core call has returned and the lock is
//! released, on the calling thread with the GIL held, so a callback is never
//! inside a core call.

use std::sync::{Arc, Mutex};

use pyo3::prelude::*;
use pyo3::types::PyDict;
use velo_core::events::{Event, Listener, Ref};
use velo_core::merge::{MergeDriver, MergeResult};
use velo_core::{Drivers, Error, Result};

use crate::errors::to_py;

/// Events committed through the handle, waiting for the core call to return.
#[derive(Default)]
pub struct Queue(Mutex<Vec<Event>>);

impl Queue {
    pub fn drain(&self) -> Vec<Event> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The listener installed on the core handle; it only queues.
pub struct QueueListener(pub Arc<Queue>);

impl Listener for QueueListener {
    fn notify(&self, event: &Event) {
        self.0
             .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.clone());
    }
}

/// The first exception a driver raised during the current core call.
pub type ErrorSlot = Arc<Mutex<Option<PyErr>>>;

/// A merge driver written in Python.
struct PyDriver {
    func: Py<PyAny>,
    error: ErrorSlot,
}

impl PyDriver {
    fn fail(&self, err: PyErr) {
        let mut slot = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(err);
        }
    }
}

impl MergeDriver for PyDriver {
    fn name(&self) -> &str {
        "python"
    }

    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        // After the first failure every later path is a conflict too, so the
        // merge cannot go on to record anything built on a failed driver.
        let conflict = || MergeResult::Conflicted(Vec::new());
        if self
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return conflict();
        }
        Python::attach(|py| match self.func.call1(py, (ancestor, ours, theirs)) {
            Err(e) => {
                self.fail(e);
                conflict()
            }
            Ok(value) => {
                let value = value.bind(py);
                if value.is_none() {
                    conflict()
                } else {
                    match value.extract::<String>() {
                        Ok(text) => MergeResult::Clean(text),
                        Err(_) => {
                            self.fail(pyo3::exceptions::PyTypeError::new_err(
                                "a merge driver must return a str, or None for a conflict",
                            ));
                            conflict()
                        }
                    }
                }
            }
        })
    }
}

/// What `set_merge_driver` was given.
pub enum Rule {
    Builtin(&'static str),
    Custom(Py<PyAny>),
}

impl Rule {
    pub fn parse(driver: &Bound<'_, PyAny>) -> PyResult<Rule> {
        if let Ok(name) = driver.extract::<String>() {
            return match name.as_str() {
                "json" => Ok(Rule::Builtin("json")),
                "yaml" => Ok(Rule::Builtin("yaml")),
                "toml" => Ok(Rule::Builtin("toml")),
                other => Err(to_py(Error::invalid(format!(
                    "'{}' is not a merge driver; expected 'json', 'yaml', 'toml' or a function.",
                    other
                )))),
            };
        }
        if driver.is_callable() {
            return Ok(Rule::Custom(driver.clone().unbind()));
        }
        Err(pyo3::exceptions::PyTypeError::new_err(
            "a merge driver must be 'json', 'yaml', 'toml' or a callable",
        ))
    }

    fn duplicate(&self, py: Python<'_>) -> Rule {
        match self {
            Rule::Builtin(n) => Rule::Builtin(n),
            Rule::Custom(f) => Rule::Custom(f.clone_ref(py)),
        }
    }
}

/// Everything a handle keeps besides the core repository.
pub struct Hooks {
    pub queue: Arc<Queue>,
    callbacks: Mutex<Vec<Py<PyAny>>>,
    rules: Mutex<Vec<(String, Rule)>>,
    pub error: ErrorSlot,
}

impl Hooks {
    pub fn new() -> Hooks {
        Hooks {
            queue: Arc::new(Queue::default()),
            callbacks: Mutex::new(Vec::new()),
            rules: Mutex::new(Vec::new()),
            error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn listener(&self) -> QueueListener {
        QueueListener(self.queue.clone())
    }

    pub fn add_callback(&self, callback: Py<PyAny>) {
        self.callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(callback);
    }

    pub fn has_custom_driver(&self) -> bool {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|(_, r)| matches!(r, Rule::Custom(_)))
    }

    /// The error a driver raised, if any, clearing the slot.
    pub fn take_error(&self) -> Option<PyErr> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// The rule list with `rule` appended (or empty, to clear), as core drivers.
    pub fn drivers_with(
        &self,
        py: Python<'_>,
        add: Option<(String, Rule)>,
    ) -> Result<(Drivers, Vec<(String, Rule)>)> {
        let mut list: Vec<(String, Rule)> = self
            .rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(p, r)| (p.clone(), r.duplicate(py)))
            .collect();
        list.extend(add);
        let mut drivers = Drivers::new();
        for (pattern, rule) in &list {
            drivers = match rule {
                Rule::Builtin("json") => drivers.with(pattern, velo_core::merge::JsonDriver)?,
                Rule::Builtin("yaml") => drivers.with(pattern, velo_core::merge::YamlDriver)?,
                Rule::Builtin(_) => drivers.with(pattern, velo_core::merge::TomlDriver)?,
                Rule::Custom(f) => drivers.with(
                    pattern,
                    PyDriver {
                        func: f.clone_ref(py),
                        error: self.error.clone(),
                    },
                )?,
            };
        }
        Ok((drivers, list))
    }

    pub fn set_rules(&self, list: Vec<(String, Rule)>) {
        *self.rules.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }

    /// Deliver queued events. Called with the repository lock released.
    ///
    /// A callback that raises is reported through `sys.unraisablehook` and
    /// does not fail the call: the write it announces has already committed.
    pub fn flush(&self) {
        let events = self.queue.drain();
        if events.is_empty() {
            return;
        }
        Python::attach(|py| {
            let callbacks: Vec<Py<PyAny>> = self
                .callbacks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|c| c.clone_ref(py))
                .collect();
            if callbacks.is_empty() {
                return;
            }
            for event in &events {
                let Ok(dict) = event_dict(py, event) else {
                    continue;
                };
                for callback in &callbacks {
                    if let Err(e) = callback.call1(py, (&dict,)) {
                        e.write_unraisable(py, Some(callback.bind(py)));
                    }
                }
            }
        });
    }
}

fn event_dict<'py>(py: Python<'py>, event: &Event) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let id = |s: &velo_core::SnapshotId| s.as_str().to_string();
    match event {
        Event::Saved {
            snapshot,
            branch,
            parent,
            merge_parent,
        } => {
            d.set_item("kind", "Saved")?;
            d.set_item("snapshot", id(snapshot))?;
            d.set_item("branch", branch.as_str())?;
            d.set_item("parent", parent.as_ref().map(id))?;
            d.set_item("merge_parent", merge_parent.as_ref().map(id))?;
        }
        Event::Merged {
            snapshot,
            into,
            ours,
            theirs,
        } => {
            d.set_item("kind", "Merged")?;
            d.set_item("snapshot", id(snapshot))?;
            d.set_item("branch", into.as_str())?;
            d.set_item("ours", id(ours))?;
            d.set_item("theirs", id(theirs))?;
        }
        Event::RefMoved {
            reference,
            from,
            to,
        } => {
            d.set_item("kind", "RefMoved")?;
            match reference {
                Ref::Branch(b) => {
                    d.set_item("ref_kind", "branch")?;
                    d.set_item("name", b.as_str())?;
                }
                Ref::Tag(t) => {
                    d.set_item("ref_kind", "tag")?;
                    d.set_item("name", t.as_str())?;
                }
                _ => d.set_item("ref_kind", "unknown")?,
            }
            d.set_item("from", from.as_ref().map(id))?;
            d.set_item("to", to.as_ref().map(id))?;
        }
        Event::Imported { snapshots } => {
            d.set_item("kind", "Imported")?;
            d.set_item("snapshots", *snapshots)?;
        }
        _ => d.set_item("kind", "Unknown")?,
    }
    Ok(d)
}
