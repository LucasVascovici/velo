//! Render branch listings for the terminal.

use console::style;
use velo_core::commands::branches::Branch;

pub fn print_list(branches: &[Branch]) {
    println!("{}", style("Branches:").bold());
    for b in branches {
        let prefix = if b.is_current { "* " } else { "  " };
        let name = if b.is_current {
            style(&b.name).green().bold().to_string()
        } else {
            style(&b.name).white().to_string()
        };
        let meta = match &b.tip {
            Some(tip) => format!(
                "  {} {} · \"{}\"",
                style(&tip.hash[..8.min(tip.hash.len())]).yellow().dim(),
                style(short_date(&tip.created_at)).dim(),
                style(&tip.message).dim()
            ),
            None => style("  (no commits yet)").dim().to_string(),
        };
        println!("  {}{}{}", prefix, name, meta);
    }
}

pub fn print_deleted(name: &str) {
    println!(
        "{} Deleted branch '{}'.",
        style("✔").green(),
        style(name).yellow()
    );
}

/// Just the calendar date — branch listings don't need the time.
fn short_date(created_at: &str) -> &str {
    if created_at.len() >= 10 {
        &created_at[..10]
    } else {
        created_at
    }
}
