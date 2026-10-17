//! Render a garbage-collection tally for the terminal.

use console::style;
use velo_core::commands::gc::{AutoVacuum, Collected, Vacuum, Vacuumed};

pub fn print(collected: &Collected) {
    let dry = collected.dry_run;
    if collected.is_empty() {
        println!(
            "{}",
            style("Repository is already clean. Nothing to collect.").dim()
        );
        if dry {
            println!("{}", style("Nothing was removed (dry run).").dim());
        }
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
            "{} {} trash entry/entries older than {} days.",
            if dry { "Would remove" } else { "Removed" },
            collected.expired_trash,
            collected.keep_days
        ),
    );
    line(
        collected.orphan_file_map,
        format!(
            "{} {} orphaned file_map row(s).",
            if dry { "Would clean" } else { "Cleaned" },
            collected.orphan_file_map
        ),
    );
    line(
        collected.orphan_decisions,
        format!(
            "{} {} orphaned hunk decision(s).",
            if dry { "Would clean" } else { "Cleaned" },
            collected.orphan_decisions
        ),
    );
    line(
        collected.orphan_shelved_tags,
        format!(
            "{} {} shelved tag(s) with no snapshot.",
            if dry { "Would clean" } else { "Cleaned" },
            collected.orphan_shelved_tags
        ),
    );
    line(
        collected.stale_cache,
        format!(
            "{} {} stale index cache entry/entries.",
            if dry { "Would prune" } else { "Pruned" },
            collected.stale_cache
        ),
    );

    // Said only when chunks were reclaimed, so repositories without chunked
    // objects keep their familiar line.
    let chunks = if collected.chunks > 0 {
        format!(" and {} chunk(s)", collected.chunks)
    } else {
        String::new()
    };
    if dry {
        println!(
            "{} Would reclaim {} object(s){}, freeing {}.",
            style("~").yellow().bold(),
            collected.objects,
            chunks,
            human_size(collected.bytes_freed)
        );
        println!("{}", style("Nothing was removed (dry run).").dim());
        return;
    }
    println!(
        "{} GC complete — removed {} object(s){}, freed {}.",
        style("✔").green().bold(),
        collected.objects,
        chunks,
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

/// The bytes a vacuum gave back, said only when there are some. A full vacuum
/// that changed `auto_vacuum` says so, since the repository now behaves
/// differently from here on.
pub fn print_vacuum(v: &Vacuumed) {
    if v.mode == Vacuum::Full && v.auto_vacuum == AutoVacuum::Incremental {
        println!(
            "{}",
            style("Database set to incremental vacuum (converted if it was not already).").dim()
        );
    }
    let returned = v.bytes_before.saturating_sub(v.bytes_after);
    if returned > 0 {
        println!(
            "{} Returned {} to the operating system.",
            style("✔").green().bold(),
            human_size(returned)
        );
    }
}
