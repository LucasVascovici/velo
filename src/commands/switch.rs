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
    let current_head = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_default();
    if current_head.trim() == branch_name {
        println!("Already on branch '{}'.", style(branch_name).cyan().bold());
        return Ok(());
    }

    // ── Save current PARENT so the new branch can inherit it if it's new ──────
    let parent_hash = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // ── Find where the target branch points ───────────────────────────────────
    let latest_hash: Option<String> = crate::commands::branch_tip(&conn, branch_name);

    // ── Dirty check ───────────────────────────────────────────────────────────
    // Only *tracked* changes are at risk: switching restores the target
    // snapshot over them. Brand-new files aren't in any snapshot, so they're
    // simply carried along — blocking on them (and then deleting them under
    // --force) was both surprising and destructive. Creating a branch that
    // inherits the current position restores nothing, so it never blocks.
    let dirty = get_dirty_files(root);
    let at_risk: Vec<&String> = dirty
        .iter()
        .filter(|(_, s)| **s != crate::commands::FileStatus::New)
        .map(|(p, _)| p)
        .collect();

    if latest_hash.is_some() && !at_risk.is_empty() && !force {
        println!(
            "{} Unsaved changes to tracked files — aborting switch.",
            style("✖").red().bold()
        );
        let mut keys: Vec<&&String> = at_risk.iter().collect();
        keys.sort();
        for k in keys {
            println!("  {}", style(k).yellow());
        }
        println!(
            "Save them first, or use {} to discard them.",
            style(format!("velo switch {} --force", branch_name)).cyan()
        );
        return Ok(());
    }

    // Record that the branch exists, but leave it unborn: merely visiting a
    // branch must never move it. It starts pointing somewhere only when you
    // explicitly save on it, or merge/pull into it.
    let already_known = crate::commands::branch_exists(&conn, branch_name);
    crate::commands::register_branch(&conn, branch_name, "")?;

    // ── Update HEAD ───────────────────────────────────────────────────────────
    crate::storage::write_atomic(&root.join(".velo/HEAD"), branch_name.as_bytes())?;

    if let Some(hash) = latest_hash {
        println!(
            "Switched to branch '{}' at snapshot {}",
            style(branch_name).cyan().bold(),
            style(&hash).yellow()
        );
        crate::commands::restore::run(root, &hash, true, &[])?;
    } else {
        // The branch has no commits yet, so there is nothing to restore — the
        // working tree carries over. It stays unborn until you save on it (which
        // starts it from where you are now) or merge into it.
        let verb = if already_known {
            "Switched to"
        } else {
            "Created and switched to"
        };
        let from = parent_hash.trim();
        if from.is_empty() {
            println!(
                "{} {} branch '{}' — no commits yet, so your first {} starts its history.",
                style("✨").bold(),
                verb,
                style(branch_name).cyan().bold(),
                style("velo save").cyan()
            );
        } else {
            println!(
                "{} {} branch '{}' — no commits yet; your first {} will start it from {}.",
                style("✨").bold(),
                verb,
                style(branch_name).cyan().bold(),
                style("velo save").cyan(),
                style(from).yellow()
            );
        }
    }
    Ok(())
}
