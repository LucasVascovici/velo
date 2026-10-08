//! Python binding over `velo-core`'s embedder API.
//!
//! Calls hold the GIL: `Repo` is unsendable, so `allow_threads` cannot borrow it.

use pyo3::prelude::*;

mod errors;
mod history;
mod merge;
mod repo;

#[pymodule]
fn _velo(m: &Bound<'_, PyModule>) -> PyResult<()> {
    errors::register(m)?;
    repo::register(m)?;
    Ok(())
}
