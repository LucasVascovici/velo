use std::fs;
use std::path::Path;

use console::style;

use crate::commands::{get_conflict_files, get_dirty_files, FileStatus};
use crate::error::Result;

pub fn run(root: &Path) -> Result<()> {
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
    let dirty = get_dirty_files(root);

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