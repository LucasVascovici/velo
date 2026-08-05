//! `velo gc` — reclaim space and prune bookkeeping that nothing points at.
//!
//! Returns a tally of what was collected; wording lives in `velo-cli`.

use std::collections::HashSet;
use std::fs;

use crate::error::Result;
use crate::progress::Phase;
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

/// Collect garbage, keeping trash newer than `keep_days`.
pub fn run(guard: &WriteGuard, keep_days: u32) -> Result<Collected> {
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
    let progress = guard.phase(Phase::Collecting, None);
    for entry in fs::read_dir(root.join(".velo/objects"))? {
        let entry = entry?;
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

    Ok(collected)
}
