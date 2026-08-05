//! `velo tag` — create, list, and delete tags.
//!
//! One `run` taking four optional arguments became three entry points, one per
//! operation. Formatting lives in `velo-cli`.

use rusqlite::params;

use crate::error::{RefKind, Result, VeloError};
use crate::{Repo, SnapshotId, TagName, WriteGuard};

/// A tag and what it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: TagName,
    pub snapshot: SnapshotId,
    /// Message of the tagged snapshot. `None` when the snapshot is gone, which
    /// can happen after `undo` shelves it.
    pub message: Option<String>,
}

/// The outcome of creating a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub name: TagName,
    pub snapshot: SnapshotId,
    /// Set when `force` overwrote an existing tag, carrying what it pointed at.
    pub replaced: Option<SnapshotId>,
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
/// Tag `snapshot`, or the current position when it is `None`.
///
/// Takes a resolved [`SnapshotId`] rather than a spec: a caller that already has
/// an id — everyone who just saved something — would otherwise hand back text to
/// be resolved a second time. Resolve a user's spec with
/// [`resolve_snapshot_id`](crate::commands::resolve_snapshot_id) first.
pub fn create(
    guard: &WriteGuard,
    name: &TagName,
    snapshot: Option<&SnapshotId>,
    force: bool,
) -> Result<Created> {
    let root = guard.root();
    let conn = guard.conn();

    let target = match snapshot {
        Some(id) => id.clone().into_string(),
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

    let existing: Option<SnapshotId> = conn
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
        name: name.clone(),
        snapshot: SnapshotId::from_stored(target),
        replaced: existing,
    })
}

/// Delete the tag called `name`.
pub fn delete(guard: &WriteGuard, name: &TagName) -> Result<()> {
    let conn = guard.conn();
    let rows = conn.execute("DELETE FROM tags WHERE name = ?", [name])?;
    if rows == 0 {
        return Err(VeloError::not_found(RefKind::Tag, name.as_str()));
    }
    Ok(())
}
