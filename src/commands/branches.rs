use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::db;
use crate::error::{Result, VeloError};

pub fn run(root: &Path, delete: Option<String>) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let head_raw =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let current = head_raw.trim();

    // ── Delete branch ─────────────────────────────────────────────────────────
    if let Some(del) = delete {
        if del.trim() == current {
            return Err(VeloError::InvalidInput(format!(
                "Cannot delete the currently active branch '{}'. Switch to another branch first.",
                del
            )));
        }
        if del == "main" {
            return Err(VeloError::InvalidInput(
                "Cannot delete the default 'main' branch.".into(),
            ));
        }
        // Soft-delete: rename the branch so its snapshots stay in the DB
        let rows = conn.execute(
            "UPDATE snapshots SET branch = ?1 WHERE branch = ?2",
            params![format!("_deleted_{}", del), del],
        )?;
        if rows == 0 {
            return Err(VeloError::InvalidInput(format!(
                "Branch '{}' not found.",
                del
            )));
        }
        println!(
            "{} Deleted branch '{}'.",
            style("✔").green(),
            style(&del).yellow()
        );
        return Ok(());
    }

    // ── List branches with metadata ───────────────────────────────────────────
    struct BranchInfo {
        name: String,
        last_hash: Option<String>,
        last_msg: Option<String>,
        last_date: Option<String>,
    }

    // Collect distinct non-deleted branch names (from DB + current HEAD)
    let mut stmt = conn.prepare(
        "SELECT DISTINCT branch FROM snapshots WHERE branch NOT LIKE '_deleted_%'",
    )?;
    let mut branches: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    // The current branch might not have any snapshots yet
    if !branches.iter().any(|b| b.trim() == current) {
        branches.push(current.to_string());
    }
    branches.sort();

    // Fetch the latest snapshot for each branch
    let infos: Vec<BranchInfo> = branches
        .into_iter()
        .map(|name| {
            let row: Option<(String, String, String)> = conn
                .query_row(
                    "SELECT hash, message, created_at FROM snapshots
                     WHERE branch = ?
                     ORDER BY created_at DESC, rowid DESC LIMIT 1",
                    [&name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok();
            if let Some((hash, msg, date)) = row {
                let date_short = if date.len() >= 10 {
                    date[..10].to_string()
                } else {
                    date
                };
                BranchInfo {
                    name,
                    last_hash: Some(hash),
                    last_msg: Some(msg),
                    last_date: Some(date_short),
                }
            } else {
                BranchInfo {
                    name,
                    last_hash: None,
                    last_msg: None,
                    last_date: None,
                }
            }
        })
        .collect();

    println!("{}", style("Branches:").bold());
    for info in &infos {
        let is_current = info.name.trim() == current;
        let prefix = if is_current { "* " } else { "  " };
        let name_str = if is_current {
            style(&info.name).green().bold().to_string()
        } else {
            style(&info.name).white().to_string()
        };

        let meta = match (&info.last_hash, &info.last_msg, &info.last_date) {
            (Some(h), Some(m), Some(d)) => format!(
                "  {} {} · \"{}\"",
                style(&h[..8.min(h.len())]).yellow().dim(),
                style(d).dim(),
                style(m).dim()
            ),
            _ => style("  (no snapshots)").dim().to_string(),
        };

        println!("  {}{}{}", prefix, name_str, meta);
    }

    Ok(())
}