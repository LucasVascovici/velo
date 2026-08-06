//! Progress reporting for long operations.
//!
//! Commands return data, which means nothing can be rendered until they finish.
//! For a `pull` over `ssh://` or a many-commit `rebase` that means silence, so a
//! caller can supply an [`Observer`] and be told what is happening as it happens.
//!
//! The observer is configured on the [`Repo`](crate::Repo), not passed to each
//! command, so no command signature mentions it. A repository without one reports
//! to [`Silent`], which costs a vtable call that does nothing.

use std::fmt;

/// What a long operation is currently doing.
///
/// `#[non_exhaustive]`: adding a phase is not a breaking change, and an observer
/// that doesn't recognise one can fall back on [`fmt::Display`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// Hashing and compressing working-tree files.
    Hashing,
    /// Writing files into the working tree.
    Writing,
    /// Three-way reconciling files, during a merge, cherry-pick or rebase.
    Reconciling,
    /// Replaying commits onto a new base.
    Replaying,
    /// Assembling a pack to send.
    Packing,
    /// Moving a pack over the wire.
    ///
    /// Reported without a total: the transport moves a pack in one read/write, so
    /// there is no loop to count. A consumer should show this as a spinner.
    Transferring,
    /// Verifying and inserting received objects.
    Importing,
    /// Re-hashing stored objects to check integrity.
    Verifying,
    /// Scanning the object store for objects nothing references.
    Collecting,
}

impl Phase {
    /// The unit this phase counts, for a consumer that wants to say "12/40 files".
    pub fn unit(&self) -> &'static str {
        match self {
            Phase::Hashing | Phase::Writing | Phase::Reconciling => "files",
            Phase::Replaying => "commits",
            Phase::Packing | Phase::Importing | Phase::Verifying | Phase::Collecting => "objects",
            Phase::Transferring => "bytes",
        }
    }
}

impl fmt::Display for Phase {
    /// A default label. A consumer is free to word it differently.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Phase::Hashing => "Hashing",
            Phase::Writing => "Writing",
            Phase::Reconciling => "Reconciling",
            Phase::Replaying => "Replaying",
            Phase::Packing => "Packing",
            Phase::Transferring => "Transferring",
            Phase::Importing => "Importing",
            Phase::Verifying => "Verifying",
            Phase::Collecting => "Collecting",
        })
    }
}

/// Where a long operation reports its progress.
///
/// Every method defaults to doing nothing, so an implementation overrides only
/// what it uses.
///
/// # Contract
///
/// Calls may arrive **from several threads at once** — hashing and writing files
/// are parallel — and arrive **once per item**. An implementation must therefore
/// be cheap and do its own rate limiting: core does not throttle, because how
/// often to redraw is a presentation decision.
///
/// [`advance`](Observer::advance) reports a *delta*, never a running total.
/// Deltas are race-free when several workers report at once; a cumulative count
/// would not be.
pub trait Observer: Send + Sync {
    /// A phase began. `total` is `None` when the size isn't known in advance.
    fn begin(&self, _phase: Phase, _total: Option<u64>) {}

    /// `by` more items of `phase` are done.
    fn advance(&self, _phase: Phase, _by: u64) {}

    /// The phase finished — including when the operation is failing.
    fn finish(&self, _phase: Phase) {}
}

impl<T: Observer + ?Sized> Observer for Box<T> {
    fn begin(&self, phase: Phase, total: Option<u64>) {
        (**self).begin(phase, total);
    }
    fn advance(&self, phase: Phase, by: u64) {
        (**self).advance(phase, by);
    }
    fn finish(&self, phase: Phase) {
        (**self).finish(phase);
    }
}

/// An observer that discards everything. The default for a repository.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl Observer for Silent {}

/// An open phase, which closes itself.
///
/// Obtained from `Repo::phase`, which is internal. Dropping it calls
/// [`Observer::finish`], so a phase is closed even when the operation returns
/// early through `?` — which is what makes `finish` reliable enough to drive a
/// consumer's teardown.
///
/// `Sync`, so it can be shared with rayon workers inside a parallel section.
pub struct PhaseGuard<'a> {
    observer: &'a dyn Observer,
    phase: Phase,
}

impl<'a> PhaseGuard<'a> {
    pub(crate) fn new(observer: &'a dyn Observer, phase: Phase, total: Option<u64>) -> Self {
        observer.begin(phase, total);
        PhaseGuard { observer, phase }
    }

    /// Report `by` more items done.
    pub fn advance(&self, by: u64) {
        self.observer.advance(self.phase, by);
    }

    /// Report one more item done.
    pub fn tick(&self) {
        self.advance(1);
    }
}

impl Drop for PhaseGuard<'_> {
    fn drop(&mut self) {
        self.observer.finish(self.phase);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        began: Mutex<Vec<(Phase, Option<u64>)>>,
        done: AtomicU64,
        finished: Mutex<Vec<Phase>>,
    }

    impl Observer for Recorder {
        fn begin(&self, phase: Phase, total: Option<u64>) {
            self.began.lock().unwrap().push((phase, total));
        }
        fn advance(&self, _phase: Phase, by: u64) {
            self.done.fetch_add(by, Ordering::Relaxed);
        }
        fn finish(&self, phase: Phase) {
            self.finished.lock().unwrap().push(phase);
        }
    }

    #[test]
    fn a_guard_brackets_the_phase() {
        let r = Recorder::default();
        {
            let p = PhaseGuard::new(&r, Phase::Hashing, Some(3));
            p.tick();
            p.advance(2);
        }
        assert_eq!(*r.began.lock().unwrap(), vec![(Phase::Hashing, Some(3))]);
        assert_eq!(r.done.load(Ordering::Relaxed), 3);
        assert_eq!(*r.finished.lock().unwrap(), vec![Phase::Hashing]);
    }

    #[test]
    fn the_phase_closes_even_when_the_operation_fails() {
        let r = Recorder::default();
        let attempt = |fail: bool| -> Result<(), ()> {
            let p = PhaseGuard::new(&r, Phase::Verifying, None);
            p.tick();
            if fail {
                return Err(()); // guard drops here
            }
            Ok(())
        };
        assert!(attempt(true).is_err());
        assert_eq!(
            *r.finished.lock().unwrap(),
            vec![Phase::Verifying],
            "an early return must still close the phase"
        );
    }

    #[test]
    fn a_guard_is_usable_from_parallel_workers() {
        use rayon::prelude::*;
        let r = Recorder::default();
        let p = PhaseGuard::new(&r, Phase::Writing, Some(1000));
        (0..1000u64).into_par_iter().for_each(|_| p.tick());
        drop(p);
        // Deltas rather than a running total is what makes this exact.
        assert_eq!(r.done.load(Ordering::Relaxed), 1000);
    }

    #[test]
    fn silent_discards_everything() {
        let s = Silent;
        let p = PhaseGuard::new(&s, Phase::Packing, Some(1));
        p.tick();
        // Nothing to assert beyond "doesn't panic": that is the whole contract.
    }

    #[test]
    fn every_phase_has_a_label_and_a_unit() {
        for phase in [
            Phase::Hashing,
            Phase::Writing,
            Phase::Reconciling,
            Phase::Replaying,
            Phase::Packing,
            Phase::Transferring,
            Phase::Importing,
            Phase::Verifying,
            Phase::Collecting,
        ] {
            assert!(!phase.to_string().is_empty());
            assert!(!phase.unit().is_empty());
        }
    }
}
