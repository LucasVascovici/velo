use std::collections::HashSet;
use std::fs;
use std::path::Path;

use console::style;

use crate::db;
use crate::error::Result;

/// Remove orphaned objects, stale trash entries, and stale index_cache rows.
pub fn run(root: &Path, keep_days: u32) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // ── 1. Purge old trash entries ────────────────────────────────────────────
    let trash_rows = conn.execute(
        "DELETE FROM trash WHERE deleted_at <= datetime('now', ?)",
        [format!("-{} days", keep_days)],
    )?;
    if trash_rows > 0 {
        println!(
            "  {} Removed {} trash entry/entries older than {} days.",
            style("~").yellow(),
            trash_rows,
            keep_days
        );
    }

    // ── 2. Remove orphaned file_map rows ─────────────────────────────────────
    let orphan_fm = conn.execute(
        "DELETE FROM file_map
         WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
           AND snapshot_hash NOT IN (SELECT hash FROM trash)",
        [],
    )?;
    if orphan_fm > 0 {
        println!(
            "  {} Cleaned {} orphaned file_map row(s).",
            style("~").yellow(),
            orphan_fm
        );
    }

    // ── 3. Prune stale index_cache entries ────────────────────────────────────
    // Remove entries for paths that no longer exist on disk.
    let stale_cache = conn.execute(
        "DELETE FROM index_cache
         WHERE path NOT IN (
             SELECT path FROM file_map
             WHERE snapshot_hash IN (SELECT hash FROM snapshots)
         )",
        [],
    )?;
    if stale_cache > 0 {
        println!(
            "  {} Pruned {} stale index cache entry/entries.",
            style("~").yellow(),
            stale_cache
        );
    }

    // ── 4. Collect referenced object hashes ──────────────────────────────────
    let mut stmt = conn.prepare("SELECT DISTINCT hash FROM file_map")?;
    let referenced: HashSet<String> = stmt
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    // ── 5. Delete unreferenced objects ────────────────────────────────────────
    let objects_dir = root.join(".velo/objects");
    let mut deleted_count = 0usize;
    let mut freed_bytes = 0u64;

    for entry in fs::read_dir(&objects_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !referenced.contains(&name) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            fs::remove_file(entry.path())?;
            deleted_count += 1;
            freed_bytes += size;
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    if deleted_count == 0 && trash_rows == 0 && orphan_fm == 0 && stale_cache == 0 {
        println!("{}", style("Repository is already clean. Nothing to collect.").dim());
    } else {
        println!(
            "{} GC complete — removed {} object(s), freed {:.1} KB.",
            style("✔").green().bold(),
            deleted_count,
            freed_bytes as f64 / 1024.0
        );
    }

    Ok(())
}