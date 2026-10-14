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
    let mut parts = Vec::new();
    if applied.snapshots > 0 {
        parts.push(format!("{} snapshot(s)", applied.snapshots));
    }
    if applied.objects > 0 {
        parts.push(format!("{} object(s)", applied.objects));
    }
    if applied.tags > 0 {
        parts.push(format!("{} tag(s)", applied.tags));
    }
    let list = match parts.as_slice() {
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {}", init.join(", "), last),
        [] => String::new(),
    };
    println!("{} Imported {}.", style("✔").green().bold(), list);
    println!(
        "  Use {} to see them, or {} to check into one.",
        style("velo history --all").cyan(),
        style("velo switch <branch>").cyan()
    );
}
