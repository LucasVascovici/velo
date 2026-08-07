//! Render [`Blame`] for the terminal.

use console::style;
use velo_core::commands::blame::Blame;

/// Width of the message column, matching the `{:<28}` layout below.
const MSG_WIDTH: usize = 28;

/// Width of the author column, when there is one.
const WHO_WIDTH: usize = 14;

pub fn print(blame: &Blame) {
    // The column appears only when the history has authors to show. A repository
    // saved without them would otherwise get a stripe of blanks in every line.
    let show_author = blame
        .lines
        .iter()
        .any(|l| l.origin.as_ref().is_some_and(|o| o.author.is_some()));

    for line in &blame.lines {
        let (hash, date, message) = match &line.origin {
            Some(o) => (
                style(super::id::short(&o.hash)).yellow().to_string(),
                style(super::when::minutes(o.created_at)).dim().to_string(),
                style(truncate(&o.message, MSG_WIDTH)).dim().to_string(),
            ),
            None => (
                style("????????").dim().to_string(),
                style(" ".repeat(16)).dim().to_string(),
                style("(unknown)").dim().to_string(),
            ),
        };
        let who = if show_author {
            let name = line
                .origin
                .as_ref()
                .and_then(|o| o.author.as_ref())
                .map_or("", |a| a.name());
            format!(
                "{} ",
                style(format!("{:<WHO_WIDTH$}", truncate(name, WHO_WIDTH))).cyan()
            )
        } else {
            String::new()
        };
        println!(
            "{} {} {}{:<width$}  {}  {}",
            hash,
            date,
            who,
            message,
            style(format!("{:>4}", line.line_no)).dim(),
            line.text,
            width = MSG_WIDTH
        );
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
