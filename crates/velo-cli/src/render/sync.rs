//! Render sync outcomes for the terminal.

use console::style;
use velo_core::commands::sync::{BranchCreated, Cloned, Fetched, Pulled, Pushed};

pub fn print_cloned(cloned: &Cloned) {
    println!(
        "{} Cloned {} snapshot(s), {} object(s), {} branch(es) into {}",
        style("✔").green().bold(),
        cloned.snapshots,
        cloned.objects,
        cloned.branches,
        style(cloned.into.display()).cyan()
    );
    println!(
        "  Checked out {} — from {}",
        style(&cloned.branch).cyan().bold(),
        style(&cloned.url).dim()
    );
}

pub fn print_fetched(fetched: &Fetched) {
    println!(
        "{} Fetched from '{}' — {} new snapshot(s), {} object(s).",
        style("✔").green().bold(),
        fetched.remote,
        fetched.snapshots,
        fetched.objects
    );
    for r in &fetched.refs {
        println!(
            "  {}/{}  →  {}",
            fetched.remote,
            style(&r.branch).cyan(),
            style(super::id::short(&r.hash)).yellow()
        );
    }
    // Fetch deliberately leaves local branches and the working tree alone, which
    // surprises people expecting `pull`.
    if fetched.snapshots > 0 {
        println!(
            "  {} your branches are untouched — {} to incorporate the changes.",
            style("note:").dim(),
            style(format!("velo pull {}", fetched.remote)).cyan()
        );
    }
}

pub fn print_pushed(pushed: &Pushed) {
    match pushed {
        Pushed::AlreadyUpToDate { branch, remote } => println!(
            "{} '{}' is already up to date on '{}'.",
            style("✔").green(),
            branch,
            remote
        ),
        Pushed::Sent {
            branch,
            remote,
            snapshots,
            objects,
            created,
        } => {
            println!(
                "{} Pushed '{}' to '{}' — {} snapshot(s), {} object(s).",
                style("✔").green().bold(),
                branch,
                remote,
                snapshots,
                objects
            );
            if let Some(created) = created {
                let what = match created {
                    BranchCreated::RemoteWasEmpty => "was empty; it now has its first history",
                    BranchCreated::BranchWasMissing => {
                        "did not have this branch; it was created there"
                    }
                };
                println!(
                    "  {} '{}' {}. Others can now {}.",
                    style("note:").dim(),
                    remote,
                    what,
                    style(format!("velo clone <url> / velo pull {}", remote)).cyan()
                );
            }
            println!(
                "  {} the remote's working tree is unchanged; it updates on its next {}.",
                style("note:").dim(),
                style("velo pull").cyan()
            );
        }
    }
}

pub fn print_pulled(pulled: &Pulled) {
    match pulled {
        Pulled::AlreadyUpToDate { branch, remote } => println!(
            "{} '{}' is already up to date with '{}'.",
            style("✔").green(),
            branch,
            remote
        ),
        Pulled::FastForwarded { branch, to, .. } => println!(
            "{} Fast-forwarded '{}' to {}.",
            style("✔").green().bold(),
            branch,
            style(super::id::short(to)).yellow()
        ),
        Pulled::Diverged { branch, remote } => {
            println!(
                "{} '{}' and '{}/{}' have diverged.",
                style("!").yellow().bold(),
                branch,
                remote,
                branch
            );
            println!(
                "  Reconcile with {} then {}",
                style(format!("velo merge {}/{}", remote, branch)).cyan(),
                style("velo save \"Merge …\"").cyan()
            );
        }
    }
}
