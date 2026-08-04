//! Render undo and redo outcomes for the terminal.
//!
//! Both used to name the snapshot twice — core printed a "Removing…" line and
//! returned a "removed" message for the CLI to print. With one place owning the
//! output, once is enough.

use console::style;
use velo_core::commands::{redo, undo};

pub fn print(outcome: &undo::Outcome) {
    println!(
        "{} Removed snapshot {} — \"{}\"",
        style("✔").green().bold(),
        style(&outcome.snapshot).yellow(),
        style(&outcome.message).dim()
    );
    match &outcome.now_at {
        Some(now_at) => println!("  Now at {}", style(now_at).yellow()),
        None => println!(
            "  {} That was the first snapshot, so the working tree is now empty.",
            style("!").yellow()
        ),
    }
    println!("  Bring it back with {}", style("velo redo").cyan());
}

pub fn print_redo(outcome: &redo::Outcome) {
    println!(
        "{} Restored snapshot {} — \"{}\"",
        style("✔").green().bold(),
        style(&outcome.snapshot).yellow(),
        style(&outcome.message).dim()
    );
}
