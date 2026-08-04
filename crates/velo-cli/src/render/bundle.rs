//! Render bundle outcomes for the terminal.

use console::style;
use velo_core::commands::bundle::{Applied, Created};

pub fn print_created(created: &Created) {
    println!(
        "{} Bundled {} snapshot(s), {} object(s), {} tag(s) → {} ({})",
        style("✔").green().bold(),
        created.snapshots,
        created.objects,
        created.tags,
        style(&created.file).cyan(),
        super::gc::human_size(created.bytes)
    );
}

pub fn print_applied(applied: &Applied) {
    if applied.is_empty() {
        println!(
            "{} Already up to date — nothing new in this bundle.",
            style("✔").green()
        );
        return;
    }
    println!(
        "{} Imported {} snapshot(s) and {} object(s).",
        style("✔").green().bold(),
        applied.snapshots,
        applied.objects
    );
    println!(
        "  Use {} to see them, or {} to check into one.",
        style("velo history --all").cyan(),
        style("velo switch <branch>").cyan()
    );
}
