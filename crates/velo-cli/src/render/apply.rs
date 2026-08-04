//! Render three-way reconciliation — shared by merge, cherry-pick and rebase.
//!
//! All three do the same work, so they report it the same way. They used to
//! differ gratuitously: cherry-pick listed only conflicts while merge listed
//! every file, and cherry-pick's summary said "Changed" where merge's said
//! "Updated" for the identical count.

use console::style;
use velo_core::commands::apply::{Applied, FileAction};

/// One line per changed file, in path order.
pub fn print_files(applied: &Applied) {
    for file in &applied.files {
        match file.action {
            FileAction::Deleted => println!("  {} Deleted: {}", style("-").red(), file.path),
            FileAction::Added => println!("  {} New file: {}", style("+").green(), file.path),
            FileAction::Updated => println!("  {} Updated:  {}", style("~").cyan(), file.path),
            FileAction::AutoMerged => {
                println!("  {} Auto-merged: {}", style("~").cyan(), file.path)
            }
            FileAction::KeptOurs => println!(
                "  {} Delete/modify conflict: '{}' (keeping ours)",
                style("!").yellow().bold(),
                file.path
            ),
            FileAction::Conflicted => {
                println!("  {} Conflict: {}", style("!").yellow().bold(), file.path)
            }
        }
    }
}

/// The counts, under `<title> summary`.
pub fn print_summary(title: &str, applied: &Applied) {
    println!(
        "\n{}",
        style(format!("{} summary", title)).bold().underlined()
    );
    println!("  New:      {}", applied.added());
    println!("  Updated:  {}", applied.updated());
    println!("  Deleted:  {}", applied.deleted());
    println!("  Conflicts: {}", applied.conflicts().len());
}

/// Per-file resolution guidance, then how to finish the operation.
///
/// `finish` is the command that completes this particular operation — a merge
/// and a paused rebase are finished differently.
pub fn print_conflict_next_steps(conflicts: &[&str], finish: &[(&str, &str)]) {
    println!("\n{}", style("Action required:").red().bold());
    for path in conflicts {
        println!("  [{}]", style(path).yellow());
        println!(
            "    Resolve interactively: {}",
            style(format!("velo resolve {}", path)).cyan()
        );
        println!(
            "    Quick-take:            {}  or  {}",
            style(format!("velo resolve {} --take theirs", path)).green(),
            style(format!("velo resolve {} --take ours", path)).dim()
        );
    }
    println!(
        "\nResolve all at once:  {}",
        style("velo resolve --all --take theirs").cyan()
    );
    let width = finish
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    for (label, command) in finish {
        println!(
            "{:<width$}  {}",
            label,
            style(command).yellow().bold(),
            width = width
        );
    }
}
