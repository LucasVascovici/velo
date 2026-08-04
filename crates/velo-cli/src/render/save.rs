//! Render save outcomes for the terminal.

use console::style;
use velo_core::commands::save::{Outcome, SaveResult};

/// `branch` is passed in rather than re-read: core already knows where the
/// snapshot landed, and the CLI shouldn't be reading repository files.
pub fn print(outcome: &Outcome, branch: &str, amended: bool) {
    match outcome {
        Outcome::NothingToSave => println!(
            "{}",
            style("Working directory clean. Nothing to save.").dim()
        ),
        Outcome::NothingToAmend => println!(
            "{}",
            style("Nothing to amend — no changes staged and no new message.").dim()
        ),
        Outcome::Saved(result) => print_saved(result, branch, amended),
    }
}

fn print_saved(result: &SaveResult, branch: &str, amended: bool) {
    println!(
        "{} {} {} on {}  ({} new, {} modified, {} deleted)",
        style("✔").green().bold(),
        if amended { "Amended" } else { "Saved" },
        style(&result.hash).yellow(),
        style(branch).cyan(),
        result.new_count,
        result.modified_count,
        result.deleted_count,
    );
}
