//! Render tag listings and outcomes for the terminal.

use console::style;
use velo_core::commands::tag::{Created, Tag};

/// Minimum width of the tag-name column, wide enough for the `Tag` header.
const NAME_MIN: usize = 20;
/// Width of the ` | ` column separator.
const SEP: usize = 3;

pub fn print_list(tags: &[Tag]) {
    if tags.is_empty() {
        println!("{}", style("No tags defined.").dim());
        return;
    }

    // Derive both widths from the data instead of hard-coding them. A fixed
    // 14-wide snapshot column was narrower than a 16-char hash, so every data row
    // pushed the message two places right of its own header.
    let name_w = tags
        .iter()
        .map(|t| t.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(NAME_MIN);
    let hash_w = tags
        .iter()
        .map(|t| super::id::short(&t.snapshot).chars().count())
        .max()
        .unwrap_or(0)
        .max(velo_core::commands::SNAP_HASH_LEN);

    println!("{:<name_w$} | {:<hash_w$} | Message", "Tag", "Snapshot");
    println!(
        "{}",
        "-".repeat(name_w + SEP + hash_w + SEP + "Message".len())
    );

    for t in tags {
        // A tag can outlive its snapshot: `undo` shelves the snapshot but leaves
        // the tag pointing at it.
        let message = t.message.as_deref().unwrap_or("(deleted)");
        println!(
            "{:<name_w$} | {:<hash_w$} | {}",
            style(&t.name).cyan(),
            style(super::id::short(&t.snapshot)).yellow(),
            style(message).dim()
        );
    }
}

pub fn print_created(created: &Created) {
    if let Some(prev) = &created.replaced {
        println!(
            "{} Overwriting tag '{}' (was → {}).",
            style("!").yellow(),
            style(&created.name).yellow(),
            style(prev).dim()
        );
    }
    println!(
        "{} Tagged {} as '{}'.",
        style("✔").green(),
        style(super::id::short(&created.snapshot)).yellow(),
        style(&created.name).cyan()
    );
}

pub fn print_deleted(name: &str) {
    println!(
        "{} Deleted tag '{}'.",
        style("✔").green(),
        style(name).yellow()
    );
}
