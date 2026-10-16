//! Render a retention pass for the terminal.

use console::style;
use velo_core::commands::compact::Protected;
use velo_core::commands::retention::Report;

pub fn print(report: &Report, dry_run: bool) {
    println!(
        "{} Retention on '{}': examined {} snapshot(s).",
        style("◆").cyan().bold(),
        style(&report.branch).cyan(),
        report.examined
    );

    if !report.protected.is_empty() {
        println!("  Protected ({} kept as they are):", report.protected.len());
        for (id, why) in &report.protected {
            // Signed and tagged ones are the ones an operator asked to keep, so
            // they are marked rather than left to the reason text.
            let mark = match why {
                Protected::Signed | Protected::Tagged => style("*").green().bold(),
                _ => style("-").dim(),
            };
            println!(
                "    {} {} {}",
                mark,
                style(super::id::short(id)).yellow(),
                style(why).dim()
            );
        }
    }

    if report.removed == 0 {
        println!("{}", style("Nothing to thin.").dim());
        return;
    }
    if dry_run {
        println!(
            "{} would remove {} snapshot(s). Nothing was changed (dry run).",
            style("~").yellow().bold(),
            report.removed
        );
    } else {
        println!(
            "{} Removed {} snapshot(s).",
            style("✔").green().bold(),
            report.removed
        );
    }
}
