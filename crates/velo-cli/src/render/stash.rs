//! Render stash outcomes for the terminal.

use console::style;
use velo_core::commands::stash::{Popped, Pushed, Shelf, ShelfDetail};

pub fn print_pushed(pushed: &Pushed) {
    match pushed {
        Pushed::NothingToStash => println!(
            "{}",
            style("Working directory clean — nothing to stash.").dim()
        ),
        Pushed::Shelved {
            name,
            modified,
            new,
            deleted,
            restored_to,
        } => {
            println!(
                "{} Shelved '{}' ({} modified, {} new, {} deleted)",
                style("✔").green().bold(),
                style(name).cyan(),
                modified,
                new,
                deleted
            );
            match restored_to {
                Some(hash) => {
                    println!("  Working tree restored to {}", style(hash).yellow())
                }
                None => println!(
                    "  {}",
                    style("Working tree emptied — the branch had no snapshots.").dim()
                ),
            }
            println!("  Bring it back with {}", style("velo stash pop").cyan());
        }
    }
}

pub fn print_list(shelves: &[Shelf]) {
    if shelves.is_empty() {
        println!("{}", style("No shelves found.").dim());
        return;
    }
    println!("{}", style("Stash shelves:").bold());
    for s in shelves {
        println!(
            "  {} {} {} {}",
            style(&s.name).cyan().bold(),
            style(format!("(on {})", s.branch)).dim(),
            style(super::when::minutes(s.created_at)).dim(),
            style(super::id::short(&s.snapshot)).yellow().dim()
        );
    }
}

pub fn print_popped(popped: &Popped) {
    // Neither of these is a reason to refuse, but both change what the user is
    // about to see, so they come before the result.
    if let Some(mismatch) = &popped.branch_mismatch {
        println!(
            "{} Note: shelf was created on branch '{}', you are on '{}'.",
            style("!").yellow(),
            style(&mismatch.shelf).cyan(),
            style(&mismatch.current).cyan()
        );
    }
    if popped.position_moved {
        println!(
            "{} Note: the working tree has moved since this shelf was created.",
            style("!").yellow()
        );
    }

    print!(
        "{} Applied shelf '{}' — {} file(s) restored",
        style("✔").green().bold(),
        style(&popped.name).cyan(),
        popped.restored
    );
    if popped.removed > 0 {
        print!(", {} removed", popped.removed);
    }
    println!();
}

pub fn print_dropped(name: &str) {
    println!(
        "{} Dropped shelf '{}'.",
        style("✔").green(),
        style(name).cyan()
    );
}

pub fn print_shelf(shelf: &ShelfDetail) {
    println!(
        "Shelf: {}  (from {} @ {})",
        style(&shelf.name).cyan().bold(),
        style(&shelf.branch).dim(),
        style(&shelf.parent[..8.min(shelf.parent.len())]).yellow()
    );
    super::diff::print_change_list(&shelf.diff);
}
