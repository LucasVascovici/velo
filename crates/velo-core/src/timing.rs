//! Named stage timers for profiling save, save_tree, restore, compaction and
//! retention.
//!
//! Each instrumented function calls [`begin`] at its entry and [`lap`] after
//! each stage. With the `timing` feature off, `begin`, `lap` and `count` are
//! empty inline functions and the calls compile to nothing, so the normal path
//! pays nothing.
//!
//! With the feature on, laps are recorded in a thread-local list that the
//! caller reads with [`drain_laps`]; `lap` is only ever called on the calling
//! thread, never inside a rayon closure. Counters ([`count`]) are global
//! because filesystem work runs on rayon workers.
//!
//! These are process-wide or thread-wide globals. That does not breach the
//! rule against globals for progress or cancellation: they exist only in
//! profiling builds, are compiled out of every real build, and never influence
//! behaviour (the same justification as `failpoint`).

#[cfg(feature = "timing")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// One named stage and the time since the previous mark.
    #[derive(Clone, Copy, Debug)]
    pub struct Lap {
        pub name: &'static str,
        pub elapsed: Duration,
    }

    thread_local! {
        static MARK: Cell<Option<Instant>> = const { Cell::new(None) };
        static LAPS: RefCell<Vec<Lap>> = const { RefCell::new(Vec::new()) };
    }

    static COUNTS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

    pub(crate) fn begin() {
        MARK.with(|m| m.set(Some(Instant::now())));
    }

    pub(crate) fn lap(name: &'static str) {
        let now = Instant::now();
        let elapsed = MARK.with(|m| {
            let e = m.get().map_or(Duration::ZERO, |t| now - t);
            m.set(Some(now));
            e
        });
        LAPS.with(|l| l.borrow_mut().push(Lap { name, elapsed }));
    }

    #[allow(dead_code)]
    pub(crate) fn count(name: &'static str, n: u64) {
        let mut g = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
        *g.entry(name).or_insert(0) += n;
    }

    /// Return and clear the calling thread's laps.
    pub fn drain_laps() -> Vec<Lap> {
        LAPS.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    /// Return and clear the global counters.
    pub fn take_counts() -> Vec<(&'static str, u64)> {
        let mut g = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *g).into_iter().collect()
    }
}

#[cfg(feature = "timing")]
#[allow(unused_imports)]
pub(crate) use imp::{begin, count, lap};
#[cfg(feature = "timing")]
pub use imp::{drain_laps, take_counts, Lap};

#[cfg(not(feature = "timing"))]
#[inline(always)]
pub(crate) fn begin() {}

#[cfg(not(feature = "timing"))]
#[inline(always)]
pub(crate) fn lap(_name: &'static str) {}

#[cfg(not(feature = "timing"))]
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn count(_name: &'static str, _n: u64) {}
