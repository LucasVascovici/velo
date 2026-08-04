//! Render cherry-pick outcomes for the terminal.

use console::style;
use velo_core::commands::cherry_pick::Outcome;

pub fn print(outcome: &Outcome) {
    println!(
        "Cherry-picking {} — \"{}\"",
        style(&outcome.snapshot).yellow(),
        style(&outcome.message).dim()
    );

    super::apply::print_files(&outcome.applied);
    super::apply::print_summary("Cherry-pick", &outcome.applied);

    let conflicts = outcome.applied.conflicts();
    if !conflicts.is_empty() {
        super::apply::print_conflict_next_steps(
            &conflicts,
            &[
                ("Once resolved:", "velo save \"Apply cherry-pick\""),
                ("To cancel:", "velo merge --abort"),
            ],
        );
        return;
    }

    match &outcome.saved_as {
        Some(hash) => println!(
            "
{} Cherry-pick applied as snapshot {}",
            style("✔").green().bold(),
            style(hash).yellow()
        ),
        None => println!(
            "
{} Nothing to apply — already up to date.",
            style("✔").green()
        ),
    }
}
