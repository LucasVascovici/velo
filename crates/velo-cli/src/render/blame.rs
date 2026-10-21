//! Render [`Blame`] for the terminal.

use console::style;
use velo_core::commands::blame::Blame;

/// Width of the message column, matching the `{:<28}` layout below.
const MSG_WIDTH: usize = 28;

/// Width of the author column, when there is one.
const WHO_WIDTH: usize = 14;

/// What the author cell says for a line whose snapshot recorded no author.
///
/// A blank cell vanishes under whitespace splitting, which moves the first word
/// of the commit message into the author's token position for any script that
/// reads the text. A fixed word keeps the column count stable.
const NO_AUTHOR: &str = "(unknown)";

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
                .map_or(NO_AUTHOR, |a| a.name());
            format!(
                "{} ",
                style(format!("{:<WHO_WIDTH$}", truncate(name, WHO_WIDTH))).cyan()
            )
        } else {
            String::new()
        };
        let prefix = format!(
            "{} {} {}{:<width$}  {}  ",
            hash,
            date,
            who,
            message,
            style(format!("{:>4}", line.line_no)).dim(),
            width = MSG_WIDTH
        );
        // A unit can span several lines under a paragraph or key driver. Its
        // first line carries the gutter; the rest get a blank one of the same
        // width so the text stays in its column.
        let blank = " ".repeat(console::measure_text_width(&prefix));
        let mut text = line.text.split('\n');
        println!("{}{}", prefix, text.next().unwrap_or(""));
        for rest in text {
            println!("{}{}", blank, rest);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_placeholder_is_one_token_that_fits_the_column() {
        assert_eq!(NO_AUTHOR.split_whitespace().count(), 1);
        assert!(NO_AUTHOR.chars().count() <= WHO_WIDTH);
        assert_eq!(truncate(NO_AUTHOR, WHO_WIDTH), NO_AUTHOR);
    }
}
