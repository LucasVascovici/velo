use tempfile::TempDir;

use crate::commands::retention::{self, Policy, Tier};
use crate::commands::{fsck, gc, resolve_snapshot_id, tag};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Error, Repo, SnapshotId, SnapshotMeta, TagName};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// A whole number of days, so hour and day buckets line up with it.
const NOW: i64 = 100 * DAY;

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

/// One snapshot on main per timestamp, each with a unique body.
fn history(repo: &Repo, stamps: &[i64]) -> Vec<SnapshotId> {
    let mut ids: Vec<SnapshotId> = Vec::new();
    for (i, ts) in stamps.iter().enumerate() {
        let id = repo
            .write()
            .unwrap()
            .save_tree(SaveTree {
                branch: &main_branch(),
                parent: ids.last(),
                merge_parent: None,
                message: &format!("step {ts}"),
                entries: vec![TreeEntry::file(
                    "f.txt",
                    format!("unique body {i} {ts}\n").into_bytes(),
                )],
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(*ts),
                renames: &[],
            })
            .unwrap();
        ids.push(id);
    }
    ids
}

fn spaced(start: i64, step: i64, n: i64) -> Vec<i64> {
    (0..n).map(|i| start + i * step).collect()
}

fn apply(repo: &Repo, dry_run: bool) -> crate::Result<retention::Report> {
    let guard = repo.write()?;
    retention::apply(
        &guard,
        &main_branch(),
        &Policy::default(),
        retention::Options {
            now_ms: Some(NOW),
            dry_run,
            ..Default::default()
        },
    )
}

fn count(repo: &Repo) -> i64 {
    repo.conn()
        .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn healthy(repo: &Repo) {
    let report = fsck::check(repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

#[test]
fn the_default_policy_is_hourly_then_daily() {
    assert_eq!(
        Policy::default().tiers,
        vec![
            Tier {
                older_than_ms: DAY,
                keep_one_per_ms: HOUR
            },
            Tier {
                older_than_ms: 30 * DAY,
                keep_one_per_ms: DAY
            },
        ]
    );
}

#[test]
fn an_invalid_policy_is_refused() {
    let (_t, repo) = setup();
    let t = |older_than_ms, keep_one_per_ms| Tier {
        older_than_ms,
        keep_one_per_ms,
    };
    for tiers in [
        vec![t(2 * DAY, HOUR), t(DAY, DAY)],
        vec![t(DAY, HOUR), t(DAY, DAY)],
        vec![t(0, HOUR)],
        vec![t(DAY, 0)],
        vec![t(-1, HOUR)],
        vec![t(DAY, DAY), t(2 * DAY, HOUR)],
    ] {
        let guard = repo.write().unwrap();
        let err = retention::apply(
            &guard,
            &main_branch(),
            &Policy { tiers },
            Default::default(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");
    }
}

#[test]
fn hourly_thinning_keeps_one_per_hour() {
    let (_t, repo) = setup();
    history(&repo, &spaced(NOW - 3 * DAY, HOUR / 2, 48));
    let report = apply(&repo, false).unwrap();
    assert_eq!(count(&repo), 24);
    assert_eq!(report.removed, 24);
    assert_eq!(report.examined, 48);
    assert!(report.compaction.is_some());
    healthy(&repo);
}

#[test]
fn recent_snapshots_are_untouched() {
    let (_t, repo) = setup();
    history(&repo, &spaced(NOW - 5 * HOUR, 10 * 60_000, 20));
    let report = apply(&repo, false).unwrap();
    assert_eq!(count(&repo), 20);
    assert_eq!(report.removed, 0);
    assert!(report.compaction.is_none());
}

#[test]
fn a_daily_tier_keeps_one_per_day_beyond_thirty_days() {
    let (_t, repo) = setup();
    history(&repo, &spaced(NOW - 60 * DAY, HOUR, 48));
    let report = apply(&repo, false).unwrap();
    assert_eq!(count(&repo), 2);
    assert_eq!(report.removed, 46);
    healthy(&repo);
}

#[test]
fn a_tag_splits_a_run_and_survives() {
    let (_t, repo) = setup();
    let ids = history(&repo, &spaced(NOW - 3 * DAY, 5 * 60_000, 6));
    {
        let guard = repo.write().unwrap();
        let name: TagName = "v1".parse().unwrap();
        tag::create(&guard, &name, Some(&ids[2]), false).unwrap();
    }
    let report = apply(&repo, false).unwrap();
    // [0,1] squashed, 2 kept, [3,4,5] squashed.
    assert_eq!(count(&repo), 3);
    assert_eq!(report.removed, 3);
    assert!(report
        .protected
        .iter()
        .any(|(id, why)| id == &ids[2] && *why == crate::commands::compact::Protected::Tagged));
    let tags = tag::list(&repo).unwrap();
    assert_eq!(tags.len(), 1);
    let tagged = repo.snapshot(&tags[0].snapshot).unwrap();
    assert_eq!(
        tagged.message,
        format!("step {}", NOW - 3 * DAY + 10 * 60_000)
    );
    healthy(&repo);
}

#[test]
fn a_published_snapshot_blocks_everything_below_it() {
    let (_t, repo) = setup();
    let ids = history(&repo, &spaced(NOW - 3 * DAY, 5 * 60_000, 10));
    repo.conn()
        .execute(
            "INSERT INTO remote_refs (remote, branch, hash) VALUES ('origin', 'main', ?)",
            [&ids[3]],
        )
        .unwrap();
    let report = apply(&repo, false).unwrap();
    // 0..=3 untouched, 4..=9 become one.
    assert_eq!(count(&repo), 5);
    assert_eq!(report.examined, 6);
    assert_eq!(report.removed, 5);
    for id in &ids[..=3] {
        assert!(repo.snapshot(id).is_ok());
    }
    healthy(&repo);
}

#[test]
fn dry_run_changes_nothing_and_reports_the_same() {
    let (_t, repo) = setup();
    history(&repo, &spaced(NOW - 3 * DAY, HOUR / 2, 48));
    let before = count(&repo);
    let dry = apply(&repo, true).unwrap();
    assert_eq!(count(&repo), before);
    let real = apply(&repo, false).unwrap();
    assert_eq!(dry.examined, real.examined);
    assert_eq!(dry.removed, real.removed);
    assert_eq!(dry.protected, real.protected);
    assert_eq!(
        dry.compaction.unwrap().squashed,
        real.compaction.unwrap().squashed
    );
}

#[test]
fn removed_ids_report_compacted() {
    let (_t, repo) = setup();
    let ids = history(&repo, &spaced(NOW - 3 * DAY, 5 * 60_000, 6));
    let report = apply(&repo, false).unwrap();
    let into = report.compaction.unwrap().squashed[0].into.clone();
    match resolve_snapshot_id(&repo, ids[0].as_str()) {
        Err(Error::Compacted { into: got, .. }) => assert_eq!(got, into),
        other => panic!("expected Compacted, got {other:?}"),
    }
}

#[test]
fn gc_after_retention_reclaims_objects() {
    let (tmp, repo) = setup();
    history(&repo, &spaced(NOW - 3 * DAY, 5 * 60_000, 10));
    let objects = || {
        std::fs::read_dir(tmp.path().join(".velo/objects"))
            .unwrap()
            .count()
    };
    let before = objects();
    apply(&repo, false).unwrap();
    {
        let guard = repo.write().unwrap();
        gc::run(
            &guard,
            gc::Options {
                keep_days: 0,
                ..Default::default()
            },
        )
        .unwrap();
    }
    assert!(objects() < before, "{} !< {before}", objects());
    healthy(&repo);
}
