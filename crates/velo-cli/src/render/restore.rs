//! Render restore outcomes for the terminal.

use console::style;
use velo_core::commands::restore::Outcome;

pub fn print(outcome: &Outcome) {
    match outcome {
        Outcome::AlreadyThere { snapshot } => println!(
            "{} Already at snapshot {}. Nothing to do.",
            style("✔").green(),
            style(super::id::short(snapshot)).yellow()
        ),

        Outcome::NoMatchingPaths { snapshot } => println!(
            "{} No matching files found in snapshot '{}' for the given paths.",
            style("!").yellow(),
            snapshot
        ),

        Outcome::Restored {
            snapshot,
            branch,
            message,
            ghosts_removed,
            discarded,
            ..
        } => {
            print_discarded(*discarded);
            if *ghosts_removed > 0 {
                println!(
                    "  {} Removed {} ghost file(s).",
                    style("~").yellow(),
                    ghosts_removed
                );
            }
            println!(
                "{} Restored to {} on {} — \"{}\"",
                style("✔").green().bold(),
                style(super::id::short(snapshot)).yellow(),
                style(branch).cyan(),
                style(message).white()
            );
        }

        Outcome::RestoredPaths {
            snapshot,
            files,
            discarded,
        } => {
            print_discarded(*discarded);
            println!(
                "{} Restored {} file(s) from {} to working tree.",
                style("✔").green().bold(),
                files,
                style(super::id::short(snapshot)).yellow()
            );
        }
    }
}

fn print_discarded(count: usize) {
    if count > 0 {
        println!(
            "{} Discarding {} unsaved change(s).",
            style("!").yellow().bold(),
            count
        );
    }
}
