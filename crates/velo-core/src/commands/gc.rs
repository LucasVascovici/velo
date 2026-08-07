//! `velo gc` — reclaim space and prune bookkeeping that nothing points at.
//!
//! Returns a tally of what was collected; wording lives in `velo-cli`.

use std::collections::HashSet;
use std::fs;

use crate::error::Result;
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::WriteGuard;

/// What a collection pass reclaimed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    /// Trash entries aged past the retention window.
    pub expired_trash: usize,
    /// File-map rows for snapshots that no longer exist.
    pub orphan_file_map: usize,
    /// Hunk decisions for files no longer in conflict.
    pub orphan_decisions: usize,
    /// Shelved tags whose snapshot is gone for good.
    pub orphan_shelved_tags: usize,
    /// Index-cache rows for paths no snapshot tracks.
    pub stale_cache: usize,
    /// Objects nothing references any more.
    pub objects: usize,
    /// Bytes those objects occupied on disk.
    pub bytes_freed: u64,
    /// The retention window that was applied, in days.
    pub keep_days: u32,
}

impl Collected {
    /// Nothing needed collecting.
    pub fn is_empty(&self) -> bool {
        self.expired_trash == 0
            && self.orphan_file_map == 0
            && self.orphan_decisions == 0
            && self.orphan_shelved_tags == 0
            && self.stale_cache == 0
            && self.objects == 0
    }
}

/// How to run a collection pass.
///
/// The last of the four commands [7.6](../../../ARCHITECTURE.md) named to get
/// per-call progress. `gc` is the longest purely local operation velo has, and
/// it reported through the handle observer only — so a GUI running a collection
/// alongside anything else could not tell the two apart, and could not stop
/// either.
#[derive(Default)]
pub struct Options<'a> {
    /// Trash newer than this survives.
    pub keep_days: u32,
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between objects. Cancelling keeps what was already collected —
    /// an earlier stopping point, not a broken repository, because everything
    /// deleted was unreachable before the pass started.
    ///
    /// The database work runs first and is not interruptible: it is a handful of
    /// statements in one transaction, and splitting it would trade a bounded
    /// wait for a half-pruned index.
    pub cancel: Option<&'a Cancel>,
}

/// Collect garbage.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// # let mut repo = velo_core::Repo::discover(std::path::Path::new("."))?;
/// # let guard = repo.write()?;
/// use velo_core::commands::gc;
///
/// let collected = gc::run(&guard, gc::Options {
///     keep_days: 30,
///     ..Default::default()
/// })?;
/// # let _ = collected;
/// # Ok(()) }
/// ```
pub fn run(guard: &WriteGuard, options: Options<'_>) -> Result<Collected> {
    let Options {
        keep_days,
        observer,
        cancel,
    } = options;
    let root = guard.root();
    let conn = guard.conn();
    let mut collected = Collected {
        keep_days,
        ..Default::default()
    };

    collected.expired_trash = conn.execute(
        "DELETE FROM trash WHERE deleted_at_ms <= datetime('now', ?)",
        [format!("-{} days", keep_days)],
    )?;

    collected.orphan_file_map = conn.execute(
        "DELETE FROM file_map
         WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
           AND snapshot_hash NOT IN (SELECT hash FROM trash)",
        [],
    )?;

    collected.orphan_decisions = conn.execute(
        "DELETE FROM hunk_decisions WHERE file_path NOT IN (SELECT path FROM conflict_files)",
        [],
    )?;

    collected.orphan_shelved_tags = conn.execute(
        "DELETE FROM trash_tags
         WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
           AND snapshot_hash NOT IN (SELECT hash FROM trash)",
        [],
    )?;

    collected.stale_cache = conn.execute(
        "DELETE FROM index_cache
         WHERE path NOT IN (
             SELECT path FROM file_map
             WHERE snapshot_hash IN (SELECT hash FROM snapshots)
         )",
        [],
    )?;

    // Anything the database still points at has to survive. `file_map` is the
    // only thing that references objects, and orphaned rows were pruned above.
    let referenced: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT DISTINCT hash FROM file_map")?;
        let set = stmt
            .query_map([], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        set
    };

    // No total: the directory is streamed rather than counted first, so a
    // consumer gets liveness without us walking the whole store twice.
    let progress = PhaseGuard::new(
        observer.unwrap_or_else(|| guard.repo().observer()),
        Phase::Collecting,
        None,
    );
    for entry in fs::read_dir(root.join(".velo/objects"))? {
        let entry = entry?;
        // Checked per object, so cancelling takes effect at the next one rather
        // than part-way through a delete.
        if cancel.is_some_and(Cancel::is_cancelled) {
            break;
        }
        progress.tick();
        let name = entry.file_name().to_string_lossy().to_string();
        if referenced.contains(&name) {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        fs::remove_file(entry.path())?;
        collected.objects += 1;
        collected.bytes_freed += size;
    }

    // A cancelled pass reports `Cancelled` rather than a partial tally, as
    // `restore` does: what it did collect is durable either way, and a count
    // returned as if the pass had finished would be the misleading half.
    Cancel::check(cancel)?;

    Ok(collected)
}
