use std::fs;
use std::path::Path;

use console::style;

use crate::commands::{get_conflict_files, get_dirty_files, FileStatus};
use crate::error::Result;

/// Print how the current branch relates to its remote-tracking ref, if it has
/// one. Silent when the branch isn't tracked, so non-sync users see nothing new.
fn print_tracking_status(root: &Path, branch: &str) {
    let conn = match crate::db::get_conn_at_path(&root.join(".velo/velo.db")) {
        Ok(c) => c,
        Err(_) => return,
    };
    let (remote, remote_tip) = match crate::commands::tracking_ref(&conn, branch) {
        Some(x) => x,
        None => return,
    };
    let label = format!("{}/{}", remote, branch);

    let local_tip = match crate::commands::branch_tip(&conn, branch) {
        Some(t) => t,
        None => return,
    };
    // If we don't have the remote's tip locally, we can't count — nudge a fetch.
    let known: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
            [&remote_tip],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false);
    if !known {
        println!(
            "  {} tracking {} — run {} to compare.",
            style("↕").dim(),
            style(&label).cyan(),
            style("velo fetch").cyan()
        );
        return;
    }

    let (ahead, behind) = crate::commands::ahead_behind(&conn, &local_tip, &remote_tip);
    match (ahead, behind) {
        (0, 0) => println!("  {} up to date with {}", style("✔").green(), style(&label).cyan()),
        (a, 0) => println!(
            "  {} {} ahead of {} — {} to publish",
            style("↑").green().bold(),
            a,
            style(&label).cyan(),
            style("velo push").cyan()
        ),
        (0, b) => println!(
            "  {} {} behind {} — {} to catch up",
            style("↓").yellow().bold(),
            b,
            style(&label).cyan(),
            style("velo pull").cyan()
        ),
        (a, b) => println!(
            "  {} diverged from {} ({} ahead, {} behind) — {} then {}",
            style("↕").yellow().bold(),
            style(&label).cyan(),
            a,
            b,
            style("velo pull").cyan(),
            style(format!("velo merge {}", label)).cyan()
        ),
    }
}

pub fn run(root: &Path, paths: &[String]) -> Result<()> {
    let branch =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let parent_hash =
        fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let parent_hash = parent_hash.trim();

    // ── Header ────────────────────────────────────────────────────────────────
    let position_str = if parent_hash.is_empty() {
        style("no snapshots yet").dim().to_string()
    } else {
        style(parent_hash).yellow().to_string()
    };
    print!(
        "Branch: {}  Position: {}",
        style(branch.trim()).cyan().bold(),
        position_str
    );

    // Show the message of the current snapshot if one exists
    if !parent_hash.is_empty() {
        let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db"))?;
        if let Ok(msg) =
            conn.query_row("SELECT message FROM snapshots WHERE hash = ?", [parent_hash], |r| {
                r.get::<_, String>(0)
            })
        {
            print!("  \"{}\"", style(&msg).dim());
        }
    }
    println!();

    // ── Ahead/behind vs the tracked remote branch ─────────────────────────────
    // Uses the last fetched state (remote_refs) — no network access.
    print_tracking_status(root, branch.trim());

    // ── Merge-in-progress banner ──────────────────────────────────────────────
    let conflicts = get_conflict_files(root);
    if !conflicts.is_empty() {
        println!(
            "\n{} Merge in progress — {} conflict(s) unresolved.",
            style("!").yellow().bold(),
            conflicts.len()
        );
        for c in &conflicts {
            println!(
                "  {} {}",
                style("[Conflict]").red().bold(),
                c
            );
        }
        println!(
            "  Run {} or {} to resolve, then {}",
            style("velo resolve <file>").cyan(),
            style("velo resolve --all --take ours|theirs").cyan(),
            style("velo save \"Finish merge\"").green()
        );
        println!();
    }

    // ── Dirty files ───────────────────────────────────────────────────────────
    let raw_dirty2 = get_dirty_files(root);
    let dirty: std::collections::HashMap<String, FileStatus> =
        if paths.is_empty() { raw_dirty2 }
        else {
            raw_dirty2.into_iter()
                .filter(|(p, _)| paths.iter().any(|spec| p.starts_with(spec.as_str())))
                .collect()
        };

    if dirty.is_empty() && conflicts.is_empty() {
        println!("  {}", style("Working directory clean.").dim());
        return Ok(());
    }

    // Separate and sort by category
    let mut new_files: Vec<&str> = Vec::new();
    let mut modified: Vec<&str> = Vec::new();
    let mut deleted: Vec<&str> = Vec::new();

    for (path, status) in &dirty {
        match status {
            FileStatus::New => new_files.push(path.as_str()),
            FileStatus::Modified => modified.push(path.as_str()),
            FileStatus::Deleted => deleted.push(path.as_str()),
        }
    }
    new_files.sort_unstable();
    modified.sort_unstable();
    deleted.sort_unstable();

    if !new_files.is_empty() {
        println!("\n  {} {} file(s):", style("New").green().bold(), new_files.len());
        for f in &new_files {
            println!("    {}", style(f).green());
        }
    }
    if !modified.is_empty() {
        println!("\n  {} {} file(s):", style("Modified").yellow().bold(), modified.len());
        for f in &modified {
            println!("    {}", style(f).yellow());
        }
    }
    if !deleted.is_empty() {
        println!("\n  {} {} file(s):", style("Deleted").red().bold(), deleted.len());
        for f in &deleted {
            println!("    {}", style(f).red());
        }
    }

    let total = dirty.len();
    println!(
        "\n  {} change(s) total — use {} or {}",
        total,
        style("velo diff").cyan(),
        style("velo save \"<message>\"").green()
    );

    Ok(())
}