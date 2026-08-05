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
use crate::{BranchName, Repo, SnapshotId, TagName};

/// One snapshot in a history listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub hash: SnapshotId,
    pub message: String,
    /// Raw stored timestamp; formatting is the consumer's choice.
    pub created_at: DateTime<Utc>,
    /// Branch the snapshot was recorded on. Display context only — the branch is
    /// deliberately not part of a snapshot's identity.
    pub branch: BranchName,
    /// First parent; `None` for the root snapshot.
    pub parent: Option<SnapshotId>,
    /// Second parent, set on merge commits.
    pub merge_parent: Option<SnapshotId>,
    pub tag: Option<TagName>,
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
    pub name: BranchName,
    /// True when this is the checked-out branch.
    pub is_head: bool,
}

/// What was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Ancestry of the current position on the checked-out branch.
    CurrentBranch { name: BranchName },
    /// Every snapshot recorded on one named branch.
    NamedBranch { name: BranchName },
    /// Every branch.
    All,
}

/// Why a listing came back empty. Each case wants a different message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmptyReason {
    /// The checked-out branch has no snapshots yet.
    UnbornBranch { branch: BranchName },
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
    pub current: Option<SnapshotId>,
    /// Newest first. Empty exactly when `empty` is set.
    pub entries: Vec<Entry>,
    /// Branch names pointing at each snapshot, checked-out branch first. Several
    /// branches can label one snapshot, which is why this is a list.
    pub refs: HashMap<SnapshotId, Vec<BranchRef>>,
    /// Set when there is nothing to show, explaining which case it is.
    pub empty: Option<EmptyReason>,
}

impl History {
    /// Branches pointing at `hash`, or an empty slice.
    pub fn refs_at(&self, hash: &SnapshotId) -> &[BranchRef] {
        self.refs.get(hash).map_or(&[], |v| v.as_slice())
    }
}

/// What to list.
///
/// A struct rather than four positional arguments: `run(&repo, false, 0, None,
/// None)` said nothing at the call site about what any of it meant, and the
/// `limit` in particular was a trap — see [`Options::limit`].
#[derive(Clone, Debug, Default)]
pub struct Options<'a> {
    /// List every branch instead of the ancestry of the current position.
    /// Ignored when `branch` is set.
    pub all: bool,
    /// Narrow to one branch.
    pub branch: Option<&'a BranchName>,
    /// Keep only snapshots that tracked this path.
    pub file: Option<&'a str>,
    /// The newest N entries, or **all of them** when `None` — which is the
    /// default.
    ///
    /// This used to be a bare `usize` that reached SQL as `LIMIT ?`, so the
    /// obvious way to ask for everything — `0` — returned *nothing*, and
    /// `usize::MAX` worked only because the cast to `i64` wrapped to `-1`, which
    /// SQLite happens to read as unlimited. `None` says it outright.
    pub limit: Option<usize>,
}

/// Collect history.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// # let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
/// use velo_core::commands::history;
///
/// // Everything on one branch.
/// let branch = "main".parse()?;
/// let all = history::run(&repo, history::Options {
///     branch: Some(&branch),
///     ..Default::default()
/// })?;
///
/// // The ten most recent, from the current position.
/// let recent = history::run(&repo, history::Options {
///     limit: Some(10),
///     ..Default::default()
/// })?;
/// # let _ = (all, recent);
/// # Ok(()) }
/// ```
pub fn run(repo: &Repo, options: Options<'_>) -> Result<History> {
    let Options {
        all,
        branch: filter_branch,
        file: file_filter,
        limit,
    } = options;
    // SQLite reads a negative limit as "no limit". Stating it here means the
    // unlimited case is deliberate rather than a consequence of a wrapping cast.
    let limit = limit.map_or(-1_i64, |n| n as i64);
    let root = repo.root();
    let conn = repo.conn();

    let position = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let position = position.trim().to_string();
    let current = (!position.is_empty()).then(|| SnapshotId::from_stored(position.clone()));

    let branch = BranchName::from_stored(
        fs::read_to_string(root.join(".velo/HEAD"))
            .unwrap_or_else(|_| "main".into())
            .trim(),
    );

    let scope = match (all, filter_branch) {
        (_, Some(b)) => Scope::NamedBranch { name: b.clone() },
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
        // An absent parent is stored as '', not NULL, so it has to be filtered
        // rather than read as an Option.
        parent: (!parent.is_empty()).then(|| SnapshotId::from_stored(parent)),
        merge_parent: r
            .get::<_, Option<String>>(5)?
            .filter(|s| !s.is_empty())
            .map(SnapshotId::from_stored),
        tag: r.get(6)?,
    })
}

/// Walk back from `tip` through first parents.
fn ancestry_of(conn: &rusqlite::Connection, tip: &str, limit: i64) -> Result<Vec<Entry>> {
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
    let rows = stmt.query_map(params![tip, limit], row_to_entry)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Every snapshot on `branch`, or on all branches when `None`.
///
/// Internal branches — soft-deleted history and stash shelves — are always
/// excluded; they aren't part of the user's history.
fn on_branch(conn: &rusqlite::Connection, branch: Option<&str>, limit: i64) -> Result<Vec<Entry>> {
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
    let rows = stmt.query_map(params![branch.unwrap_or(""), limit], row_to_entry)?;
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
fn branch_refs(conn: &rusqlite::Connection, head: &str) -> HashMap<SnapshotId, Vec<BranchRef>> {
    let mut by_commit: HashMap<SnapshotId, Vec<BranchRef>> = HashMap::new();
    for (name, tip) in crate::commands::all_branch_tips(conn) {
        let is_head = name == head;
        let entry = by_commit.entry(SnapshotId::from_stored(tip)).or_default();
        let r = BranchRef {
            name: BranchName::from_stored(name),
            is_head,
        };
        if is_head {
            entry.insert(0, r);
        } else {
            entry.push(r);
        }
    }
    by_commit
}
