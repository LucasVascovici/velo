//! Render an fsck [`Report`] for the terminal.

use console::style;
use velo_core::commands::fsck::{Report, Section};

pub fn print(report: &Report) {
    println!("{}", style("Checking repository integrity…").bold());

    for section in &report.sections {
        match section {
            // Cruft is explicitly not corruption, so it gets a warning mark
            // rather than a cross — a ✖ here contradicted the "No corruption"
            // summary printed a few lines later.
            Section::State { outstanding, .. } if *outstanding > 0 => println!(
                "  {} State ({} cleanup item(s))",
                style("!").yellow(),
                outstanding
            ),
            _ => report_line(section.problems(), &label(section)),
        }
    }

    // Only separate the per-item list when there is one; the summary below
    // supplies its own leading blank line.
    if !report.cruft.is_empty() || !report.repaired.is_empty() {
        println!();
        for item in &report.cruft {
            println!("  {} {}", style("!").yellow(), item.describe());
        }
        for item in &report.repaired {
            println!("  {} {}", style("✔").green(), item.describe_repaired());
        }
    }

    if !report.is_healthy() {
        // No summary line here: the caller turns an unhealthy report into an
        // error, and that error message already states the count.
        for problem in &report.problems {
            println!("  {} {}", style("✖").red().bold(), problem);
        }
    } else if report.has_cleanable_cruft() {
        println!(
            "\n{} No corruption; {} cleanup item(s) — run {} to tidy.",
            style("✔").green(),
            report.cruft.len(),
            style("velo fsck --repair").cyan()
        );
    } else {
        println!("\n{} Repository is healthy.", style("✔").green().bold());
    }
}

fn label(section: &Section) -> String {
    match section {
        Section::Objects {
            referenced,
            verified,
            ..
        } => format!("Objects: {} referenced, {} verified", referenced, verified),
        Section::Snapshots {
            checked,
            ids_verified,
            ids_legacy,
            ..
        } => {
            let legacy = if *ids_legacy > 0 {
                format!(", {} legacy (unverifiable)", ids_legacy)
            } else {
                String::new()
            };
            format!(
                "Snapshots: {} checked, {} ids verified{}",
                checked, ids_verified, legacy
            )
        }
        Section::Refs { .. } => "Refs: PARENT, tags, stash".to_string(),
        Section::Renames { checked, .. } => format!(
            "Renames: {} edge{} checked",
            checked,
            if *checked == 1 { "" } else { "s" }
        ),
        Section::State {
            outstanding,
            repaired,
        } => {
            if *repaired {
                "State: repaired".to_string()
            } else if *outstanding == 0 {
                "State: no cruft".to_string()
            } else {
                "State".to_string()
            }
        }
    }
}

fn report_line(problems: usize, label: &str) {
    if problems == 0 {
        println!("  {} {}", style("✔").green(), label);
    } else {
        println!(
            "  {} {} ({} problem(s))",
            style("✖").red().bold(),
            label,
            problems
        );
    }
}
