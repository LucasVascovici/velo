//! Conflict inspection and resolution — the data layer.
//!
//! Core exposes conflicts as data and applies decisions; it never prompts. The
//! interactive hunk-by-hunk navigator lives in `velo-tui`, which drives the
//! functions here. Non-interactive resolution (`--take ours|theirs|both`) needs no
//! UI and is served by [`take_side`]; accepting a file edited by hand is served
//! by [`mark`].

use rusqlite::params;
use velo_merge::{build_resolved_content, compute_conflict_hunks, ConflictHunk, Decision};

use crate::commands::require_working_tree;
use crate::db;
use crate::error::{InProgress, RefKind, Result, VeloError};
use crate::{Repo, WriteGuard};

/// Which side to take when resolving without per-hunk decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TakeOption {
    Ours,
    Theirs,
    /// Ours then theirs, per hunk: the same order as the TUI's key `3`.
    Both,
}

impl TakeOption {
    /// The equivalent per-hunk decision.
    pub fn decision(self) -> Decision {
        match self {
            TakeOption::Ours => Decision::Ours,
            TakeOption::Theirs => Decision::Theirs,
            TakeOption::Both => Decision::BothOursFirst,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TakeOption::Ours => "ours",
            TakeOption::Theirs => "theirs",
            TakeOption::Both => "both",
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
    let objects = repo.objects();
    let ancestor = read_text(&objects, &file.ancestor_hash)?;
    let ours = read_text(&objects, &file.our_hash)?;
    let theirs = read_text(&objects, &file.their_hash)?;

    let mut hunks = compute_conflict_hunks(&ancestor, &ours, &theirs);
    if hunks.is_empty() {
        // The conflict was raised by a merge driver for something diff3 sees as
        // clean; give the session the whole file to decide.
        hunks = velo_merge::whole_file_conflict(&ancestor, &ours, &theirs);
    }
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
    require_conflict_row(guard, path)?;
    let (kind, manual) = decision_to_db(decision);
    guard.conn().execute(
        "INSERT OR REPLACE INTO hunk_decisions (file_path, hunk_id, decision, manual_content)
         VALUES (?, ?, ?, ?)",
        params![path, hunk_id as i64, kind, manual],
    )?;
    Ok(())
}

/// The stored conflict row for `path`, if any, read under the guard.
///
/// An interactive session holds the lock only per decision, so a merge abort
/// can land between two decisions; every write re-checks the row first.
fn conflict_row(guard: &WriteGuard, path: &str) -> Option<ConflictFile> {
    guard
        .conn()
        .query_row(
            "SELECT path, ancestor_hash, our_hash, their_hash FROM conflict_files WHERE path = ?",
            [path],
            |r| {
                Ok(ConflictFile {
                    path: r.get(0)?,
                    ancestor_hash: r.get(1)?,
                    our_hash: r.get(2)?,
                    their_hash: r.get(3)?,
                })
            },
        )
        .ok()
}

fn require_conflict_row(guard: &WriteGuard, path: &str) -> Result<ConflictFile> {
    conflict_row(guard, path).ok_or_else(|| VeloError::not_found(RefKind::Path, path))
}

/// Forget one hunk's decision.
pub fn clear_decision(guard: &WriteGuard, path: &str, hunk_id: usize) -> Result<()> {
    require_conflict_row(guard, path)?;
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
    require_working_tree(guard.repo(), "resolve")?;
    let path = &session.file.path;
    match conflict_row(guard, path) {
        None => {
            return Err(if guard.root().join(".velo/MERGE_HEAD").exists() {
                VeloError::not_found(RefKind::Path, path.as_str())
            } else {
                VeloError::NoOperationInProgress {
                    what: InProgress::Merge,
                }
            });
        }
        Some(row)
            if row.ancestor_hash != session.file.ancestor_hash
                || row.our_hash != session.file.our_hash
                || row.their_hash != session.file.their_hash =>
        {
            return Err(VeloError::invalid(format!(
                "the conflict in '{path}' changed since it was opened; open it again"
            )));
        }
        Some(_) => {}
    }
    let anc: Vec<&str> = session.ancestor.lines().collect();
    let our: Vec<&str> = session.ours.lines().collect();
    let thr: Vec<&str> = session.theirs.lines().collect();

    // Ours has no text on a modify/delete conflict; take the newline habit from
    // whichever side does, but never turn "nothing" into a lone newline.
    let trailing_newline = if session.ours.is_empty() {
        session.theirs.ends_with('\n')
    } else {
        session.ours.ends_with('\n')
    };
    let mut resolved = build_resolved_content(&anc, &our, &thr, &session.hunks, false);
    if trailing_newline && !resolved.is_empty() && !resolved.ends_with('\n') {
        resolved.push('\n');
    }
    let full = crate::safe_path::join_checked(guard.root(), &session.file.path)?;
    // A modify/delete conflict has an absent side; resolving to nothing on that
    // side means the file stays deleted, not that it becomes a zero-byte file.
    let absent = session.file.our_hash.is_empty() || session.file.their_hash.is_empty();
    if resolved.is_empty() && absent {
        if full.exists() {
            std::fs::remove_file(&full)?;
        }
        if let Some(parent) = full.parent() {
            crate::commands::remove_empty_parents(parent, guard.root());
        }
    } else {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The store keeps text LF-normalised, so `resolved` is LF. A working
        // file that is consistently CRLF gets its endings back; anything else
        // (LF, mixed, absent) is written as LF, as before.
        let crlf = std::fs::read(&full).is_ok_and(|b| uses_crlf(&b));
        if crlf {
            std::fs::write(&full, resolved.replace('\n', "\r\n"))?;
        } else {
            std::fs::write(&full, &resolved)?;
        }
    }
    clear_conflict(guard, &session.file.path)
}

/// Whether a working file is consistently CRLF: text (no NUL), at least one
/// `\r\n`, and no `\n` that is not preceded by `\r`.
///
/// Mixed-ending files return false on purpose; rewriting them uniformly would
/// change lines the user never touched.
pub(crate) fn uses_crlf(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    let mut seen = false;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            if i == 0 || bytes[i - 1] != b'\r' {
                return false;
            }
            seen = true;
        }
    }
    seen
}

/// What to do when the working copy of a conflicted file holds edits made by
/// hand since the merge wrote it.
///
/// An enum rather than a bool so a call site reads as a decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnEdits {
    /// Refuse with [`VeloError::WouldOverwriteEdits`]; nothing is written.
    #[default]
    Refuse,
    /// Overwrite the edits with the chosen side.
    Replace,
}

/// What [`take_all`] did to one file.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Taken {
    pub path: String,
    pub side: TakeOption,
    /// Lines of hand edits that `Replace` overwrote; 0 when there were none.
    pub replaced_edit_lines: usize,
    /// Lines of the losing side's conflicting hunks that were discarded.
    pub dropped_lines: usize,
    /// Conflict hunks the take decided.
    pub conflicts: usize,
    /// Lines of the other side's clean (non-conflicting) changes that the
    /// result keeps; 0 for a whole-file driver conflict.
    pub kept_clean_lines: usize,
}

/// Has the working copy of `file` been edited by hand?
///
/// A merge leaves a conflicted file on disk equal to "ours", with no markers,
/// so a working file that matches neither side was changed by the user.
/// Returns the number of inserted plus deleted lines relative to "ours", or
/// `None` when the file matches either side.
pub fn edits(repo: &Repo, file: &ConflictFile) -> Result<Option<usize>> {
    let (full, hash) = working_hash(repo, file);
    if hash == file.our_hash || hash == file.their_hash {
        return Ok(None);
    }
    let ours = read_text(&repo.objects(), &file.our_hash)?;
    let working = read_working_lf(&full);
    let n = similar::TextDiff::from_lines(&ours, &working)
        .iter_all_changes()
        .filter(|c| c.tag() != similar::ChangeTag::Equal)
        .count();
    Ok(Some(n))
}

/// The working file's path and content hash; the hash is empty when missing.
fn working_hash(repo: &Repo, file: &ConflictFile) -> (std::path::PathBuf, String) {
    let full = repo.root().join(db::db_to_path(&file.path));
    let hash = if full.exists() {
        crate::storage::hash_for(&full, crate::storage::capture_mode(&full))
    } else {
        String::new()
    };
    (full, hash)
}

/// The working file as text with CR removed (binary is left as is), so it
/// compares against the LF-stored objects line for line.
fn read_working_lf(full: &std::path::Path) -> String {
    match std::fs::read(full) {
        Ok(bytes) => String::from_utf8_lossy(&crate::storage::normalise_crlf(bytes)).into_owned(),
        Err(_) => String::new(),
    }
}

/// Inserted plus deleted lines between two texts.
fn changed_lines(a: &str, b: &str) -> usize {
    similar::TextDiff::from_lines(a, b)
        .iter_all_changes()
        .filter(|c| c.tag() != similar::ChangeTag::Equal)
        .count()
}

/// What [`mark`] accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Marked {
    pub path: String,
    /// Lines in the accepted working file.
    pub lines: usize,
    /// Inserted plus deleted lines relative to ours.
    pub differs_from_ours: usize,
    /// Inserted plus deleted lines relative to theirs.
    pub differs_from_theirs: usize,
    /// Clean lines from the other side that `Drop` accepted losing; empty otherwise.
    pub dropped_clean_changes: Vec<String>,
}

/// What to do when a hand edit lacks lines the other side changed cleanly,
/// outside every conflict hunk.
///
/// An enum rather than a bool so a call site reads as a decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnCleanChanges {
    /// Refuse with [`VeloError::MissingCleanChanges`]; nothing is cleared.
    #[default]
    Refuse,
    /// Accept the edit and report the lines it drops.
    Drop,
}

/// The lines the other side changed cleanly that the working file lacks.
///
/// Compared by exact line as a multiset: a line counts as present when it
/// occurs in the working file at least as often as across all clean changes,
/// so a line moved on purpose is accepted. Whitespace-only lines are skipped.
/// Empty for a whole-file (driver) conflict, which has no clean regions.
pub fn missing_clean_changes(repo: &Repo, file: &ConflictFile) -> Result<Vec<String>> {
    let objects = repo.objects();
    let ancestor = read_text(&objects, &file.ancestor_hash)?;
    let ours = read_text(&objects, &file.our_hash)?;
    let theirs = read_text(&objects, &file.their_hash)?;
    if compute_conflict_hunks(&ancestor, &ours, &theirs).is_empty() {
        return Ok(Vec::new());
    }
    let full = repo.root().join(db::db_to_path(&file.path));
    let working = match std::fs::read(&full) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    };
    let mut available: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for l in working.lines() {
        *available.entry(l).or_insert(0) += 1;
    }
    let mut missing = Vec::new();
    for change in velo_merge::clean_changes_theirs(&ancestor, &ours, &theirs) {
        for line in change.lines {
            if line.trim().is_empty() {
                continue;
            }
            match available.get_mut(line.as_str()) {
                Some(n) if *n > 0 => *n -= 1,
                _ => missing.push(line),
            }
        }
    }
    Ok(missing)
}

/// Accept the working file, edited by hand, as the resolution of `file`.
///
/// Refuses when the file is missing, still equals either side untouched (that
/// is what `--take` is for), or still holds conflict markers. The working file
/// is never written, and `MERGE_HEAD` stays until `save`.
pub fn mark(guard: &WriteGuard, file: &ConflictFile) -> Result<Marked> {
    mark_with(guard, file, OnCleanChanges::Refuse)
}

/// [`mark`], with a choice for a hand edit that drops clean changes from the
/// other side. The clean-change check runs after the other refusals.
pub fn mark_with(
    guard: &WriteGuard,
    file: &ConflictFile,
    on_clean: OnCleanChanges,
) -> Result<Marked> {
    let repo = guard.repo();
    require_working_tree(repo, "resolve")?;
    let (full, hash) = working_hash(repo, file);
    let path = &file.path;
    if hash.is_empty() {
        return Err(VeloError::invalid(format!(
            "'{path}' is missing; restore it or use --take"
        )));
    }
    if hash == file.our_hash {
        return Err(VeloError::invalid(format!(
            "'{path}' is still your saved version — nothing from the other side is in it. Edit it, or use --take ours"
        )));
    }
    if hash == file.their_hash {
        return Err(VeloError::invalid(format!(
            "'{path}' is exactly the incoming version; use --take theirs"
        )));
    }
    let working = match std::fs::read(&full) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    };
    if working
        .lines()
        .any(|l| l.starts_with("<<<<<<<") || l.starts_with(">>>>>>>"))
    {
        return Err(VeloError::invalid(format!(
            "'{path}' still contains conflict markers"
        )));
    }
    let missing = missing_clean_changes(repo, file)?;
    let mut dropped_clean_changes = Vec::new();
    if !missing.is_empty() {
        match on_clean {
            OnCleanChanges::Refuse => {
                return Err(VeloError::MissingCleanChanges {
                    path: std::path::PathBuf::from(path),
                    missing: missing.len(),
                    sample: missing.into_iter().take(5).collect(),
                });
            }
            OnCleanChanges::Drop => dropped_clean_changes = missing,
        }
    }
    let ours = read_text(&repo.objects(), &file.our_hash)?;
    let theirs = read_text(&repo.objects(), &file.their_hash)?;
    let working_lf = read_working_lf(&full);
    let marked = Marked {
        path: file.path.clone(),
        lines: working.lines().count(),
        differs_from_ours: changed_lines(&ours, &working_lf),
        differs_from_theirs: changed_lines(&theirs, &working_lf),
        dropped_clean_changes,
    };
    clear_conflict(guard, &file.path)?;
    Ok(marked)
}

/// The source of the merge in progress (`feature`, `cherry-pick/<snap>`, ...).
///
/// `.velo/MERGE_HEAD` holds `pre_merge_hash:source`.
pub fn merge_source(repo: &Repo) -> Option<String> {
    let text = std::fs::read_to_string(repo.root().join(".velo/MERGE_HEAD")).ok()?;
    let (_, source) = text.split_once(':')?;
    let source = source.trim();
    (!source.is_empty()).then(|| source.to_string())
}

/// Resolve several files by taking one side of each.
///
/// Every file is checked for hand edits first; with [`OnEdits::Refuse`] one
/// edited file fails the whole call before anything is written, so a refusal
/// never leaves the set half-resolved.
pub fn take_all(
    guard: &WriteGuard,
    files: &[ConflictFile],
    side: TakeOption,
    on_edits: OnEdits,
) -> Result<Vec<Taken>> {
    require_working_tree(guard.repo(), "resolve")?;
    let mut edited: Vec<Option<usize>> = Vec::with_capacity(files.len());
    for f in files {
        edited.push(edits(guard.repo(), f)?);
    }
    if on_edits == OnEdits::Refuse && edited.iter().any(Option::is_some) {
        let mut paths: Vec<std::path::PathBuf> = files
            .iter()
            .zip(&edited)
            .filter(|(_, e)| e.is_some())
            .map(|(f, _)| db::db_to_path(&f.path))
            .collect();
        paths.sort();
        return Err(VeloError::WouldOverwriteEdits { paths });
    }

    let mut out = Vec::with_capacity(files.len());
    for (file, edit) in files.iter().zip(edited) {
        let mut session = open_session(guard.repo(), file.clone())?;
        let decision = side.decision();
        let dropped_lines = session
            .hunks
            .iter()
            .map(|h| match side {
                TakeOption::Ours => h.theirs.len(),
                TakeOption::Theirs => h.ours.len(),
                TakeOption::Both => 0,
            })
            .sum();
        let conflicts = session.hunks.len();
        let kept_clean_lines = if conflicts == 0 {
            0
        } else {
            let objects = guard.repo().objects();
            let ancestor = read_text(&objects, &file.ancestor_hash)?;
            let ours = read_text(&objects, &file.our_hash)?;
            let theirs = read_text(&objects, &file.their_hash)?;
            let changes = match side {
                TakeOption::Theirs => velo_merge::clean_changes_theirs(&ancestor, &theirs, &ours),
                _ => velo_merge::clean_changes_theirs(&ancestor, &ours, &theirs),
            };
            changes.iter().map(|c| c.lines.len()).sum()
        };
        for h in &mut session.hunks {
            h.decision = Some(decision.clone());
        }
        for h in &session.hunks {
            record_decision(guard, &file.path, h.id, &decision)?;
        }
        finalise(guard, &session)?;
        out.push(Taken {
            path: file.path.clone(),
            side,
            replaced_edit_lines: edit.unwrap_or(0),
            dropped_lines,
            conflicts,
            kept_clean_lines,
        });
    }
    Ok(out)
}

/// Resolve a whole file by taking one side, with no per-hunk interaction.
pub fn take_side(
    guard: &WriteGuard,
    file: &ConflictFile,
    side: TakeOption,
    on_edits: OnEdits,
) -> Result<Taken> {
    let mut taken = take_all(guard, std::slice::from_ref(file), side, on_edits)?;
    Ok(taken.remove(0))
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

pub(crate) fn decision_to_db(d: &Decision) -> (&'static str, Option<String>) {
    match d {
        Decision::Ours => ("ours", None),
        Decision::Theirs => ("theirs", None),
        Decision::BothOursFirst => ("both_ours", None),
        Decision::BothTheirsFirst => ("both_theirs", None),
        Decision::Manual(lines) => ("manual", Some(lines.join("\n"))),
    }
}

pub(crate) fn decision_from_db(kind: &str, content: Option<&str>) -> Option<Decision> {
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
pub(crate) fn read_text(objects: &crate::storage::ObjectStore, hash: &str) -> Result<String> {
    if hash.is_empty() {
        return Ok(String::new());
    }
    let bytes = objects.get(hash)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
