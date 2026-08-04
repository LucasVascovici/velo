//! `velo tag` — create, list, and delete tags.
//!
//! One `run` taking four optional arguments became three entry points, one per
//! operation. Formatting lives in `velo-cli`.

use rusqlite::params;

use crate::error::{RefKind, Result, VeloError};
use crate::Repo;
use crate::WriteGuard;

/// A tag and what it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub snapshot: String,
    /// Message of the tagged snapshot. `None` when the snapshot is gone, which
    /// can happen after `undo` shelves it.
    pub message: Option<String>,
}

/// The outcome of creating a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub name: String,
    pub snapshot: String,
    /// Set when `force` overwrote an existing tag, carrying what it pointed at.
    pub replaced: Option<String>,
}

/// Every tag, ordered by name.
pub fn list(repo: &Repo) -> Result<Vec<Tag>> {
    let conn = repo.conn();
    let mut stmt = conn.prepare(
        "SELECT t.name, t.snapshot_hash, s.message
         FROM tags t
         LEFT JOIN snapshots s ON t.snapshot_hash = s.hash
         ORDER BY t.name",
    )?;
    let tags: Vec<Tag> = stmt
        .query_map([], |r| {
            Ok(Tag {
                name: r.get(0)?,
                snapshot: r.get(1)?,
                message: r.get(2)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(tags)
}

/// Tag `snapshot` — or the current position when `None` — as `name`.
///
/// Fails if the name is taken unless `force` is set.
pub fn create(
    guard: &WriteGuard,
    name: &str,
    snapshot: Option<&str>,
    force: bool,
) -> Result<Created> {
    let root = guard.root();
    let conn = guard.conn();

    let target = match snapshot {
        Some(id) => crate::commands::resolve_snapshot_id(guard.repo(), id)?,
        None => {
            let position = std::fs::read_to_string(root.join(".velo/PARENT"))
                .unwrap_or_default()
                .trim()
                .to_string();
            if position.is_empty() {
                return Err(VeloError::invalid(
                    "No snapshot to tag. Save something first.",
                ));
            }
            position
        }
    };

    let existing: Option<String> = conn
        .query_row(
            "SELECT snapshot_hash FROM tags WHERE name = ?",
            [name],
            |r| r.get(0),
        )
        .ok();
    if let Some(prev) = &existing {
        if !force {
            return Err(VeloError::invalid(format!(
                "Tag '{}' already exists (→ {}). Use --force to overwrite.",
                name, prev
            )));
        }
    }

    conn.execute(
        "INSERT OR REPLACE INTO tags (name, snapshot_hash) VALUES (?, ?)",
        params![name, target],
    )?;

    Ok(Created {
        name: name.to_string(),
        snapshot: target,
        replaced: existing,
    })
}

/// Delete the tag called `name`.
pub fn delete(guard: &WriteGuard, name: &str) -> Result<()> {
    let conn = guard.conn();
    let rows = conn.execute("DELETE FROM tags WHERE name = ?", [name])?;
    if rows == 0 {
        return Err(VeloError::not_found(RefKind::Tag, name));
    }
    Ok(())
}
