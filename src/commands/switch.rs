use std::fs;
use std::path::Path;

use console::style;

use crate::commands::get_dirty_files;
use crate::db;
use crate::error::{Result, VeloError};

pub fn run(root: &Path, branch_name: &str, force: bool) -> Result<()> {
    // ── Guard: can't switch to a soft-deleted branch ──────────────────────────
    if branch_name.starts_with("_deleted_") {
        return Err(VeloError::InvalidInput(format!(
            "Branch '{}' has been deleted.",
            branch_name.trim_start_matches("_deleted_")
        )));
    }

    // ── Early exit: already on this branch ────────────────────────────────────
    let current_head =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    if current_head.trim() == branch_name {
        println!(
            "Already on branch '{}'.",
            style(branch_name).cyan().bold()
        );
        return Ok(());
    }

    // ── Dirty check ───────────────────────────────────────────────────────────
    let dirty = get_dirty_files(root);
    if !dirty.is_empty() {
        if !force {
            println!(
                "{} Unsaved changes detected — aborting switch.",
                style("✖").red().bold()
            );
            let mut keys: Vec<_> = dirty.keys().collect();
            keys.sort();
            for k in &keys {
                println!("  {}", style(k).yellow());
            }
            println!(
                "Use {} to switch anyway (changes will be discarded).",
                style(format!("velo switch {} --force", branch_name)).cyan()
            );
            return Ok(());
        }
        println!(
            "{} Discarding {} unsaved change(s).",
            style("!").yellow().bold(),
            dirty.len()
        );
    }

    // ── Save current PARENT so the new branch can inherit it if it's new ──────
    let parent_hash =
        fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // ── Find the latest snapshot on the target branch ─────────────────────────
    let latest_hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM snapshots WHERE branch = ? ORDER BY created_at DESC, rowid DESC LIMIT 1",
            [branch_name],
            |r| r.get(0),
        )
        .ok();

    // ── Update HEAD ───────────────────────────────────────────────────────────
    fs::write(root.join(".velo/HEAD"), branch_name)?;

    if let Some(hash) = latest_hash {
        println!(
            "Switched to branch '{}' at snapshot {}",
            style(branch_name).cyan().bold(),
            style(&hash).yellow()
        );
        crate::commands::restore::run(root, &hash, true)?;
    } else {
        // New branch — inherit working tree state from current position
        let from = parent_hash.trim();
        let from_display = if from.is_empty() {
            "(empty)".to_string()
        } else {
            from.to_string()
        };
        println!(
            "{} Created and switched to new branch '{}' (inheriting from {}).",
            style("✨").bold(),
            style(branch_name).cyan().bold(),
            style(&from_display).yellow()
        );
    }
    Ok(())
}