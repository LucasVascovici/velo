//! Render [`Status`] for the terminal.

use console::style;
use velo_core::commands::status::{Status, Tracking};

pub fn print(status: &Status) {
    print_header(status);
    print_tracking(&status.tracking);
    print_conflicts(&status.conflicts);

    if status.is_clean() {
        println!("  {}", style("Working directory clean.").dim());
        return;
    }

    print_group("New", &status.new_files, Colour::Green);
    print_group("Modified", &status.modified, Colour::Yellow);
    print_group("Deleted", &status.deleted, Colour::Red);

    if status.change_count() > 0 {
        println!(
            "\n  {} change(s) total — use {} or {}",
            status.change_count(),
            style("velo diff").cyan(),
            style("velo save \"<message>\"").green()
        );
    }
}

fn print_header(status: &Status) {
    let position = match &status.position {
        Some(hash) => style(super::id::short(hash)).yellow().to_string(),
        None => style("no snapshots yet").dim().to_string(),
    };
    print!(
        "Branch: {}  Position: {}",
        style(&status.branch).cyan().bold(),
        position
    );
    if let Some(msg) = &status.position_message {
        print!("  \"{}\"", style(msg).dim());
    }
    println!();
}

/// Silent when the branch isn't tracked, so users who never sync see nothing new.
fn print_tracking(tracking: &Tracking) {
    let label = match tracking.label() {
        Some(l) => l,
        None => return,
    };
    match tracking {
        Tracking::Untracked => {}
        Tracking::Unfetched { .. } => println!(
            "  {} tracking {} — run {} to compare.",
            style("↕").dim(),
            style(&label).cyan(),
            style("velo fetch").cyan()
        ),
        Tracking::Known { ahead, behind, .. } => match (*ahead, *behind) {
            (0, 0) => println!(
                "  {} up to date with {}",
                style("✔").green(),
                style(&label).cyan()
            ),
            (a, 0) => println!(
                "  {} {} ahead of {} — {} to publish",
                style("↑").green().bold(),
                a,
                style(&label).cyan(),
                style("velo push").cyan()
            ),
            (0, b) => println!(
                "  {} {} behind {} — {} to catch up",
                style("↓").yellow().bold(),
                b,
                style(&label).cyan(),
                style("velo pull").cyan()
            ),
            (a, b) => println!(
                "  {} diverged from {} ({} ahead, {} behind) — {} then {}",
                style("↕").yellow().bold(),
                style(&label).cyan(),
                a,
                b,
                style("velo pull").cyan(),
                style(format!("velo merge {}", label)).cyan()
            ),
        },
    }
}

fn print_conflicts(conflicts: &[String]) {
    if conflicts.is_empty() {
        return;
    }
    println!(
        "\n{} Merge in progress — {} conflict(s) unresolved.",
        style("!").yellow().bold(),
        conflicts.len()
    );
    for c in conflicts {
        println!("  {} {}", style("[Conflict]").red().bold(), c);
    }
    println!(
        "  Run {} or {} to resolve, then {}",
        style("velo resolve <file>").cyan(),
        style("velo resolve --all --take ours|theirs").cyan(),
        style("velo save \"Finish merge\"").green()
    );
    println!();
}

enum Colour {
    Green,
    Yellow,
    Red,
}

fn print_group(label: &str, files: &[String], colour: Colour) {
    if files.is_empty() {
        return;
    }
    let heading = match colour {
        Colour::Green => style(label).green().bold(),
        Colour::Yellow => style(label).yellow().bold(),
        Colour::Red => style(label).red().bold(),
    };
    println!("\n  {} {} file(s):", heading, files.len());
    for f in files {
        let line = match colour {
            Colour::Green => style(f).green(),
            Colour::Yellow => style(f).yellow(),
            Colour::Red => style(f).red(),
        };
        println!("    {}", line);
    }
}
