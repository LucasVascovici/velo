//! Named crash points, for proving that an interrupted operation leaves either
//! the old state or the new one.
//!
//! Each durable step of a command (a transaction commit, a write of
//! `.velo/PARENT` or `.velo/HEAD`, the first file of a multi-file working-tree
//! write) is preceded or followed by a call to [`hit`] with a name of the form
//! `"<op>.<stage>"`. With the `failpoints` feature off, `hit` is an empty inline
//! function and the calls compile to nothing.
//!
//! With the feature on, a handler installed by the test with [`set_handler`] is
//! called with each name. Core itself never aborts or exits: the handler, which
//! lives in the test, decides what to do (the crash harness aborts the process).
//!
//! The handler is a process-wide global. That does not breach the rule against
//! globals for progress or cancellation: it exists only in test builds, is
//! compiled out of every real build, and carries no per-call state.

#[cfg(feature = "failpoints")]
mod imp {
    use std::sync::OnceLock;

    static HANDLER: OnceLock<fn(&'static str)> = OnceLock::new();

    /// Install the handler called at every named point. Only the first call
    /// takes effect.
    pub fn set_handler(f: fn(&'static str)) {
        let _ = HANDLER.set(f);
    }

    /// Report that the named point was reached.
    pub(crate) fn hit(name: &'static str) {
        if let Some(f) = HANDLER.get() {
            f(name);
        }
    }
}

#[cfg(feature = "failpoints")]
pub(crate) use imp::hit;
#[cfg(feature = "failpoints")]
pub use imp::set_handler;

/// Report that the named point was reached. Compiled out without `failpoints`.
#[cfg(not(feature = "failpoints"))]
#[inline(always)]
pub(crate) fn hit(_: &'static str) {}
