//! The audit report model: a repository's provenance as plain data.
//!
//! [`build`] gathers, for every local branch, the timeline of snapshots with
//! their author, metadata, signature status and changed files; a per-file view
//! with line provenance from [`blame`](crate::commands::blame); and a verdict
//! from [`fsck`](crate::commands::fsck) on whether every id recomputes. Core
//! returns the data and a renderer (`velody report`, or an embedder's own) turns
//! it into a page, as with every other command.
//!
//! # Everything in here is attacker-controlled
//!
//! Messages, metadata, author names, branch names, file names and line text all
//! come from whoever wrote the history, which for an agent's repository means
//! the agent, or whatever fed it. Core passes every string through **verbatim**
//! and does no escaping, because the right escaping depends on where the text
//! lands (HTML, JSON, a terminal). Escaping is the renderer's job; a renderer
//! that interpolates these strings into markup unescaped has a script
//! injection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::commands::{blame, fsck, paths};
use crate::error::Result;
use crate::progress::{Cancel, Observer};
use crate::sign::{self, RowCheck, TrustPolicy};
use crate::tree::FileKind;
use crate::{Author, BranchName, Repo, SnapshotId};

/// What to report on, and how much of it.
#[derive(Clone)]
#[non_exhaustive]
pub struct Options<'a> {
    /// Only this branch. `None` is every local branch.
    pub branch: Option<&'a BranchName>,
    /// Only snapshots created at or after this time, in milliseconds since the
    /// epoch.
    pub since_ms: Option<i64>,
    /// Which keys count as trusted. With none, valid signatures are
    /// [`SignatureState::Untrusted`].
    pub trust: Option<&'a dyn TrustPolicy>,
    /// Blame exactly these paths instead of the ones the reported snapshots
    /// changed.
    pub blame_paths: Option<&'a [&'a Path]>,
    /// The most files blamed per branch; the rest are listed in
    /// [`Report::skipped_files`]. Blame walks history per file, so this bounds
    /// the cost of a report on a large repository.
    pub max_blame_files: usize,
    /// Run `fsck` and fill [`Report::integrity`].
    pub fsck: bool,
    /// Where to report blame progress.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between snapshots.
    pub cancel: Option<&'a Cancel>,
}

impl<'a> Options<'a> {
    /// Every branch, 200 blamed files per branch, `fsck` on.
    pub fn new() -> Self {
        Options {
            branch: None,
            since_ms: None,
            trust: None,
            blame_paths: None,
            max_blame_files: 200,
            fsck: true,
            observer: None,
            cancel: None,
        }
    }

    /// Report only `branch`.
    pub fn branch(mut self, branch: &'a BranchName) -> Self {
        self.branch = Some(branch);
        self
    }

    /// Report only snapshots created at or after `since_ms`.
    pub fn since_ms(mut self, since_ms: i64) -> Self {
        self.since_ms = Some(since_ms);
        self
    }

    /// Decide which signing keys are trusted.
    pub fn trust(mut self, trust: &'a dyn TrustPolicy) -> Self {
        self.trust = Some(trust);
        self
    }

    /// Blame exactly these paths.
    pub fn blame_paths(mut self, paths: &'a [&'a Path]) -> Self {
        self.blame_paths = Some(paths);
        self
    }

    /// Blame at most `max` files per branch.
    pub fn max_blame_files(mut self, max: usize) -> Self {
        self.max_blame_files = max;
        self
    }

    /// Run `fsck` (the default) or skip it.
    pub fn fsck(mut self, fsck: bool) -> Self {
        self.fsck = fsck;
        self
    }

    /// Report blame progress to `observer`.
    pub fn observer(mut self, observer: &'a dyn Observer) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Stop early when `cancel` is set.
    pub fn cancel(mut self, cancel: &'a Cancel) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// The whole report.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Report {
    /// One timeline per reported branch, in name order.
    pub branches: Vec<BranchTimeline>,
    /// Line provenance for the blamed files, per branch tip.
    pub files: Vec<FileProvenance>,
    /// Paths left out of [`files`](Report::files): over the cap, not UTF-8, or
    /// symlinks.
    pub skipped_files: Vec<String>,
    /// The `fsck` verdict; `None` when it was not run.
    pub integrity: Option<Integrity>,
}

/// The snapshots recorded on one branch.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BranchTimeline {
    pub branch: BranchName,
    /// The branch's current tip, regardless of the `since_ms` filter.
    pub tip: Option<SnapshotId>,
    /// Newest first.
    pub snapshots: Vec<SnapshotRecord>,
}

/// One snapshot and everything recorded about it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SnapshotRecord {
    pub id: SnapshotId,
    pub parent: Option<SnapshotId>,
    pub merge_parent: Option<SnapshotId>,
    pub created_at_ms: i64,
    pub message: String,
    pub author: Option<Author>,
    /// `(namespace, key, value)` for every namespace, the reserved one included.
    pub meta: Vec<(String, String, String)>,
    /// Empty means unsigned.
    pub signatures: Vec<SignatureStatus>,
    /// Against the first parent; every file for a root snapshot.
    pub changes: Vec<ChangedPath>,
}

/// How a stored signature fared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignatureState {
    /// Valid, by a key the trust policy accepts.
    Trusted,
    /// Valid, by a key the policy does not accept (or no policy was given).
    Untrusted,
    /// The signature does not verify.
    Invalid,
    /// An algorithm this build does not know.
    Unverifiable,
}

/// One signature on a snapshot.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SignatureStatus {
    /// The signing key, as stored.
    pub public_key: String,
    /// The first 16 hex characters of the key, for display.
    pub fingerprint: String,
    pub state: SignatureState,
}

/// What a snapshot did to one path.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathChange {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
}

/// One changed path.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ChangedPath {
    pub path: String,
    pub change: PathChange,
}

/// One file with its lines attributed.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FileProvenance {
    pub path: String,
    /// The branch tip the file was blamed at.
    pub at: SnapshotId,
    pub lines: Vec<LineProvenance>,
}

/// One line and where it came from.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct LineProvenance {
    pub line_no: usize,
    pub text: String,
    /// `None` when blame could not attribute the line.
    pub snapshot: Option<SnapshotId>,
    pub author: Option<Author>,
}

/// The `fsck` verdict.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Integrity {
    pub snapshots_checked: usize,
    /// Snapshots whose id was recomputed from content and matched.
    pub ids_verified: usize,
    pub problems: usize,
    pub healthy: bool,
}

fn is_reserved(name: &str) -> bool {
    name == "_stash" || name.starts_with("_deleted_") || name.starts_with("remotes/")
}

fn opt_id(text: String) -> Option<SnapshotId> {
    (!text.is_empty()).then(|| SnapshotId::from_stored(text))
}

/// Build the report. See the [module docs](self) for the trust model of the
/// strings in it.
pub fn build(repo: &Repo, options: Options<'_>) -> Result<Report> {
    let conn = repo.conn();
    let mut names: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT name, tip FROM branches ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    names.retain(|(n, _)| !is_reserved(n));
    if let Some(only) = options.branch {
        names.retain(|(n, _)| n == only.as_str());
    }

    let mut report = Report {
        branches: Vec::new(),
        files: Vec::new(),
        skipped_files: Vec::new(),
        integrity: None,
    };
    let mut skipped: BTreeSet<String> = BTreeSet::new();

    for (name, tip_col) in names {
        let mut stmt = conn.prepare(
            "SELECT hash, message, parent_hash, merge_parent, created_at_ms
             FROM snapshots WHERE branch = ? AND created_at_ms >= ?
             ORDER BY created_at_ms DESC, rowid DESC",
        )?;
        let rows: Vec<(String, String, String, String, i64)> = stmt
            .query_map(
                rusqlite::params![name, options.since_ms.unwrap_or(i64::MIN)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?
            .collect::<std::result::Result<_, _>>()?;

        let mut tip = opt_id(tip_col);
        if tip.is_none() {
            tip = conn
                .query_row(
                    "SELECT hash FROM snapshots WHERE branch = ?
                     ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
                    [&name],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(opt_id);
        }

        let mut snapshots = Vec::with_capacity(rows.len());
        let mut touched: BTreeSet<String> = BTreeSet::new();
        for (hash, message, parent, merge_parent, created_at_ms) in rows {
            Cancel::check(options.cancel)?;
            let id = SnapshotId::from_stored(hash);
            let parent = opt_id(parent);
            let meta = repo.snapshot_meta(&id)?;
            let changes = changes_of(repo, &id, parent.as_ref())?;
            for c in &changes {
                if c.change != PathChange::Deleted {
                    touched.insert(c.path.clone());
                }
            }
            snapshots.push(SnapshotRecord {
                author: meta.author(),
                meta: meta
                    .iter()
                    .map(|(n, k, v)| (n.to_string(), k.to_string(), v.to_string()))
                    .collect(),
                signatures: signatures_of(repo, &id, options.trust)?,
                id,
                parent,
                merge_parent: opt_id(merge_parent),
                created_at_ms,
                message,
                changes,
            });
        }

        if let Some(tip_id) = &tip {
            blame_group(
                repo,
                &options,
                tip_id,
                &touched,
                &mut report.files,
                &mut skipped,
            )?;
        }

        report.branches.push(BranchTimeline {
            branch: BranchName::from_stored(name),
            tip,
            snapshots,
        });
    }
    report.skipped_files = skipped.into_iter().collect();

    if options.fsck {
        Cancel::check(options.cancel)?;
        let check = fsck::check(repo)?;
        let mut integrity = Integrity {
            snapshots_checked: 0,
            ids_verified: 0,
            problems: check.problems.len(),
            healthy: check.is_healthy(),
        };
        for section in &check.sections {
            if let fsck::Section::Snapshots {
                checked,
                ids_verified,
                ..
            } = section
            {
                integrity.snapshots_checked = *checked;
                integrity.ids_verified = *ids_verified;
            }
        }
        report.integrity = Some(integrity);
    }
    Ok(report)
}

fn signatures_of(
    repo: &Repo,
    id: &SnapshotId,
    trust: Option<&dyn TrustPolicy>,
) -> Result<Vec<SignatureStatus>> {
    Ok(sign::signatures(repo, id)?
        .into_iter()
        .map(|row| {
            let state =
                match sign::check_row(id.as_str(), &row.algorithm, &row.public_key, &row.signature)
                {
                    RowCheck::Valid
                        if trust.is_some_and(|t| t.trusts(&row.algorithm, &row.public_key)) =>
                    {
                        SignatureState::Trusted
                    }
                    RowCheck::Valid => SignatureState::Untrusted,
                    RowCheck::Invalid => SignatureState::Invalid,
                    RowCheck::Unverifiable => SignatureState::Unverifiable,
                };
            SignatureStatus {
                fingerprint: row.public_key.chars().take(16).collect(),
                public_key: row.public_key,
                state,
            }
        })
        .collect())
}

/// The paths `id` changed against its first parent, renames folded in.
fn changes_of(
    repo: &Repo,
    id: &SnapshotId,
    parent: Option<&SnapshotId>,
) -> Result<Vec<ChangedPath>> {
    let key = |f: &crate::tree::TreeFile| (f.object.to_string(), f.kind);
    let new: BTreeMap<String, _> = repo
        .tree_at(id)?
        .into_iter()
        .map(|f| (f.path.clone(), key(&f)))
        .collect();
    // A parent that no longer resolves (pruned or compacted away) is treated as
    // empty rather than failing the whole report.
    let old: BTreeMap<String, _> = match parent {
        Some(p) => repo
            .tree_at(p)
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.path.clone(), key(&f)))
            .collect(),
        None => BTreeMap::new(),
    };

    let mut out: BTreeMap<String, PathChange> = BTreeMap::new();
    for (path, v) in &new {
        match old.get(path) {
            None => {
                out.insert(path.clone(), PathChange::Added);
            }
            Some(o) if o != v => {
                out.insert(path.clone(), PathChange::Modified);
            }
            Some(_) => {}
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            out.insert(path.clone(), PathChange::Deleted);
        }
    }
    for (from, to) in paths::recorded_by(repo, id)? {
        let norm = |p: &Path| crate::db::normalise(&p.to_string_lossy());
        let (from, to) = (norm(&from), norm(&to));
        if out.contains_key(&to) {
            if out.get(&from) == Some(&PathChange::Deleted) {
                out.remove(&from);
            }
            out.insert(to, PathChange::Renamed { from });
        }
    }
    Ok(out
        .into_iter()
        .map(|(path, change)| ChangedPath { path, change })
        .collect())
}

fn blame_group(
    repo: &Repo,
    options: &Options<'_>,
    tip: &SnapshotId,
    touched: &BTreeSet<String>,
    files: &mut Vec<FileProvenance>,
    skipped: &mut BTreeSet<String>,
) -> Result<()> {
    let at_tip: BTreeMap<String, FileKind> = repo
        .tree_at(tip)?
        .into_iter()
        .map(|f| (f.path, f.kind))
        .collect();
    let wanted: BTreeSet<String> = match options.blame_paths {
        Some(list) => list
            .iter()
            .map(|p| crate::db::normalise(&p.to_string_lossy()))
            .collect(),
        None => touched.clone(),
    };
    let mut done = 0usize;
    for path in wanted {
        let Some(kind) = at_tip.get(&path) else {
            continue;
        };
        if done >= options.max_blame_files {
            skipped.insert(path);
            continue;
        }
        Cancel::check(options.cancel)?;
        let text_ok = *kind != FileKind::Symlink
            && repo
                .read_file_at(tip, &path)
                .is_ok_and(|b| std::str::from_utf8(&b).is_ok());
        if !text_ok {
            skipped.insert(path);
            continue;
        }
        done += 1;
        let b = blame::run(
            repo,
            Path::new(&path),
            blame::Options {
                at: Some(tip),
                lines: None,
                observer: options.observer,
                cancel: options.cancel,
            },
        )?;
        files.push(FileProvenance {
            path,
            at: tip.clone(),
            lines: b
                .lines
                .into_iter()
                .map(|l| LineProvenance {
                    line_no: l.line_no,
                    text: l.text,
                    snapshot: l.origin.as_ref().map(|o| o.hash.clone()),
                    author: l.origin.and_then(|o| o.author),
                })
                .collect(),
        });
    }
    Ok(())
}
