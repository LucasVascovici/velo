//! Python binding over `velody-core`'s embedder API.
//!
//! Calls hold the GIL except `merge_plan` and `merge_commit`, which release it so a
//! Python merge driver can run on any thread. `Repo` is unsendable, so the others
//! keep it.

use pyo3::prelude::*;

mod callbacks;
mod errors;
mod history;
mod merge;
mod repo;
mod signing;
mod store;

#[pymodule]
fn _velody(m: &Bound<'_, PyModule>) -> PyResult<()> {
    errors::register(m)?;
    repo::register(m)?;
    store::register(m)?;
    signing::register(m)?;
    Ok(())
}
