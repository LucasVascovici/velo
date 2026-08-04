//! Render a garbage-collection tally for the terminal.

use console::style;
use velo_core::commands::gc::Collected;

pub fn print(collected: &Collected) {
    if collected.is_empty() {
        println!(
            "{}",
            style("Repository is already clean. Nothing to collect.").dim()
        );
        return;
    }

    let line = |count: usize, what: String| {
        if count > 0 {
            println!("  {} {}", style("~").yellow(), what);
        }
    };
    line(
        collected.expired_trash,
        format!(
            "Removed {} trash entry/entries older than {} days.",
            collected.expired_trash, collected.keep_days
        ),
    );
    line(
        collected.orphan_file_map,
        format!(
            "Cleaned {} orphaned file_map row(s).",
            collected.orphan_file_map
        ),
    );
    line(
        collected.orphan_decisions,
        format!(
            "Cleaned {} orphaned hunk decision(s).",
            collected.orphan_decisions
        ),
    );
    line(
        collected.orphan_shelved_tags,
        format!(
            "Cleaned {} shelved tag(s) with no snapshot.",
            collected.orphan_shelved_tags
        ),
    );
    line(
        collected.stale_cache,
        format!(
            "Pruned {} stale index cache entry/entries.",
            collected.stale_cache
        ),
    );

    println!(
        "{} GC complete — removed {} object(s), freed {}.",
        style("✔").green().bold(),
        collected.objects,
        human_size(collected.bytes_freed)
    );
}

/// Bytes as something a person can read. The old output was always in KB, so a
/// 40 MB collection read as "40960.0 KB". Shared with the bundle renderer.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}
