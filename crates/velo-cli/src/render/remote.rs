//! Render remote listings and outcomes for the terminal.

use console::style;
use velo_core::commands::remote::{Added, Remote};

/// Minimum width of the remote-name column.
const NAME_MIN: usize = 16;

pub fn print_list(remotes: &[Remote]) {
    if remotes.is_empty() {
        println!(
            "{}",
            style("No remotes configured. Add one with 'velo remote add <name> <path>'.").dim()
        );
        return;
    }
    // Widen for long remote names so the URL column stays put.
    let name_w = remotes
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(NAME_MIN);
    println!(
        "{:<name_w$} {}",
        style("Remote").bold(),
        style("URL").bold()
    );
    for r in remotes {
        println!("{:<name_w$} {}", style(&r.name).cyan(), r.url);
    }
}

pub fn print_added(added: &Added) {
    println!(
        "{} Added remote '{}' → {}",
        style("✔").green().bold(),
        added.name,
        added.url
    );
    if added.unreachable {
        println!(
            "  {} '{}' is not a Velo repository yet.",
            style("note:").yellow(),
            added.url
        );
    }
}

pub fn print_removed(name: &str) {
    println!("{} Removed remote '{}'.", style("✔").green().bold(), name);
}
