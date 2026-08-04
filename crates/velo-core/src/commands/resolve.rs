//! Conflict inspection and resolution — the data layer.
//!
//! Core exposes conflicts as data and applies decisions; it never prompts. The
//! interactive hunk-by-hunk navigator lives in `velo-tui`, which drives the
//! functions here. Non-interactive resolution (`--take ours|theirs`) needs no UI
//! and is fully served by [`take_side`].

use std::path::Path;

use rusqlite::params;
use velo_merge::{build_resolved_content, compute_conflict_hunks, ConflictHunk, Decision};

use crate::db;
use crate::error::{RefKind, Result, VeloError};
use crate::storage;
use crate::{Repo, WriteGuard};

/// Which side to take when resolving without per-hunk decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakeOption {
    Ours,
    Theirs,
}

impl TakeOption {
    /// The equivalent per-hunk decision.
    pub fn decision(self) -> Decision {
        match self {
            TakeOption::Ours => Decision::Ours,
            TakeOption::Theirs => Decision::Theirs,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TakeOption::Ours => "ours",
            TakeOption::Theirs => "theirs",
        }
    }
}

/// A file with an unresolved conflict, plus the three object versions needed to
/// reconstruct its hunks.
#[derive(Clone, Debug)]
pub struct ConflictFile {
    pub path: String,
    pub ancestor_hash: String,
    pub our_hash: String,
    pub their_hash: String,
}

/// A conflict file with its computed hunks and any decisions already recorded by
/// an earlier, partially-completed session.
#[derive(Debug)]
pub struct ConflictSession {
    pub file: ConflictFile,
    pub ancestor: String,
    pub ours: String,
    pub theirs: String,
    pub hunks: Vec<ConflictHunk>,
}

impl ConflictSession {
    pub fn all_decided(&self) -> bool {
        self.hunks.iter().all(|h| h.decision.is_some())
    }

    pub fn decided_count(&self) -> usize {
        self.hunks.iter().filter(|h| h.decision.is_some()).count()
    }
}

// ─── Queries ──────────────────────────────────────────────────────────────────

/// Is a merge/cherry-pick/rebase conflict state active?
pub fn merge_active(repo: &Repo) -> bool {
    repo.root().join(".velo/MERGE_HEAD").exists() || conflict_count(repo) > 0
}

/// How many files are currently in conflict.
pub fn conflict_count(repo: &Repo) -> i64 {
    repo.conn()
        .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
        .unwrap_or(0)
}

/// Every file currently in conflict, ordered by path.
pub fn list_conflicts(repo: &Repo) -> Result<Vec<ConflictFile>> {
    let mut stmt = repo.conn().prepare(
        "SELECT path, ancestor_hash, our_hash, their_hash FROM conflict_files ORDER BY path",
    )?;
    let rows: Vec<ConflictFile> = stmt
        .query_map([], |r| {
            Ok(ConflictFile {
                path: r.get(0)?,
                ancestor_hash: r.get(1)?,
                our_hash: r.get(2)?,
                their_hash: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Look up one conflict by path.
pub fn get_conflict(repo: &Repo, path: &str) -> Result<ConflictFile> {
    let normalised = db::normalise(path);
    repo.conn()
        .query_row(
            "SELECT path, ancestor_hash, our_hash, their_hash FROM conflict_files WHERE path = ?",
            [&normalised],
            |r| {
                Ok(ConflictFile {
                    path: r.get(0)?,
                    ancestor_hash: r.get(1)?,
                    our_hash: r.get(2)?,
                    their_hash: r.get(3)?,
                })
            },
        )
        .map_err(|_| VeloError::not_found(RefKind::Path, path))
}

/// Load a conflict's three sides, compute its hunks, and re-attach decisions
/// persisted by a previous session so resolution is resumable.
pub fn open_session(repo: &Repo, file: ConflictFile) -> Result<ConflictSession> {
    let conn = repo.conn();
    let objects_dir = repo.root().join(".velo/objects");
    let ancestor = read_text(&objects_dir, &file.ancestor_hash)?;
    let ours = read_text(&objects_dir, &file.our_hash)?;
    let theirs = read_text(&objects_dir, &file.their_hash)?;

    let mut hunks = compute_conflict_hunks(&ancestor, &ours, &theirs);
    for h in &mut hunks {
        if let Ok((kind, manual)) = conn.query_row(
            "SELECT decision, manual_content FROM hunk_decisions
             WHERE file_path = ? AND hunk_id = ?",
            params![file.path, h.id as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        ) {
            h.decision = decision_from_db(&kind, manual.as_deref());
        }
    }

    Ok(ConflictSession {
        file,
        ancestor,
        ours,
        theirs,
        hunks,
    })
}

// ─── Mutations ────────────────────────────────────────────────────────────────

/// Record one hunk's decision, so a partially-resolved file survives a restart.
pub fn record_decision(
    guard: &WriteGuard,
    path: &str,
    hunk_id: usize,
    decision: &Decision,
) -> Result<()> {
    let (kind, manual) = decision_to_db(decision);
    guard.conn().execute(
        "INSERT OR REPLACE INTO hunk_decisions (file_path, hunk_id, decision, manual_content)
         VALUES (?, ?, ?, ?)",
        params![path, hunk_id as i64, kind, manual],
    )?;
    Ok(())
}

/// Forget one hunk's decision.
pub fn clear_decision(guard: &WriteGuard, path: &str, hunk_id: usize) -> Result<()> {
    let conn = guard.conn();
    conn.execute(
        "DELETE FROM hunk_decisions WHERE file_path = ? AND hunk_id = ?",
        params![path, hunk_id as i64],
    )?;
    Ok(())
}

/// Write the resolved file to the working tree and clear its conflict state.
///
/// Undecided hunks fall back to "ours", matching the merge engine's default.
pub fn finalise(guard: &WriteGuard, session: &ConflictSession) -> Result<()> {
    let anc: Vec<&str> = session.ancestor.lines().collect();
    let our: Vec<&str> = session.ours.lines().collect();
    let thr: Vec<&str> = session.theirs.lines().collect();

    let resolved = build_resolved_content(
        &anc,
        &our,
        &thr,
        &session.hunks,
        session.ours.ends_with('\n'),
    );
    std::fs::write(
        guard.root().join(db::db_to_path(&session.file.path)),
        &resolved,
    )?;
    clear_conflict(guard, &session.file.path)
}

/// Resolve a whole file by taking one side, with no per-hunk interaction.
pub fn take_side(guard: &WriteGuard, file: &ConflictFile, side: TakeOption) -> Result<()> {
    let mut session = open_session(guard.repo(), file.clone())?;
    let decision = side.decision();
    for h in &mut session.hunks {
        h.decision = Some(decision.clone());
    }
    for h in &session.hunks {
        record_decision(guard, &file.path, h.id, &decision)?;
    }
    finalise(guard, &session)
}

/// Drop a file's conflict rows once it is resolved.
pub fn clear_conflict(guard: &WriteGuard, path: &str) -> Result<()> {
    let conn = guard.conn();
    conn.execute("DELETE FROM conflict_files WHERE path      = ?", [path])?;
    conn.execute("DELETE FROM hunk_decisions WHERE file_path = ?", [path])?;
    Ok(())
}

// ─── Decision persistence ─────────────────────────────────────────────────────
// `Decision` lives in velo-merge, which knows nothing about SQLite, so mapping it
// to and from storage belongs here.

pub fn decision_to_db(d: &Decision) -> (&'static str, Option<String>) {
    match d {
        Decision::Ours => ("ours", None),
        Decision::Theirs => ("theirs", None),
        Decision::BothOursFirst => ("both_ours", None),
        Decision::BothTheirsFirst => ("both_theirs", None),
        Decision::Manual(lines) => ("manual", Some(lines.join("\n"))),
    }
}

pub fn decision_from_db(kind: &str, content: Option<&str>) -> Option<Decision> {
    match kind {
        "ours" => Some(Decision::Ours),
        "theirs" => Some(Decision::Theirs),
        "both_ours" => Some(Decision::BothOursFirst),
        "both_theirs" => Some(Decision::BothTheirsFirst),
        "manual" => Some(Decision::Manual(
            content.unwrap_or("").lines().map(String::from).collect(),
        )),
        _ => None,
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Decompress an object as text. An empty hash means "absent", which is a valid
/// side of a conflict (a file added or deleted on one side).
pub fn read_text(objects_dir: &Path, hash: &str) -> Result<String> {
    if hash.is_empty() {
        return Ok(String::new());
    }
    let bytes = storage::read_object(objects_dir, hash)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
