//! Querying snapshots by metadata (`history::Options::meta`, `Repo::find_snapshots`).

use std::path::Path;
use tempfile::TempDir;

use crate::commands::history::{self, EmptyReason, MetaFilter};
use crate::tree::{SaveTree, TreeEntry};
use crate::{Author, BranchName, Error, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

/// Save one snapshot with a single file and the given metadata.
fn save(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    file: &str,
    meta: &[(&str, &str, &str)],
    author: Option<&Author>,
    ts: i64,
) -> SnapshotId {
    let mut m = SnapshotMeta::new();
    for (ns, k, v) in meta {
        m.set(*ns, *k, *v).unwrap();
    }
    let branch: BranchName = branch.parse().unwrap();
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: file,
            entries: vec![TreeEntry::file(file, format!("{file}{ts}").into_bytes())],
            meta: m,
            author,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

fn hashes(entries: &[history::Entry]) -> Vec<SnapshotId> {
    entries.iter().map(|e| e.hash.clone()).collect()
}

/// Six snapshots on main, alternating eval_run 1 and 2, oldest first.
fn alternating(repo: &Repo) -> Vec<SnapshotId> {
    let mut ids: Vec<SnapshotId> = Vec::new();
    for i in 0..6 {
        let run = if i % 2 == 0 { "1" } else { "2" };
        let id = save(
            repo,
            "main",
            ids.last(),
            "f.txt",
            &[("ci", "eval_run", run)],
            None,
            1_000 * (i + 1),
        );
        ids.push(id);
    }
    ids
}

fn run_on_main(repo: &Repo, meta: &[MetaFilter<'_>], limit: Option<usize>) -> history::History {
    let main: BranchName = "main".parse().unwrap();
    history::run(
        repo,
        history::Options {
            branch: Some(&main),
            meta,
            limit,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn equals_on_a_named_branch_returns_only_matches() {
    let (_t, repo) = setup();
    let ids = alternating(&repo);
    let h = run_on_main(&repo, &[MetaFilter::equals("ci", "eval_run", "2")], None);
    assert_eq!(
        hashes(&h.entries),
        vec![ids[5].clone(), ids[3].clone(), ids[1].clone()]
    );
}

#[test]
fn limit_counts_matches_not_candidates() {
    let (_t, repo) = setup();
    let ids = alternating(&repo);
    // The newest snapshot is eval_run 2; a post-filter limit of 2 would
    // return only one entry here.
    let h = run_on_main(&repo, &[MetaFilter::equals("ci", "eval_run", "1")], Some(2));
    assert_eq!(hashes(&h.entries), vec![ids[4].clone(), ids[2].clone()]);
}

#[test]
fn filters_combine_with_and() {
    let (_t, repo) = setup();
    let a = save(
        &repo,
        "main",
        None,
        "a",
        &[("ci", "status", "pass"), ("ci", "run", "1")],
        None,
        1_000,
    );
    let b = save(
        &repo,
        "main",
        Some(&a),
        "b",
        &[("ci", "status", "pass"), ("ci", "run", "2")],
        None,
        2_000,
    );
    let _c = save(
        &repo,
        "main",
        Some(&b),
        "c",
        &[("ci", "status", "fail"), ("ci", "run", "2")],
        None,
        3_000,
    );
    let h = run_on_main(
        &repo,
        &[
            MetaFilter::equals("ci", "status", "pass"),
            MetaFilter::equals("ci", "run", "2"),
        ],
        None,
    );
    assert_eq!(hashes(&h.entries), vec![b]);
}

#[test]
fn has_matches_any_value() {
    let (_t, repo) = setup();
    let a = save(
        &repo,
        "main",
        None,
        "a",
        &[("ci", "status", "pass")],
        None,
        1_000,
    );
    let b = save(
        &repo,
        "main",
        Some(&a),
        "b",
        &[("ci", "status", "fail")],
        None,
        2_000,
    );
    let _c = save(
        &repo,
        "main",
        Some(&b),
        "c",
        &[("ci", "other", "x")],
        None,
        3_000,
    );
    let h = run_on_main(&repo, &[MetaFilter::has("ci", "status")], None);
    assert_eq!(hashes(&h.entries), vec![b, a]);
}

#[test]
fn composes_with_ancestry() {
    let (_t, repo) = setup();
    let m = [("ci", "status", "pass")];
    let root = save(&repo, "main", None, "r", &m, None, 1_000);
    // An ancestor of on_main that carries no metadata must be filtered out.
    let unmarked = save(&repo, "main", Some(&root), "u", &[], None, 1_500);
    let on_main = save(&repo, "main", Some(&unmarked), "m", &m, None, 2_000);
    let on_side = save(&repo, "side", Some(&root), "s", &m, None, 3_000);
    let h = history::run(
        &repo,
        history::Options {
            from: Some(&on_main),
            meta: &[MetaFilter::equals("ci", "status", "pass")],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(hashes(&h.entries), vec![on_main, root]);
    assert!(!hashes(&h.entries).contains(&unmarked));
    assert!(!hashes(&h.entries).contains(&on_side));
}

#[test]
fn composes_with_paths() {
    let (_t, repo) = setup();
    let m = [("ci", "status", "pass")];
    let a = save(&repo, "main", None, "a.txt", &m, None, 1_000);
    let b = save(&repo, "main", Some(&a), "b.txt", &m, None, 2_000);
    // The newest snapshot rewrites b.txt but has no ci/status metadata. It
    // passes the path filter, so only the meta filter can exclude it, and
    // the limit must then be applied after both filters.
    let _newest = save(&repo, "main", Some(&b), "b.txt", &[], None, 3_000);
    let paths = [Path::new("b.txt")];
    let main: BranchName = "main".parse().unwrap();
    let h = history::run(
        &repo,
        history::Options {
            branch: Some(&main),
            paths: &paths,
            meta: &[MetaFilter::has("ci", "status")],
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(hashes(&h.entries), vec![b]);
}

#[test]
fn the_reserved_namespace_can_be_queried() {
    let (_t, repo) = setup();
    let ada = Author::new("ada").unwrap();
    let bob = Author::new("bob").unwrap();
    let a = save(&repo, "main", None, "a", &[], Some(&ada), 1_000);
    let b = save(&repo, "main", Some(&a), "b", &[], Some(&bob), 2_000);
    let c = save(&repo, "main", Some(&b), "c", &[], Some(&ada), 3_000);
    let found = repo
        .find_snapshots(&[MetaFilter::equals("velo", "author.name", "ada")])
        .unwrap();
    assert_eq!(hashes(&found), vec![c, a]);
}

#[test]
fn find_snapshots_spans_branches_and_rejects_no_filters() {
    let (_t, repo) = setup();
    let m = [("ci", "status", "pass")];
    let a = save(&repo, "main", None, "a", &m, None, 1_000);
    let b = save(&repo, "other", None, "b", &m, None, 2_000);
    let _skip = save(&repo, "other", Some(&b), "c", &[], None, 3_000);
    let found = repo
        .find_snapshots(&[MetaFilter::equals("ci", "status", "pass")])
        .unwrap();
    assert_eq!(hashes(&found), vec![b, a]);
    assert!(matches!(
        repo.find_snapshots(&[]),
        Err(Error::InvalidInput { .. })
    ));
}

#[test]
fn reports_when_no_snapshot_matches() {
    let (_t, repo) = setup();
    alternating(&repo);
    let h = run_on_main(&repo, &[MetaFilter::equals("ci", "eval_run", "9")], None);
    assert!(h.entries.is_empty());
    assert_eq!(h.empty, Some(EmptyReason::NoSnapshotsMatching));
}
