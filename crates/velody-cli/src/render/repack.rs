//! Render a repack for the terminal.

use console::style;
use velody_core::commands::repack::Repacked;

use super::gc::human_size;

pub fn print(r: &Repacked) {
    if r.snapshots_converted == 0 && r.chains_shortened == 0 {
        println!("{}", style("Nothing to repack.").dim());
        return;
    }
    let verb = if r.dry_run {
        "would convert"
    } else {
        "Converted"
    };
    println!(
        "{} {} {} snapshot(s) to deltas, {} chain(s) shortened, {} keyframe(s) kept.",
        style("◆").cyan().bold(),
        verb,
        r.snapshots_converted,
        r.chains_shortened,
        r.keyframes_kept
    );
    println!("  Rows:  {} -> {}", r.rows_before, r.rows_after);
    println!(
        "  Bytes: {} -> {}{}",
        human_size(r.bytes_before),
        human_size(r.bytes_after),
        if r.dry_run { " (estimated)" } else { "" }
    );
    let saved = r.bytes_before.saturating_sub(r.bytes_after);
    if r.dry_run {
        println!(
            "{} would save {} bytes (estimated). Nothing was changed (dry run).",
            style("~").yellow().bold(),
            saved
        );
    } else {
        println!("{} Saved {} bytes.", style("✔").green().bold(), saved);
    }
}
