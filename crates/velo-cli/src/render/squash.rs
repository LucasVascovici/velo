//! Render squash outcomes for the terminal.

use console::style;
use velo_core::commands::squash::Outcome;

pub fn print(outcome: &Outcome) {
    println!(
        "\n{} Squashed {} snapshots on '{}'…",
        style("◆").cyan().bold(),
        outcome.replaced.len(),
        style(&outcome.branch).cyan()
    );
    for (i, item) in outcome.replaced.iter().enumerate() {
        let marker = if i == 0 { "HEAD" } else { "    " };
        println!(
            "  {} {} {}",
            style(marker).dim(),
            style(short(&item.snapshot)).yellow(),
            style(&item.message).dim()
        );
    }
    println!("  {} → new snapshot", style("─".repeat(40)).dim());
    println!(
        "{} Squashed into {} — \"{}\"",
        style("✔").green().bold(),
        style(&outcome.snapshot).yellow(),
        outcome.message
    );
}

fn short(hash: &str) -> &str {
    &hash[..8.min(hash.len())]
}
