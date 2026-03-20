use std::path::Path;

use console::style;
use rusqlite::params;

use crate::db;
use crate::error::{Result, VeloError};

/// Create, list, or delete tags.
///
/// - `name = Some, snapshot = None`           → tag HEAD snapshot
/// - `name = Some, snapshot = Some(hash)`     → tag the named snapshot
/// - `name = None, delete = Some(tag)`        → delete that tag
/// - `name = None, delete = None`             → list all tags
/// - `force`                                  → overwrite existing tag without error
pub fn run(
    root: &Path,
    name: Option<String>,
    snapshot: Option<String>,
    delete: Option<String>,
    force: bool,
) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // ── Delete a tag ──────────────────────────────────────────────────────────
    if let Some(del_tag) = delete {
        let rows = conn.execute("DELETE FROM tags WHERE name = ?", [&del_tag])?;
        if rows == 0 {
            return Err(VeloError::InvalidInput(format!(
                "Tag '{}' not found.",
                del_tag
            )));
        }
        println!(
            "{} Deleted tag '{}'.",
            style("✔").green(),
            style(&del_tag).yellow()
        );
        return Ok(());
    }

    // ── Create a tag ──────────────────────────────────────────────────────────
    if let Some(tag_name) = name {
        // Resolve the target hash
        let target_hash = if let Some(snap_id) = snapshot {
            // User-supplied snapshot id (hash prefix or tag)
            crate::commands::resolve_snapshot_id(root, &snap_id)?
        } else {
            // Default to HEAD
            let parent_hash =
                std::fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
            let h = parent_hash.trim().to_string();
            if h.is_empty() {
                return Err(VeloError::InvalidInput(
                    "No snapshot to tag. Save something first.".into(),
                ));
            }
            h
        };

        // Check for existing tag
        let existing: Option<String> = conn
            .query_row(
                "SELECT snapshot_hash FROM tags WHERE name = ?",
                [&tag_name],
                |r| r.get(0),
            )
            .ok();

        if let Some(prev) = existing {
            if !force {
                return Err(VeloError::InvalidInput(format!(
                    "Tag '{}' already exists (→ {}). Use --force to overwrite.",
                    tag_name, prev
                )));
            }
            println!(
                "{} Overwriting tag '{}' (was → {}).",
                style("!").yellow(),
                style(&tag_name).yellow(),
                style(&prev).dim()
            );
        }

        conn.execute(
            "INSERT OR REPLACE INTO tags (name, snapshot_hash) VALUES (?, ?)",
            params![tag_name, target_hash],
        )?;
        println!(
            "{} Tagged {} as '{}'.",
            style("✔").green(),
            style(&target_hash).yellow(),
            style(&tag_name).cyan()
        );
        return Ok(());
    }

    // ── List all tags ─────────────────────────────────────────────────────────
    let mut stmt = conn.prepare(
        "SELECT t.name, t.snapshot_hash, s.message
         FROM tags t
         LEFT JOIN snapshots s ON t.snapshot_hash = s.hash
         ORDER BY t.name",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?
                    .unwrap_or_else(|| "(deleted)".into()),
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        println!("{}", style("No tags defined.").dim());
        return Ok(());
    }

    println!("{:<20} | {:<14} | {}", "Tag", "Snapshot", "Message");
    println!("{}", "-".repeat(60));
    for (name, hash, msg) in &rows {
        println!(
            "{:<20} | {:<14} | {}",
            style(name).cyan(),
            style(hash).yellow(),
            style(msg).dim()
        );
    }

    Ok(())
}
