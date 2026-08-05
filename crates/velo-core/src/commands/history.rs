//! `velo history` — snapshot history, as data.
//!
//! Returns the entries plus the branch refs that decorate them. Which of the
//! full / oneline / graph presentations to use is the consumer's choice, so none
//! of that lives here.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fs;

use rusqlite::params;

use crate::db;
use crate::error::Result;
use crate::Repo;

/// One snapshot in a history listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub hash: String,
    pub message: String,
    /// Raw stored timestamp; formatting is the consumer's choice.
    pub created_at: DateTime<Utc>,
    /// Branch the snapshot was recorded on. Display context only — the branch is
    /// deliberately not part of a snapshot's identity.
    pub branch: String,
    /// First parent; `None` for the root snapshot.
    pub parent: Option<String>,
    /// Second parent, set on merge commits.
    pub merge_parent: Option<String>,
    pub tag: Option<String>,
}

impl Entry {
    /// True when this snapshot joined two lines of history.
    pub fn is_merge(&self) -> bool {
        self.merge_parent.is_some()
    }
}

/// A branch name pointing at a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRef {
    pub name: String,
    /// True when this is the checked-out branch.
    pub is_head: bool,
}

/// What was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Ancestry of the current position on the checked-out branch.
    CurrentBranch { name: String },
    /// Every snapshot recorded on one named branch.
    NamedBranch { name: String },
    /// Every branch.
    All,
}

/// Why a listing came back empty. Each case wants a different message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmptyReason {
    /// The checked-out branch has no snapshots yet.
    UnbornBranch { branch: String },
    /// The query matched nothing.
    NoSnapshots,
    /// A file filter excluded every snapshot.
    NoSnapshotsTouching { file: String },
}

/// A history listing.
#[derive(Clone, Debug)]
pub struct History {
    pub scope: Scope,
    /// The snapshot the working tree sits on, when there is one.
    pub current: Option<String>,
    /// Newest first. Empty exactly when `empty` is set.
    pub entries: Vec<Entry>,
    /// Branch names pointing at each snapshot, checked-out branch first. Several
    /// branches can label one snapshot, which is why this is a list.
    pub refs: HashMap<String, Vec<BranchRef>>,
    /// Set when there is nothing to show, explaining which case it is.
    pub empty: Option<EmptyReason>,
}

impl History {
    /// Branches pointing at `hash`, or an empty slice.
    pub fn refs_at(&self, hash: &str) -> &[BranchRef] {
        self.refs.get(hash).map_or(&[], |v| v.as_slice())
    }
}

/// Collect history.
///
/// `all` lists every branch; `filter_branch` narrows to one; otherwise the
/// ancestry of the current position is walked. `file_filter` keeps only the
/// snapshots that tracked that path.
pub fn run(
    repo: &Repo,
    all: bool,
    limit: usize,
    filter_branch: Option<&str>,
    file_filter: Option<&str>,
) -> Result<History> {
    let root = repo.root();
    let conn = repo.conn();

    let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let position = position.trim().to_string();
    let current = (!position.is_empty()).then(|| position.clone());

    let branch = fs::read_to_string(root.join(".velo/HEAD"))
        .unwrap_or_else(|_| "main".into())
        .trim()
        .to_string();

    let scope = match (all, filter_branch) {
        (_, Some(b)) => Scope::NamedBranch {
            name: b.to_string(),
        },
        (true, None) => Scope::All,
        (false, None) => Scope::CurrentBranch {
            name: branch.clone(),
        },
    };

    // An unborn branch has no ancestry to walk; the other scopes query directly.
    if matches!(scope, Scope::CurrentBranch { .. }) && position.is_empty() {
        return Ok(History {
            scope,
            current,
            entries: Vec::new(),
            refs: HashMap::new(),
            empty: Some(EmptyReason::UnbornBranch { branch }),
        });
    }

    let mut entries = match &scope {
        Scope::CurrentBranch { .. } => ancestry_of(conn, &position, limit)?,
        Scope::NamedBranch { name } => on_branch(conn, Some(name.as_str()), limit)?,
        Scope::All => on_branch(conn, None, limit)?,
    };

    let mut empty = None;
    if let Some(file) = file_filter {
        let normalised = db::normalise(file);
        entries.retain(|e| touched(conn, &e.hash, &normalised));
        if entries.is_empty() {
            empty = Some(EmptyReason::NoSnapshotsTouching {
                file: file.to_string(),
            });
        }
    }
    if entries.is_empty() && empty.is_none() {
        empty = Some(EmptyReason::NoSnapshots);
    }

    let refs = if entries.is_empty() {
        HashMap::new()
    } else {
        branch_refs(conn, &branch)
    };

    Ok(History {
        scope,
        current,
        entries,
        refs,
        empty,
    })
}

// ─── Queries ──────────────────────────────────────────────────────────────────

const COLUMNS: &str =
    "s.hash, s.message, s.created_at_ms, s.branch, s.parent_hash, s.merge_parent, t.name";

fn row_to_entry(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    let parent: String = r.get(4)?;
    Ok(Entry {
        hash: r.get(0)?,
        message: r.get(1)?,
        created_at: crate::commands::timestamp_from_ms(r.get(2)?),
        branch: r.get(3)?,
        parent: (!parent.is_empty()).then_some(parent),
        merge_parent: r.get::<_, Option<String>>(5)?.filter(|s| !s.is_empty()),
        tag: r.get(6)?,
    })
}

/// Walk back from `tip` through first parents.
fn ancestry_of(conn: &rusqlite::Connection, tip: &str, limit: usize) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE cte(hash, message, created_at_ms, branch, parent_hash, merge_parent) AS (
            SELECT hash, message, created_at_ms, branch, parent_hash, merge_parent
            FROM snapshots WHERE hash = ?1
            UNION ALL
            SELECT s.hash, s.message, s.created_at_ms, s.branch, s.parent_hash, s.merge_parent
            FROM snapshots s JOIN cte c ON s.hash = c.parent_hash
        )
        SELECT c.hash, c.message, c.created_at_ms, c.branch, c.parent_hash, c.merge_parent, t.name
        FROM cte c
        LEFT JOIN tags t ON c.hash = t.snapshot_hash
        LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![tip, limit as i64], row_to_entry)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Every snapshot on `branch`, or on all branches when `None`.
///
/// Internal branches — soft-deleted history and stash shelves — are always
/// excluded; they aren't part of the user's history.
fn on_branch(
    conn: &rusqlite::Connection,
    branch: Option<&str>,
    limit: usize,
) -> Result<Vec<Entry>> {
    let sql = format!(
        "SELECT {COLUMNS}
         FROM snapshots s
         LEFT JOIN tags t ON s.hash = t.snapshot_hash
         WHERE {}
           AND s.branch NOT LIKE '_deleted_%'
           AND s.branch NOT LIKE '_stash%'
         ORDER BY s.created_at_ms DESC, s.rowid DESC LIMIT ?2",
        if branch.is_some() {
            "s.branch = ?1"
        } else {
            "?1 IS NOT NULL"
        }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![branch.unwrap_or(""), limit as i64], row_to_entry)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn touched(conn: &rusqlite::Connection, snapshot: &str, path: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ? AND path = ?)",
        params![snapshot, path],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// Which branches point at each snapshot, checked-out branch listed first.
fn branch_refs(conn: &rusqlite::Connection, head: &str) -> HashMap<String, Vec<BranchRef>> {
    let mut by_commit: HashMap<String, Vec<BranchRef>> = HashMap::new();
    for (name, tip) in crate::commands::all_branch_tips(conn) {
        let is_head = name == head;
        let entry = by_commit.entry(tip).or_default();
        let r = BranchRef { name, is_head };
        if is_head {
            entry.insert(0, r);
        } else {
            entry.push(r);
        }
    }
    by_commit
}
