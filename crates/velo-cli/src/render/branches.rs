//! Render branch listings for the terminal.

use console::style;
use velo_core::commands::branches::Branch;

pub fn print_list(branches: &[Branch]) {
    println!("{}", style("Branches:").bold());
    let name_w = name_width(branches);
    for b in branches {
        println!("{}", line(b, name_w));
    }
}

/// Width of the name column, taken from the widest name actually listed.
///
/// Derived rather than hard-coded: branch names run from `dev` to
/// `remotes/origin/main`, and any fixed width would either truncate the long
/// ones or leave a gulf after the short ones.
fn name_width(branches: &[Branch]) -> usize {
    branches
        .iter()
        .map(|b| b.name.chars().count())
        .max()
        .unwrap_or(0)
}

/// One listing row, as a pure function of the branch and the column width.
fn line(b: &Branch, name_w: usize) -> String {
    let prefix = if b.is_current { "* " } else { "  " };
    // Pad before styling, and pad the `BranchName` itself: its `Display` uses
    // `f.pad`, which counts chars, so a non-ASCII name lines up. Styling first
    // would pad the wrong thing — ANSI escapes count toward a format width.
    let name = format!("{:<name_w$}", b.name);
    let name = if b.is_current {
        style(name).green().bold()
    } else {
        style(name).white()
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
    format!("  {}{}{}", prefix, name, meta)
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

#[cfg(test)]
mod tests {
    use super::*;
    use velo_core::commands::branches::Tip;

    fn branch(name: &str, hash: &str, is_current: bool) -> Branch {
        Branch {
            name: name.parse().unwrap(),
            is_current,
            tip: Some(Tip {
                hash: hash.parse().unwrap(),
                message: "second".into(),
                created_at: "2026-08-05T09:41:12".into(),
            }),
        }
    }

    /// Colours are off under `cargo test`, but strip anyway so the assertions
    /// hold whatever `CLICOLOR_FORCE` says.
    fn rows(branches: &[Branch]) -> Vec<String> {
        let w = name_width(branches);
        branches
            .iter()
            .map(|b| console::strip_ansi_codes(&line(b, w)).into_owned())
            .collect()
    }

    /// The column a row's metadata starts at, counted in chars.
    fn meta_col(row: &str) -> usize {
        let byte = row.find("  7d").or_else(|| row.find("  (no")).unwrap();
        row[..byte].chars().count()
    }

    #[test]
    fn names_are_padded_to_a_common_width() {
        let branches = [
            branch("dev", "7d202741", true),
            branch("remotes/origin/main", "7d202741", false),
        ];
        let rows = rows(&branches);
        // "remotes/origin/main" is 19 wide, so "dev" carries 16 spaces of padding.
        assert_eq!(
            rows[0],
            format!(
                "  * dev{}  7d202741 2026-08-05 · \"second\"",
                " ".repeat(16)
            )
        );
        assert_eq!(
            rows[1],
            "    remotes/origin/main  7d202741 2026-08-05 · \"second\""
        );
        assert_eq!(meta_col(&rows[0]), meta_col(&rows[1]));
    }

    #[test]
    fn a_multibyte_name_pads_by_chars() {
        // "ветка" is 5 chars in 10 bytes: padding on byte length would push this
        // row five places left of the other one.
        let branches = [
            branch("ветка", "7d202741", false),
            branch("main-branch", "7d202741", true),
        ];
        let rows = rows(&branches);
        assert_eq!(meta_col(&rows[0]), meta_col(&rows[1]));
    }

    #[test]
    fn a_branch_without_a_tip_still_lines_up() {
        let mut unborn = branch("dev", "7d202741", true);
        unborn.tip = None;
        let branches = [unborn, branch("remotes/origin/main", "7d202741", false)];
        let rows = rows(&branches);
        assert!(rows[0].ends_with("(no commits yet)"));
        assert_eq!(meta_col(&rows[0]), meta_col(&rows[1]));
    }

    #[test]
    fn a_lone_branch_gets_no_stray_padding() {
        let rows = rows(&[branch("main", "5949005c", true)]);
        assert_eq!(rows[0], "  * main  5949005c 2026-08-05 · \"second\"");
    }
}
