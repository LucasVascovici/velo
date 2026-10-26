//! `velody history` — snapshot history, as data.
//!
//! Returns the entries plus the branch refs that decorate them. Which of the
//! full / oneline / graph presentations to use is the consumer's choice, so none
//! of that lives here.

use chrono::{DateTime, Utc};
use std::collections::HashMap;

use rusqlite::{params_from_iter, types::Value};

use crate::db;
use crate::error::Result;
use std::path::Path;

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
    /// Ancestry of one snapshot, wherever its parts were recorded.
    Ancestry { of: SnapshotId },
    /// Every branch.
    All,
}

/// A condition on a snapshot's metadata. Several filters combine with AND.
///
/// Any namespace may be queried, including the reserved `velo` namespace: reads
/// are not restricted, only writes are. `MetaFilter::equals("velo",
/// "author.name", "ada")` therefore finds an author's snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetaFilter<'a> {
    /// `namespace`/`key` is set to exactly `value`.
    Equals {
        namespace: &'a str,
        key: &'a str,
        value: &'a str,
    },
    /// `namespace`/`key` is set, to anything.
    Has { namespace: &'a str, key: &'a str },
}

impl<'a> MetaFilter<'a> {
    /// `namespace`/`key` is set to exactly `value`.
    pub fn equals(namespace: &'a str, key: &'a str, value: &'a str) -> Self {
        MetaFilter::Equals {
            namespace,
            key,
            value,
        }
    }

    /// `namespace`/`key` is set, to anything.
    pub fn has(namespace: &'a str, key: &'a str) -> Self {
        MetaFilter::Has { namespace, key }
    }
}

/// Why a listing came back empty. Each case wants a different message.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmptyReason {
    /// The checked-out branch has no snapshots yet.
    UnbornBranch { branch: BranchName },
    /// The query matched nothing.
    NoSnapshots,
    /// A file filter excluded every snapshot.
    NoSnapshotsTouching { file: String },
    /// A metadata filter excluded every snapshot.
    NoSnapshotsMatching,
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
    /// branches can label one snapshot, which is why this is a list. Remote-
    /// tracking refs follow the local branches, named `remotes/<remote>/<branch>`
    /// and read from `remote_refs`; the prefix tells them apart from local ones.
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
    /// The ancestry of this snapshot, following both parents through merges.
    ///
    /// The scope a timeline wants, and the one neither `branch` nor `all` gives:
    /// `branch` lists snapshots *recorded on* a branch, so a branch's shared
    /// history — recorded on whatever it forked from — is excluded, and the
    /// listing stops at the fork point.
    ///
    /// Takes precedence over `branch` and `all`. With none of the three set, the
    /// ancestry of the current position is used, which needs `.velo/PARENT` and
    /// so is only meaningful for a consumer with a working tree.
    pub from: Option<&'a SnapshotId>,
    /// Keep only snapshots that **changed** something at or under one of these
    /// paths. A file matches exactly; a directory matches everything beneath it.
    ///
    /// Several paths rather than one because a document and its assets are more
    /// than one path, and running the query per path — then merging and
    /// re-sorting — is both slower and easy to get wrong at the limit.
    pub paths: &'a [&'a Path],
    /// Keep only snapshots whose metadata satisfies **every** one of these.
    /// Empty, the default, matches everything.
    ///
    /// Applied in SQL, so `limit` counts matches: the newest 20 snapshots
    /// matching, not whichever of the newest 20 happen to match. Composes with
    /// every scope and with `paths`.
    pub meta: &'a [MetaFilter<'a>],
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
/// # fn main() -> Result<(), velody_core::Error> {
/// # let repo = velody_core::Repo::discover(std::path::Path::new("."))?;
/// use velody_core::commands::history;
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
        from,
        paths: file_filter,
        meta,
        limit,
    } = options;
    // SQLite reads a negative limit as "no limit". Stating it here means the
    // unlimited case is deliberate rather than a consequence of a wrapping cast.
    let limit = limit.map_or(-1_i64, |n| n as i64);
    let conn = repo.conn();

    let current = repo.position();
    let branch = repo.head_branch();
    // With no checked-out position, the default history walks back from the
    // tip of the branch HEAD names. That covers a single-file repository, which
    // has no working tree, and a directory repository written only through the
    // library, which never sets `.velo/PARENT`: there, reporting the branch as
    // unborn contradicted `branches`, which shows its tip. The CLI's own paths
    // always set a position, so they are unaffected.
    let position = match &current {
        Some(p) => p.as_str().to_string(),
        None => repo
            .branch_tip(&branch)?
            .map(|t| t.into_string())
            .unwrap_or_default(),
    };

    let scope = match (from, all, filter_branch) {
        (Some(id), _, _) => Scope::Ancestry { of: id.clone() },
        (None, _, Some(b)) => Scope::NamedBranch { name: b.clone() },
        (None, true, None) => Scope::All,
        (None, false, None) => Scope::CurrentBranch {
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

    // With a path filter the limit has to be applied *after* filtering, so the
    // query runs unbounded and the truncation happens below. Asking for the
    // newest 20 and then filtering returns whichever of those 20 happened to
    // match — not the newest 20 matches, which is what was asked for.
    let query_limit = if file_filter.is_empty() { limit } else { -1 };

    let mut entries = match &scope {
        Scope::Ancestry { of } => ancestry_of(conn, of.as_str(), query_limit, meta)?,
        Scope::CurrentBranch { .. } => ancestry_of(conn, &position, query_limit, meta)?,
        Scope::NamedBranch { name } => on_branch(conn, Some(name.as_str()), query_limit, meta)?,
        Scope::All => on_branch(conn, None, query_limit, meta)?,
    };

    let mut empty = None;
    if !file_filter.is_empty() {
        // Each requested path stands for every name it has had. Without this the
        // filter is a filter on a *name*, so a listing for a renamed file stops
        // dead at the move — the snapshots before it changed a path that, by
        // then, nothing was called.
        //
        // Anchored at the newest entry in scope, because that is the frame the
        // caller's path is a name in. With nothing in scope there is nothing to
        // filter, so the plain name is enough. Under `all` the newest entry may
        // be on an unrelated branch, and then the aliases are that branch's —
        // the cost of a scope that deliberately has no single line of history.
        let anchor = entries.first().map(|e| e.hash.clone());
        let mut normalised: Vec<String> = Vec::new();
        for path in file_filter {
            match &anchor {
                Some(from) => {
                    normalised.extend(crate::commands::paths::alias_strings(repo, path, from)?)
                }
                None => normalised.push(db::normalise(&path.to_string_lossy())),
            }
        }
        // Two requested paths can share an alias, and checking one twice per
        // snapshot is pure waste. Sorted first because `dedup` only removes
        // neighbours.
        normalised.sort();
        normalised.dedup();
        // A snapshot qualifies if it changed *any* of the paths, so one pass over
        // the entries answers for all of them.
        let lookups: Vec<PathLookup> = normalised
            .iter()
            .map(|path| PathLookup::load(conn, path))
            .collect();
        let mut kept = Vec::with_capacity(entries.len());
        for e in entries {
            let parent = e.parent.as_deref().unwrap_or("");
            let mut any = false;
            for lookup in &lookups {
                if lookup.changed(conn, e.hash.as_str(), parent)? {
                    any = true;
                    break;
                }
            }
            if any {
                kept.push(e);
            }
        }
        entries = kept;
        if limit >= 0 {
            entries.truncate(limit as usize);
        }
        if entries.is_empty() {
            empty = Some(EmptyReason::NoSnapshotsTouching {
                // The name the caller asked about, not every alias of it: a
                // consumer saying "no snapshots touched X" should say X.
                file: file_filter
                    .iter()
                    .map(|p| db::normalise(&p.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
    }
    if entries.is_empty() && empty.is_none() {
        empty = Some(if meta.is_empty() {
            EmptyReason::NoSnapshots
        } else {
            EmptyReason::NoSnapshotsMatching
        });
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

/// One snapshot by id, without walking history.
pub(crate) fn snapshot(repo: &Repo, id: &SnapshotId) -> Result<Entry> {
    let sql = format!(
        "SELECT {COLUMNS}
         FROM snapshots s
         LEFT JOIN tags t ON s.hash = t.snapshot_hash
         WHERE s.hash = ?"
    );
    repo.conn()
        .query_row(&sql, [id], row_to_entry)
        .map_err(
            |_| match crate::commands::compacted_into(repo.conn(), id.as_str()) {
                Some(into) => crate::error::VelodyError::Compacted {
                    id: id.to_string(),
                    into,
                },
                None => crate::error::VelodyError::not_found(
                    crate::error::RefKind::Snapshot,
                    id.as_str(),
                ),
            },
        )
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

/// Every snapshot reachable from `tip`, newest first.
///
/// Follows **both** parents, so the branch a merge absorbed appears in the
/// history of the branch that absorbed it — as it does in the graph, and as
/// `git log` does. Walking first parents alone hid it: `velody history` on a branch
/// that had merged another showed none of the merged work.
///
/// Ordered by time rather than by walk order, because with two parents there is
/// no single chain to follow.
fn ancestry_of(
    conn: &rusqlite::Connection,
    tip: &str,
    limit: i64,
    meta: &[MetaFilter<'_>],
) -> Result<Vec<Entry>> {
    let mut args = vec![Value::from(tip.to_string()), Value::from(limit)];
    let meta_sql = meta_clauses(meta, &mut args);
    let mut stmt = conn.prepare(&format!(
        "WITH RECURSIVE anc(hash, parent_hash, merge_parent, depth) AS (
             SELECT hash, parent_hash, merge_parent, 0
               FROM snapshots WHERE hash = ?1
             UNION
             SELECT s.hash, s.parent_hash, s.merge_parent, a.depth + 1
               FROM snapshots s JOIN anc a
                 ON s.hash = a.parent_hash
                 OR (a.merge_parent <> '' AND s.hash = a.merge_parent)
              WHERE a.depth < {}
         )
         SELECT {COLUMNS}
         FROM snapshots s
         JOIN (SELECT DISTINCT hash FROM anc) r ON r.hash = s.hash
         LEFT JOIN tags t ON s.hash = t.snapshot_hash
         WHERE 1 = 1{meta_sql}
         ORDER BY s.created_at_ms DESC, s.rowid DESC
         LIMIT ?2",
        crate::commands::MAX_ANCESTRY_DEPTH
    ))?;
    let rows = stmt.query_map(params_from_iter(args), row_to_entry)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Every snapshot on `branch`, or on all branches when `None`.
///
/// Internal branches — soft-deleted history and stash shelves — are always
/// excluded; they aren't part of the user's history.
fn on_branch(
    conn: &rusqlite::Connection,
    branch: Option<&str>,
    limit: i64,
    meta: &[MetaFilter<'_>],
) -> Result<Vec<Entry>> {
    let mut args = vec![
        Value::from(branch.unwrap_or("").to_string()),
        Value::from(limit),
    ];
    let meta_sql = meta_clauses(meta, &mut args);
    let sql = format!(
        "SELECT {COLUMNS}
         FROM snapshots s
         LEFT JOIN tags t ON s.hash = t.snapshot_hash
         WHERE {}
           AND s.branch NOT LIKE '_deleted_%'
           AND s.branch NOT LIKE '_stash%'{meta_sql}
         ORDER BY s.created_at_ms DESC, s.rowid DESC LIMIT ?2",
        if branch.is_some() {
            "s.branch = ?1"
        } else {
            "?1 IS NOT NULL"
        }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), row_to_entry)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// One `AND EXISTS (...)` clause per filter, with its values appended to `args`.
///
/// Numbered from `?3`, after the two parameters every query already has. Values
/// are bound, never spliced into the SQL.
fn meta_clauses(meta: &[MetaFilter<'_>], args: &mut Vec<Value>) -> String {
    let mut sql = String::new();
    for filter in meta {
        let (namespace, key, value) = match *filter {
            MetaFilter::Equals {
                namespace,
                key,
                value,
            } => (namespace, key, Some(value)),
            MetaFilter::Has { namespace, key } => (namespace, key, None),
        };
        args.push(Value::from(namespace.to_string()));
        args.push(Value::from(key.to_string()));
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM snapshot_meta m WHERE m.snapshot_id = s.hash \
             AND m.namespace = ?{} AND m.key = ?{}",
            args.len() - 1,
            args.len()
        ));
        if let Some(value) = value {
            args.push(Value::from(value.to_string()));
            sql.push_str(&format!(" AND m.value = ?{}", args.len()));
        }
        sql.push(')');
    }
    sql
}

/// How one requested path answers "did this snapshot change it".
///
/// A file is answered from the path-keyed versions, one lookup for every
/// snapshot that holds it. A directory, or a path no snapshot ever tracked,
/// keeps the prefix query.
enum PathLookup {
    File(crate::tree_store::PathVersions),
    Prefix(String),
}

impl PathLookup {
    fn load(conn: &rusqlite::Connection, path: &str) -> Self {
        use crate::tree_store::PathVersions;
        if !PathVersions::is_directory(conn, path).unwrap_or(false) {
            if let Some(versions) = PathVersions::load(conn, path)
                .ok()
                .filter(|v| !v.is_empty())
            {
                return PathLookup::File(versions);
            }
        }
        PathLookup::Prefix(path.to_string())
    }

    fn changed(&self, conn: &rusqlite::Connection, snapshot: &str, parent: &str) -> Result<bool> {
        match self {
            PathLookup::File(versions) => {
                // A root snapshot has no parent, so the parent side is empty.
                let before = if parent.is_empty() {
                    None
                } else {
                    versions.at(parent)?
                };
                Ok(versions.at(snapshot)? != before)
            }
            PathLookup::Prefix(path) => {
                crate::tree_store::changed_under(conn, snapshot, parent, path)
            }
        }
    }
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
    // Remote-tracking tips come from `remote_refs`, never from the label on a
    // snapshot, which records only where a fetched snapshot arrived.
    if let Ok(mut stmt) =
        conn.prepare("SELECT remote, branch, hash FROM remote_refs ORDER BY remote, branch")
    {
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        for (remote, branch, hash) in rows {
            let exists = conn
                .query_row(
                    "SELECT 1 FROM snapshots WHERE hash = ?",
                    [&hash],
                    |_| Ok(()),
                )
                .is_ok();
            if exists {
                by_commit
                    .entry(SnapshotId::from_stored(hash))
                    .or_default()
                    .push(BranchRef {
                        name: BranchName::from_stored(format!("remotes/{remote}/{branch}")),
                        is_head: false,
                    });
            }
        }
    }
    by_commit
}
