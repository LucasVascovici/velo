//! Retention: thin a branch's history by a declarative policy.
//!
//! "Keep everything for a day, hourly for a month, then daily." A [`Policy`] is
//! a list of [`Tier`]s; snapshots older than a tier's `older_than_ms` keep at
//! most one per `keep_one_per_ms` bucket. [`apply`] finds the runs of snapshots
//! that share a bucket and hands them, as ranges, to [`compact`] in one call, so
//! the record decided in 14.1 (old id to new id) is produced exactly as for a
//! manual squash.
//!
//! **Store only**: nothing here reads or writes the working tree.
//!
//! # What is never touched
//!
//! Retention never rewrites published history, that is, anything reachable from
//! a remote-tracking ref (pushed or fetched), nor a snapshot that a snapshot
//! outside the branch's chain builds on. Rewriting anything *at or below* such a
//! snapshot would change its id, so only snapshots newer than the newest of them
//! are eligible. Tagged snapshots, merges and the checked-out snapshot are kept
//! as they are and split the runs around them.
//!
//! # Reclaiming space
//!
//! Compaction only removes the snapshot rows. The objects the squashed snapshots
//! alone referenced become unreachable, and [`gc`](crate::commands::gc) reclaims
//! them afterwards.

use std::collections::HashSet;

use rusqlite::params;

use crate::commands::compact::{self, Protected};
use crate::commands::{branch_tip, snapshot_timestamp_ms, MAX_ANCESTRY_DEPTH};
use crate::error::{Error, Result};
use crate::progress::{Cancel, Observer};
use crate::{BranchName, SnapshotId, WriteGuard};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;

/// Snapshots older than `older_than_ms` keep at most one per `keep_one_per_ms`
/// bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier {
    /// The age, in milliseconds, from which this tier applies.
    pub older_than_ms: i64,
    /// The bucket width, in milliseconds. One snapshot survives per bucket.
    pub keep_one_per_ms: i64,
}

/// A retention policy: tiers by ascending age.
///
/// A snapshot falls under the tier with the largest `older_than_ms` that its age
/// reaches. A snapshot younger than every tier is kept untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Strictly ascending by `older_than_ms`, with `keep_one_per_ms` never
    /// decreasing: older history may only be thinner.
    pub tiers: Vec<Tier>,
}

impl Default for Policy {
    /// Everything for a day, then hourly for a month, then daily.
    fn default() -> Self {
        Policy {
            tiers: vec![
                Tier {
                    older_than_ms: DAY_MS,
                    keep_one_per_ms: HOUR_MS,
                },
                Tier {
                    older_than_ms: 30 * DAY_MS,
                    keep_one_per_ms: DAY_MS,
                },
            ],
        }
    }
}

impl Policy {
    fn validate(&self) -> Result<()> {
        let mut previous: Option<Tier> = None;
        for tier in &self.tiers {
            if tier.older_than_ms <= 0 || tier.keep_one_per_ms <= 0 {
                return Err(Error::invalid(
                    "A retention tier needs a positive age and a positive bucket width.",
                ));
            }
            if let Some(prev) = previous {
                if tier.older_than_ms <= prev.older_than_ms {
                    return Err(Error::invalid(
                        "Retention tiers must be strictly ascending by age.",
                    ));
                }
                if tier.keep_one_per_ms < prev.keep_one_per_ms {
                    return Err(Error::invalid(
                        "Older retention tiers may not keep more than newer ones.",
                    ));
                }
            }
            previous = Some(*tier);
        }
        Ok(())
    }

    /// The index of the tier an `age` falls under.
    fn tier_for(&self, age: i64) -> Option<usize> {
        self.tiers.iter().rposition(|t| t.older_than_ms <= age)
    }
}

/// How to apply a policy.
#[derive(Default)]
pub struct Options<'a> {
    /// The clock, in epoch milliseconds. Defaults to the current time; tests
    /// pass it so the same inputs give the same ranges.
    pub now_ms: Option<i64>,
    /// Compute and return the result, and write nothing.
    pub dry_run: bool,
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked while planning and by the compaction, before anything is written.
    pub cancel: Option<&'a Cancel>,
}

/// What a retention pass did, or with `dry_run` what it would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The branch that was thinned.
    pub branch: BranchName,
    /// How many snapshots were eligible: newer than every unrewritable one.
    pub examined: usize,
    /// How many snapshots the history lost, i.e. each squashed run of `n`
    /// snapshots counts `n - 1`.
    pub removed: usize,
    /// The compaction, or `None` when no run needed squashing.
    pub compaction: Option<compact::Compaction>,
    /// Every protected snapshot found on the chain, newest first, and why.
    pub protected: Vec<(SnapshotId, Protected)>,
}

struct Member {
    hash: String,
    created_at_ms: i64,
}

/// Apply `policy` to `branch`.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// # let mut repo = velo_core::Repo::discover(std::path::Path::new("."))?;
/// # let guard = repo.write()?;
/// use velo_core::commands::retention::{self, Policy};
///
/// let branch = "main".parse().unwrap();
/// let report = retention::apply(&guard, &branch, &Policy::default(), Default::default())?;
/// # let _ = report;
/// # Ok(()) }
/// ```
pub fn apply(
    guard: &WriteGuard,
    branch: &BranchName,
    policy: &Policy,
    options: Options<'_>,
) -> Result<Report> {
    policy.validate()?;
    let Options {
        now_ms,
        dry_run,
        observer,
        cancel,
    } = options;
    let now = now_ms.unwrap_or_else(snapshot_timestamp_ms);
    let conn = guard.conn();

    // The first-parent chain on this branch, newest first.
    let mut chain: Vec<Member> = Vec::new();
    let mut cursor = branch_tip(conn, branch.as_str()).unwrap_or_default();
    while !cursor.is_empty() && (chain.len() as i64) < MAX_ANCESTRY_DEPTH {
        Cancel::check(cancel)?;
        let row = conn.query_row(
            "SELECT parent_hash, created_at_ms FROM snapshots WHERE hash = ? AND branch = ?",
            params![cursor, branch],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        );
        let Ok((parent, created_at_ms)) = row else {
            break;
        };
        chain.push(Member {
            hash: std::mem::replace(&mut cursor, parent),
            created_at_ms,
        });
    }

    // Snapshots something outside the chain builds on, trash included.
    let on_chain: HashSet<&str> = chain.iter().map(|m| m.hash.as_str()).collect();
    let mut depended_on: HashSet<String> = HashSet::new();
    for sql in [
        "SELECT hash, parent_hash, merge_parent FROM snapshots",
        "SELECT hash, parent_hash, merge_parent FROM trash",
    ] {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (hash, parent, merge) = row?;
            if on_chain.contains(hash.as_str()) {
                continue;
            }
            for p in [parent, merge] {
                if on_chain.contains(p.as_str()) {
                    depended_on.insert(p);
                }
            }
        }
    }

    let published = compact::published(conn)?;
    let checked_out = compact::checked_out(guard.root());

    // Classify every chain member. `hard` ones cannot be rewritten at all.
    let mut protected: Vec<(SnapshotId, Protected)> = Vec::new();
    let mut soft: Vec<bool> = Vec::with_capacity(chain.len());
    let mut blocker: Option<usize> = None;
    for (i, member) in chain.iter().enumerate() {
        let reason = if published.contains(&member.hash) {
            Some(Protected::Published)
        } else if depended_on.contains(&member.hash) {
            Some(Protected::ParentOfOutside)
        } else {
            compact::protection_in(conn, &checked_out, &published, &member.hash)
        };
        let hard = matches!(
            reason,
            Some(Protected::Published | Protected::ParentOfOutside)
        );
        if hard && blocker.is_none() {
            blocker = Some(i);
        }
        soft.push(reason.is_some());
        if let Some(reason) = reason {
            protected.push((SnapshotId::from_stored(member.hash.as_str()), reason));
        }
    }

    // Only what is newer than the newest unrewritable snapshot may change.
    let eligible = &chain[..blocker.unwrap_or(chain.len())];

    // Runs of consecutive snapshots sharing a bucket, as indices into `chain`.
    let key_of = |i: usize| -> Option<(usize, i64)> {
        if soft[i] {
            return None;
        }
        let member = &chain[i];
        let tier = policy.tier_for(now - member.created_at_ms)?;
        Some((
            tier,
            member
                .created_at_ms
                .div_euclid(policy.tiers[tier].keep_one_per_ms),
        ))
    };
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (newest index, oldest index)
    let mut open: Option<((usize, i64), usize, usize)> = None;
    for i in 0..eligible.len() {
        match (key_of(i), open) {
            (Some(key), Some((open_key, newest, _))) if key == open_key => {
                open = Some((key, newest, i));
            }
            (key, previous) => {
                if let Some((_, newest, oldest)) = previous {
                    if newest != oldest {
                        runs.push((newest, oldest));
                    }
                }
                open = key.map(|k| (k, i, i));
            }
        }
    }
    if let Some((_, newest, oldest)) = open {
        if newest != oldest {
            runs.push((newest, oldest));
        }
    }

    let mut report = Report {
        branch: branch.clone(),
        examined: eligible.len(),
        removed: 0,
        compaction: None,
        protected,
    };
    if runs.is_empty() {
        return Ok(report);
    }

    // Oldest first, as `compact` reports them.
    let ids: Vec<(SnapshotId, SnapshotId)> = runs
        .iter()
        .rev()
        .map(|&(newest, oldest)| {
            (
                SnapshotId::from_stored(chain[oldest].hash.as_str()),
                SnapshotId::from_stored(chain[newest].hash.as_str()),
            )
        })
        .collect();
    let ranges: Vec<compact::Range<'_>> = ids
        .iter()
        .map(|(oldest, newest)| compact::Range { oldest, newest })
        .collect();

    let compaction = compact::run(
        guard,
        branch,
        &ranges,
        compact::Options {
            dry_run,
            observer,
            cancel,
        },
    )?;
    report.removed = compaction
        .squashed
        .iter()
        .map(|s| s.removed.len().saturating_sub(1))
        .sum();
    report.compaction = Some(compaction);
    Ok(report)
}
