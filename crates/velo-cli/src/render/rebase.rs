//! Render rebase outcomes for the terminal.

use console::style;
use velo_core::commands::rebase::{Outcome, Replayed};

pub fn print(outcome: &Outcome) {
    match outcome {
        Outcome::AlreadyUpToDate => {
            println!("{} Already up to date.", style("✔").green().bold())
        }

        Outcome::Completed {
            branch,
            onto,
            head,
            replayed,
        } => {
            print_header(branch, onto, replayed.len());
            for step in replayed {
                print_step(step);
            }
            println!(
                "\n{} Rebase complete. HEAD is now {}.",
                style("✔").green().bold(),
                style(short(head)).yellow()
            );
        }

        Outcome::Paused {
            branch,
            onto,
            replayed,
            stopped_at,
            applied,
        } => {
            print_header(branch, onto, stopped_at.total);
            for step in replayed {
                print_step(step);
            }
            print_step(stopped_at);
            super::apply::print_files(applied);

            let conflicts = applied.conflicts();
            println!(
                "\n{} Conflict in {} file(s) while replaying {}.",
                style("!").red().bold(),
                conflicts.len(),
                style(short(&stopped_at.snapshot)).yellow()
            );
            super::apply::print_conflict_next_steps(
                &conflicts,
                &[
                    ("Then finish it:", "velo save \"…\""),
                    ("And carry on:", "velo rebase --continue"),
                    ("Or give up:", "velo rebase --abort"),
                ],
            );
        }

        Outcome::Aborted {
            restored_to,
            discarded,
        } => {
            if let Some(hash) = restored_to {
                println!(
                    "{} Rebase aborted — restored to {}{}.",
                    style("!").yellow().bold(),
                    style(short(hash)).yellow(),
                    discarded_note(*discarded)
                );
            }
            println!("{} Rebase aborted cleanly.", style("✔").green().bold());
        }
    }
}

fn print_header(branch: &str, onto: &str, total: usize) {
    println!(
        "\n{} Rebasing '{}' onto {}…\n  {} commits to replay",
        style("◆").cyan().bold(),
        style(branch).cyan(),
        style(short(onto)).yellow(),
        total
    );
}

fn print_step(step: &Replayed) {
    println!(
        "  {} {}/{} {} {}",
        style("◦").dim(),
        step.index,
        step.total,
        style(short(&step.snapshot)).yellow(),
        style(&step.message).dim()
    );
}

fn discarded_note(discarded: usize) -> String {
    if discarded == 0 {
        String::new()
    } else {
        format!(", discarding {} replayed snapshot(s)", discarded)
    }
}

/// Snapshot hashes are shown abbreviated in rebase output.
fn short(hash: &str) -> &str {
    &hash[..8.min(hash.len())]
}
