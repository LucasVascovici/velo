//! Retention, compaction, tags and fsck: the store-only maintenance an
//! embedder needs.
//!
//! A second `#[napi] impl Repo` block, kept apart from `repo.rs`. Every call
//! takes the write lock itself inside its job, like the rest of the binding.

use napi_derive::napi;
use velo_core::commands::compact::{self, Protected};
use velo_core::commands::fsck::{self, Cruft};
use velo_core::commands::gc::{self, AutoVacuum, Vacuum};
use velo_core::commands::repack;
use velo_core::commands::retention::{self, Policy, Tier};
use velo_core::commands::tag;
use velo_core::{SnapshotId, TagName};

use crate::repo::{branch_name, snapshot_id, CoreResult, Job, Repo};

/// One retention tier: snapshots older than `olderThanMs` keep one per
/// `keepOnePerMs` bucket.
#[napi(object)]
pub struct TierInput {
    pub older_than_ms: i64,
    pub keep_one_per_ms: i64,
}

#[napi(object)]
pub struct RetentionOptions {
    /// Ascending by age. Absent means the default policy.
    pub tiers: Option<Vec<TierInput>>,
    /// The clock, in epoch milliseconds. Defaults to now.
    pub now_ms: Option<i64>,
    /// Compute the report and write nothing.
    pub dry_run: Option<bool>,
}

/// One range of a branch's first-parent chain, `oldest..=newest`.
#[napi(object)]
pub struct RangeInput {
    pub oldest: String,
    pub newest: String,
}

/// A range that became one snapshot.
#[napi(object)]
pub struct SquashInfo {
    pub into: String,
    /// Every snapshot of the range, oldest first. All stopped existing.
    pub removed: Vec<String>,
}

/// A snapshot re-minted because an ancestor changed.
#[napi(object)]
pub struct RewrittenInfo {
    pub old: String,
    pub new: String,
}

#[napi(object)]
pub struct CompactionInfo {
    pub branch: String,
    pub dry_run: bool,
    pub squashed: Vec<SquashInfo>,
    pub rewritten: Vec<RewrittenInfo>,
}

/// A snapshot retention would not touch.
#[napi(object)]
pub struct ProtectedInfo {
    pub id: String,
    /// `'tagged'`, `'merge'`, `'parent_of_outside'`, `'published'` or `'checked_out'`.
    pub reason: String,
}

#[napi(object)]
pub struct RetentionReport {
    pub branch: String,
    pub examined: u32,
    pub removed: u32,
    pub dry_run: bool,
    pub squashed: Vec<SquashInfo>,
    pub rewritten: Vec<RewrittenInfo>,
    pub protected: Vec<ProtectedInfo>,
}

/// A tag and what it points at.
#[napi(object)]
pub struct TagInfo {
    pub name: String,
    pub snapshot: String,
    /// The tagged snapshot's message, absent once that snapshot is gone.
    pub message: Option<String>,
}

/// One fsck finding: a variant name and a sentence.
#[napi(object)]
pub struct Finding {
    pub kind: String,
    pub detail: String,
}

/// What `vacuum` did.
#[napi(object)]
pub struct VacuumInfo {
    pub bytes_before: f64,
    pub bytes_after: f64,
    /// `"none"`, `"full"` or `"incremental"`.
    pub auto_vacuum: String,
}

/// What `repack` did, or for a dry run would do.
#[napi(object)]
pub struct RepackInfo {
    pub snapshots_converted: f64,
    pub keyframes_kept: f64,
    pub chains_shortened: f64,
    pub rows_before: f64,
    pub rows_after: f64,
    pub bytes_before: f64,
    /// Estimated for a dry run: a lower bound on the saving.
    pub bytes_after: f64,
    pub dry_run: bool,
}

#[napi(object)]
pub struct FsckReport {
    /// Damage. Non-empty means the repository is corrupt.
    pub problems: Vec<Finding>,
    /// Harmless leftovers that `repair` cleans up.
    pub cruft: Vec<Finding>,
    /// What `repair` cleaned up.
    pub repaired: Vec<Finding>,
}

/// The variant name of a `Debug`-printed finding, e.g. `MissingObject`.
fn kind_of(finding: &dyn std::fmt::Debug) -> String {
    let text = format!("{:?}", finding);
    text.split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_string()
}

fn protected_name(why: &Protected) -> String {
    match why {
        Protected::Tagged => "tagged".into(),
        Protected::Merge => "merge".into(),
        Protected::ParentOfOutside => "parent_of_outside".into(),
        Protected::Published => "published".into(),
        Protected::CheckedOut => "checked_out".into(),
        other => other.to_string(),
    }
}

fn squashes(done: Option<&compact::Compaction>) -> (Vec<SquashInfo>, Vec<RewrittenInfo>) {
    let Some(done) = done else {
        return (Vec::new(), Vec::new());
    };
    let squashed = done
        .squashed
        .iter()
        .map(|s| SquashInfo {
            into: s.into.as_str().to_string(),
            removed: s.removed.iter().map(|r| r.as_str().to_string()).collect(),
        })
        .collect();
    let rewritten = done
        .rewritten
        .iter()
        .map(|(old, new)| RewrittenInfo {
            old: old.as_str().to_string(),
            new: new.as_str().to_string(),
        })
        .collect();
    (squashed, rewritten)
}

fn findings(items: &[Cruft], describe: impl Fn(&Cruft) -> String) -> Vec<Finding> {
    items
        .iter()
        .map(|c| Finding {
            kind: kind_of(c),
            detail: describe(c),
        })
        .collect()
}

#[napi]
impl Repo {
    /// Thin `branch` by a retention policy. With `dryRun` nothing is written.
    #[napi(ts_return_type = "Promise<RetentionReport>")]
    pub fn retention(
        &self,
        branch: String,
        options: Option<RetentionOptions>,
    ) -> napi::bindgen_prelude::AsyncTask<Job<RetentionReport>> {
        let RetentionOptions {
            tiers,
            now_ms,
            dry_run,
        } = options.unwrap_or(RetentionOptions {
            tiers: None,
            now_ms: None,
            dry_run: None,
        });
        let dry_run = dry_run.unwrap_or(false);
        self.job(move |repo| {
            let branch = branch_name(&branch)?;
            let policy = match tiers {
                Some(tiers) => Policy {
                    tiers: tiers
                        .into_iter()
                        .map(|t| Tier {
                            older_than_ms: t.older_than_ms,
                            keep_one_per_ms: t.keep_one_per_ms,
                        })
                        .collect(),
                },
                None => Policy::default(),
            };
            let guard = repo.write()?;
            let report = retention::apply(
                &guard,
                &branch,
                &policy,
                retention::Options {
                    now_ms,
                    dry_run,
                    ..Default::default()
                },
            )?;
            drop(guard);
            let (squashed, rewritten) = squashes(report.compaction.as_ref());
            Ok(RetentionReport {
                branch: report.branch.as_str().to_string(),
                examined: report.examined as u32,
                removed: report.removed as u32,
                dry_run,
                squashed,
                rewritten,
                protected: report
                    .protected
                    .iter()
                    .map(|(id, why)| ProtectedInfo {
                        id: id.as_str().to_string(),
                        reason: protected_name(why),
                    })
                    .collect(),
            })
        })
    }

    /// Squash each range of `branch`'s first-parent chain. Afterwards an old id
    /// rejects with `code: 'Compacted'` and an `into` naming its replacement.
    #[napi(ts_return_type = "Promise<CompactionInfo>")]
    pub fn compact(
        &self,
        branch: String,
        ranges: Vec<RangeInput>,
        dry_run: Option<bool>,
    ) -> napi::bindgen_prelude::AsyncTask<Job<CompactionInfo>> {
        let dry_run = dry_run.unwrap_or(false);
        self.job(move |repo| {
            let branch = branch_name(&branch)?;
            let ids: Vec<(SnapshotId, SnapshotId)> = ranges
                .iter()
                .map(|r| Ok((snapshot_id(&r.oldest)?, snapshot_id(&r.newest)?)))
                .collect::<CoreResult<_>>()?;
            let ranges: Vec<compact::Range<'_>> = ids
                .iter()
                .map(|(oldest, newest)| compact::Range { oldest, newest })
                .collect();
            let guard = repo.write()?;
            let done = compact::run(
                &guard,
                &branch,
                &ranges,
                compact::Options {
                    dry_run,
                    ..Default::default()
                },
            )?;
            drop(guard);
            let (squashed, rewritten) = squashes(Some(&done));
            Ok(CompactionInfo {
                branch: done.branch.as_str().to_string(),
                dry_run: done.dry_run,
                squashed,
                rewritten,
            })
        })
    }

    /// Every tag, ordered by name.
    #[napi(ts_return_type = "Promise<Array<TagInfo>>")]
    pub fn tags(&self) -> napi::bindgen_prelude::AsyncTask<Job<Vec<TagInfo>>> {
        self.job(|repo| {
            Ok(tag::list(repo)?
                .into_iter()
                .map(|t| TagInfo {
                    name: t.name.as_str().to_string(),
                    snapshot: t.snapshot.as_str().to_string(),
                    message: t.message,
                })
                .collect())
        })
    }

    /// Tag snapshot `at` (a name, id or prefix), or the current position when
    /// absent, and resolve to the tagged id. Fails if the name is taken unless
    /// `force`.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn create_tag(
        &self,
        name: String,
        at: Option<String>,
        force: Option<bool>,
    ) -> napi::bindgen_prelude::AsyncTask<Job<String>> {
        self.job(move |repo| {
            let name: TagName = name.parse()?;
            let at = at
                .as_deref()
                .map(|spec| velo_core::commands::resolve_snapshot_id(repo, spec))
                .transpose()?;
            let guard = repo.write()?;
            let made = tag::create(&guard, &name, at.as_ref(), force.unwrap_or(false))?;
            Ok(made.snapshot.as_str().to_string())
        })
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub fn delete_tag(&self, name: String) -> napi::bindgen_prelude::AsyncTask<Job<()>> {
        self.job(move |repo| {
            let name: TagName = name.parse()?;
            let guard = repo.write()?;
            tag::delete(&guard, &name)
        })
    }

    /// Give freed pages back to the filesystem. With `full`, a repository
    /// created before 5.3.0 is converted and rewritten, which needs free disk
    /// about the size of `velo.db` and holds the write lock meanwhile.
    #[napi(ts_return_type = "Promise<VacuumInfo>")]
    pub fn vacuum(&self, full: Option<bool>) -> napi::bindgen_prelude::AsyncTask<Job<VacuumInfo>> {
        self.job(move |repo| {
            let guard = repo.write()?;
            let mode = if full.unwrap_or(false) {
                Vacuum::Full
            } else {
                Vacuum::Incremental
            };
            let done = gc::vacuum(&guard, mode)?;
            Ok(VacuumInfo {
                bytes_before: done.bytes_before as f64,
                bytes_after: done.bytes_after as f64,
                auto_vacuum: match done.auto_vacuum {
                    AutoVacuum::None => "none",
                    AutoVacuum::Full => "full",
                    _ => "incremental",
                }
                .to_string(),
            })
        })
    }

    /// Convert stored full snapshots into deltas, a few at a time, so an
    /// existing repository shrinks. With `dryRun` nothing is written and
    /// `bytesAfter` is an estimate.
    #[napi(ts_return_type = "Promise<RepackInfo>")]
    pub fn repack(
        &self,
        dry_run: Option<bool>,
    ) -> napi::bindgen_prelude::AsyncTask<Job<RepackInfo>> {
        self.job(move |repo| {
            let mut options = repack::Options::default();
            options.dry_run = dry_run.unwrap_or(false);
            let done = repack::run(repo, options)?;
            Ok(RepackInfo {
                snapshots_converted: done.snapshots_converted as f64,
                keyframes_kept: done.keyframes_kept as f64,
                chains_shortened: done.chains_shortened as f64,
                rows_before: done.rows_before as f64,
                rows_after: done.rows_after as f64,
                bytes_before: done.bytes_before as f64,
                bytes_after: done.bytes_after as f64,
                dry_run: done.dry_run,
            })
        })
    }

    /// Verify the repository. With `repair`, cruft is cleaned up; damage is
    /// only ever reported.
    #[napi(ts_return_type = "Promise<FsckReport>")]
    pub fn fsck(&self, repair: Option<bool>) -> napi::bindgen_prelude::AsyncTask<Job<FsckReport>> {
        self.job(move |repo| {
            let report = if repair.unwrap_or(false) {
                let guard = repo.write()?;
                fsck::repair(&guard)?
            } else {
                fsck::check(repo)?
            };
            Ok(FsckReport {
                problems: report
                    .problems
                    .iter()
                    .map(|p| Finding {
                        kind: kind_of(p),
                        detail: p.to_string(),
                    })
                    .collect(),
                cruft: findings(&report.cruft, Cruft::describe),
                repaired: findings(&report.repaired, Cruft::describe_repaired),
            })
        })
    }
}
