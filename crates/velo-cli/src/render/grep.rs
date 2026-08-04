//! Render [`GrepResults`] for the terminal.

use console::style;
use velo_core::commands::grep::{GrepResults, MatchLine};

pub fn print(results: &GrepResults, names_only: bool) {
    if let Some(snap) = &results.snapshot {
        println!(
            "\n{} {}  {}",
            style("Searching snapshot").dim(),
            style(&snap.hash[..8]).yellow(),
            style(&snap.message).dim()
        );
    }

    for file in &results.files {
        println!("\n{}", style(&file.path).cyan().bold().underlined());
        if names_only {
            continue;
        }
        for line in &file.lines {
            print_line(line);
        }
    }

    if results.is_empty() {
        println!(
            "{}",
            style(format!("No matches for '{}'.", results.pattern)).dim()
        );
    }
}

fn print_line(line: &MatchLine) {
    if line.gap_before {
        println!("  {}", style("···").dim());
    }
    let line_no = style(format!("{:>4}", line.line_no)).dim();
    if line.is_match {
        let separator = style(":").yellow().bold();
        println!("  {}{}  {}", line_no, separator, highlight(line));
    } else {
        let separator = style("│").dim();
        println!("  {}{}  {}", line_no, separator, style(&line.text).dim());
    }
}

/// Wrap the matched ranges in bold+yellow, leaving the rest of the line plain.
fn highlight(line: &MatchLine) -> String {
    let mut out = String::new();
    let mut last = 0;
    for &(start, end) in &line.spans {
        // Ranges come from the same string, in order, and never overlap.
        out.push_str(&line.text[last..start]);
        out.push_str(&style(&line.text[start..end]).yellow().bold().to_string());
        last = end;
    }
    out.push_str(&line.text[last..]);
    out
}
