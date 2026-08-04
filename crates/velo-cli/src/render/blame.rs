//! Render [`Blame`] for the terminal.

use console::style;
use velo_core::commands::blame::Blame;

/// Width of the message column, matching the `{:<28}` layout below.
const MSG_WIDTH: usize = 28;

pub fn print(blame: &Blame) {
    for line in &blame.lines {
        let (hash, date, message) = match &line.origin {
            Some(o) => (
                style(&o.hash[..8]).yellow().to_string(),
                style(short_date(&o.created_at)).dim().to_string(),
                style(truncate(&o.message, MSG_WIDTH)).dim().to_string(),
            ),
            None => (
                style("????????").dim().to_string(),
                style(" ".repeat(16)).dim().to_string(),
                style("(unknown)").dim().to_string(),
            ),
        };
        println!(
            "{} {} {:<width$}  {}  {}",
            hash,
            date,
            message,
            style(format!("{:>4}", line.line_no)).dim(),
            line.text,
            width = MSG_WIDTH
        );
    }
}

/// `2026-08-04T12:30:59.123Z` → `2026-08-04 12:30`.
fn short_date(created_at: &str) -> String {
    if created_at.len() >= 16 {
        created_at[..16].replace('T', " ")
    } else {
        created_at.to_string()
    }
}

/// Truncate to `width` display cells, ellipsis included.
///
/// Counts characters rather than bytes: slicing bytes panics when the cut lands
/// mid-character, which a non-ASCII commit message can easily do.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let kept: String = s.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", kept)
}
