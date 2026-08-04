//! Render diffs for the terminal.
//!
//! Two presentations share one hunk printer:
//!
//! * [`print_comparison`] — `velo diff`, labelled `--- old  +++ new` per file.
//! * [`print_snapshot`] — `velo show`, which distinguishes added, deleted and
//!   modified files instead of labelling two sides.
//!
//! Core previously carried three hunk printers that had drifted apart (one put a
//! space after the +/- sign, one dimmed the `@@` header and one didn't), so the
//! same change looked different depending on which command you reached it
//! through. There is one printer now.

use console::{style, Color};
use velo_core::commands::diff::{Diff, DiffLine, FileChange, Hunk, LineTag};
use velo_core::commands::show::SnapshotDetail;

/// Width of the line-number gutter.
const GUTTER: usize = 5;

// ─── velo diff ────────────────────────────────────────────────────────────────

pub fn print_comparison(diff: &Diff) {
    if diff.is_empty() {
        println!("{}", style("Working directory clean.").dim());
        return;
    }

    for file in &diff.files {
        println!(
            "\n{}",
            style(format!("── {} ", file.path))
                .bold()
                .cyan()
                .underlined()
        );

        match &file.change {
            FileChange::Deleted => {
                println!("{} '{}' was deleted.", style("[-]").red().bold(), file.path);
            }
            FileChange::BinaryChanged { .. } => {
                println!(
                    "{} Binary file '{}' modified (diff omitted).",
                    style("[~]").yellow().bold(),
                    file.path
                );
            }
            FileChange::Added { lines } => {
                print_labels(diff);
                for (i, line) in lines.iter().enumerate() {
                    print_line(&DiffLine {
                        tag: LineTag::Added,
                        line_no: Some(i + 1),
                        text: line.clone(),
                    });
                }
            }
            FileChange::Modified { hunks } => {
                print_labels(diff);
                print_hunks(hunks);
            }
        }
    }
}

fn print_labels(diff: &Diff) {
    println!(
        "{} {}    {} {}",
        style("---").red(),
        style(&diff.old_label).dim(),
        style("+++").green(),
        style(&diff.new_label).dim()
    );
}

// ─── velo show ────────────────────────────────────────────────────────────────

pub fn print_snapshot(detail: &SnapshotDetail) {
    println!(
        "{} {}  {}  {}",
        style("snapshot").dim(),
        style(&detail.hash).yellow().bold(),
        style(&detail.branch).cyan(),
        style(short_date(&detail.created_at)).dim()
    );
    println!("  {}", style(&detail.message).white().bold());
    if let Some(parent) = &detail.parent {
        println!("  parent: {}", style(parent).yellow().dim());
    }
    println!();

    print_change_list(&detail.diff);
}

/// The added/deleted/modified presentation used by `show` and `stash show`.
pub fn print_change_list(diff: &Diff) {
    if diff.is_empty() {
        println!("{}", style("No changes in this snapshot.").dim());
        return;
    }

    for file in &diff.files {
        match &file.change {
            FileChange::Added { lines } => {
                println!(
                    "{} {}",
                    style("+++ new file:").green().bold(),
                    style(&file.path).green()
                );
                for line in lines {
                    println!("{}", style(format!("+{}", line)).green());
                }
            }
            FileChange::Deleted => println!(
                "{} {}",
                style("--- deleted:").red().bold(),
                style(&file.path).red()
            ),
            FileChange::Modified { hunks } => {
                println!(
                    "\n{} {}",
                    style("~~~ modified:").yellow().bold(),
                    style(&file.path).yellow().underlined()
                );
                print_hunks(hunks);
            }
            FileChange::BinaryChanged { added } => {
                if *added {
                    println!(
                        "{} {}",
                        style("+++ new file:").green().bold(),
                        style(&file.path).green()
                    );
                    println!("  {}", style("(binary)").dim());
                } else {
                    println!(
                        "\n{} {}",
                        style("~~~ modified:").yellow().bold(),
                        style(&file.path).yellow().underlined()
                    );
                    println!("  {}", style("binary file changed").dim());
                }
            }
        }
    }
}

// ─── Shared hunk printer ──────────────────────────────────────────────────────

fn print_hunks(hunks: &[Hunk]) {
    for hunk in hunks {
        println!(
            "{}",
            style(format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            ))
            .cyan()
            .dim()
        );
        for line in &hunk.lines {
            print_line(line);
        }
    }
}

fn print_line(line: &DiffLine) {
    let (sign, colour) = match line.tag {
        LineTag::Removed => ("-", Color::Red),
        LineTag::Added => ("+", Color::Green),
        LineTag::Context => (" ", Color::White),
    };
    let gutter = match line.line_no {
        Some(n) => format!("{:>width$}", n, width = GUTTER),
        None => " ".repeat(GUTTER),
    };
    println!(
        "{} {}{}",
        style(gutter).dim(),
        style(sign).fg(colour).bold(),
        style(&line.text).fg(colour)
    );
}

/// `2026-08-04T12:30:59.123Z` → `2026-08-04T12:30:59`.
fn short_date(created_at: &str) -> &str {
    if created_at.len() >= 19 {
        &created_at[..19]
    } else {
        created_at
    }
}
