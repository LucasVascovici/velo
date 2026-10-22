//! Render [`Status`] for the terminal.

use console::style;
use velo_core::commands::status::{Status, Tracking};

pub fn print(status: &Status) {
    print_header(status);
    // Mid-merge the next step is finishing the merge; sync advice would only
    // send the user down a second, competing path.
    match &status.merging {
        Some(source) => print_merge_step(source, &status.conflicts),
        None => {
            print_tracking(&status.tracking);
            print_conflicts(&status.conflicts);
        }
    }

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
        Tracking::Known {
            remote,
            branch,
            ahead,
            behind,
        } => match (*ahead, *behind) {
            (0, 0) => println!(
                "  {} up to date with {} (as of last fetch)",
                style("✔").green(),
                style(&label).cyan()
            ),
            (a, 0) => println!(
                "  {} {} ahead of {} (as of last fetch) — {} to publish",
                style("↑").green().bold(),
                a,
                style(&label).cyan(),
                style("velo push").cyan()
            ),
            (0, b) => println!(
                "  {} {} behind {} (as of last fetch) — {} to catch up",
                style("↓").yellow().bold(),
                b,
                style(&label).cyan(),
                style("velo sync").cyan()
            ),
            (a, b) => {
                println!(
                    "  {} diverged from {} ({} ahead, {} behind, as of last fetch) — reconcile with:",
                    style("↕").yellow().bold(),
                    style(&label).cyan(),
                    a,
                    b
                );
                super::sync::print_recipe(remote, branch, super::sync::RecipeState::Fetched);
            }
        },
    }
}

/// The merge's next step, in place of any sync advice.
fn print_merge_step(source: &str, conflicts: &[String]) {
    for c in conflicts {
        println!("  {} {}", style("[Conflict]").red().bold(), c);
    }
    if conflicts.is_empty() {
        println!(
            "  Merge of {} ready — {} to finish it ({} to cancel)",
            style(source).cyan(),
            style("velo save").green(),
            style("velo merge --abort").cyan()
        );
    } else {
        println!(
            "  Merging {}: resolve {} file(s) with {} ({} shows both sides), then {} to finish merging {}",
            style(source).cyan(),
            conflicts.len(),
            style("velo resolve <file>").cyan(),
            style("velo conflicts <file>").cyan(),
            style("velo save").green(),
            style(source).cyan()
        );
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
