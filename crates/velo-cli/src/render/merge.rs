//! Render merge outcomes for the terminal.

use console::style;
use velo_core::commands::merge::{Outcome, ThreeWay};

pub fn print(outcome: &Outcome) {
    match outcome {
        Outcome::Aborted {
            source,
            restored_to,
        } => print_aborted(source, restored_to.as_deref()),

        Outcome::StartedUnbornBranch { branch, at } => println!(
            "{} Fast-forwarded '{}' to {} — the branch had no commits of its own.",
            style("✔").green().bold(),
            branch,
            style(super::id::short(at)).yellow()
        ),

        Outcome::AlreadyUpToDate { branch, other } => println!(
            "{} Already up to date — '{}' and '{}' point at the same snapshot.",
            style("✔").green(),
            branch,
            other
        ),

        Outcome::FastForwarded { branch, to } => {
            println!(
                "{} Fast-forwarding '{}' to {}…",
                style(">>").green().bold(),
                branch,
                style(to).yellow()
            );
            println!("{} Fast-forward complete.", style("✔").green());
        }

        Outcome::Merged(result) => print_three_way(result),
    }
}

fn print_aborted(source: &str, restored_to: Option<&str>) {
    match restored_to {
        Some(hash) => println!(
            "{} Aborting merge of '{}' — restoring to {}…",
            style("!").yellow().bold(),
            style(source).cyan(),
            style(super::id::short(hash)).yellow()
        ),
        None => println!(
            "{} Merge aborted (no pre-merge snapshot recorded).",
            style("✔").green()
        ),
    }
    println!("{} Merge aborted cleanly.", style("✔").green().bold());
}

fn print_three_way(result: &ThreeWay) {
    println!(
        "Merging '{}' into '{}' (ancestor: {})…",
        style(&result.source).yellow().bold(),
        style(&result.into).cyan().bold(),
        style(result.ancestor.as_deref().unwrap_or("none")).dim()
    );

    super::apply::print_files(&result.applied);
    super::apply::print_summary("Merge", &result.applied);

    let conflicts = result.conflicts();
    if !conflicts.is_empty() {
        super::apply::print_conflict_next_steps(
            &conflicts,
            &[("Once resolved:", "velo save \"Merge <branch>\"")],
        );
    } else if result.applied_nothing() {
        println!(
            "
{} Already up to date — nothing to merge.",
            style("✔").green()
        );
    } else {
        println!(
            "
{} Clean merge! Run {} to finalise.",
            style("✔").green(),
            style("velo save \"Merge <branch>\"").yellow().bold()
        );
    }
}
