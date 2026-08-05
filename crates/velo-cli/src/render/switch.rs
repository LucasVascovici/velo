//! Render switch outcomes for the terminal.

use console::style;
use velo_core::commands::switch::Outcome;

pub fn print(outcome: &Outcome) {
    match outcome {
        Outcome::AlreadyOn { branch } => {
            println!("Already on branch '{}'.", style(branch).cyan().bold())
        }

        Outcome::Switched { branch, at } => println!(
            "Switched to branch '{}' at snapshot {}",
            style(branch).cyan().bold(),
            style(super::id::short(at)).yellow()
        ),

        Outcome::StartedUnborn {
            branch,
            existing,
            inherits,
        } => {
            let verb = if *existing {
                "Switched to"
            } else {
                "Created and switched to"
            };
            match inherits {
                Some(from) => println!(
                    "{} {} branch '{}' — no commits yet; your first {} will start it from {}.",
                    style("✨").bold(),
                    verb,
                    style(branch).cyan().bold(),
                    style("velo save").cyan(),
                    style(super::id::short(from)).yellow()
                ),
                None => println!(
                    "{} {} branch '{}' — no commits yet, so your first {} starts its history.",
                    style("✨").bold(),
                    verb,
                    style(branch).cyan().bold(),
                    style("velo save").cyan()
                ),
            }
        }
    }
}
