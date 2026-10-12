//! `save` never drops a merge parent or a conflict silently.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::resolve::{self, TakeOption};
use crate::commands::{self, blame, merge, save, show, switch};
use crate::error::InProgress;
use crate::{Error, Repo, SnapshotId};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn write(root: &Path, name: &str, body: &str) {
    fs::write(root.join(name), body).unwrap();
}

fn save_as(repo: &Repo, msg: &str, amend: bool) -> crate::error::Result<save::Outcome> {
    let g = repo.write().unwrap();
    save::run(
        &g,
        Some(msg),
        save::Options {
            amend,
            ..Default::default()
        },
    )
}

fn go(repo: &Repo, branch: &str, force: bool) {
    let g = repo.write().unwrap();
    switch::run(&g, branch, force).unwrap();
}

fn tip(repo: &Repo, branch: &str) -> SnapshotId {
    commands::resolve_snapshot_id(repo, branch).unwrap()
}

fn merge_in(repo: &Repo, source: &str) -> merge::Outcome {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source }).unwrap()
}

/// main and feature each add a different file, so merging is clean but needs
/// a real merge snapshot. Leaves the merge pending (unsaved), on main.
fn clean_merge_pending(root: &Path, repo: &Repo) {
    write(root, "base.txt", "base\n");
    save_as(repo, "base", false).unwrap();
    go(repo, "feature", false);
    write(root, "f.txt", "feature line\n");
    save_as(repo, "feature work", false).unwrap();
    go(repo, "main", true);
    write(root, "m.txt", "main line\n");
    save_as(repo, "main work", false).unwrap();
    assert!(matches!(
        merge_in(repo, "feature"),
        merge::Outcome::Merged(_)
    ));
}

/// Both branches edit the same line of one file: an unresolved conflict.
fn conflicted(root: &Path, repo: &Repo) {
    write(root, "c.txt", "one\n");
    save_as(repo, "base", false).unwrap();
    go(repo, "feature", false);
    write(root, "c.txt", "feature\n");
    save_as(repo, "feature edit", false).unwrap();
    go(repo, "main", true);
    write(root, "c.txt", "main\n");
    save_as(repo, "main edit", false).unwrap();
    merge_in(repo, "feature");
    assert!(resolve::conflict_count(repo) > 0);
}

#[test]
fn amending_a_merge_keeps_its_second_parent() {
    let (tmp, repo) = setup();
    clean_merge_pending(tmp.path(), &repo);
    let merged = save_as(&repo, "merge feature", false)
        .unwrap()
        .into_result()
        .unwrap()
        .hash;
    let feature_tip = tip(&repo, "feature");
    let before = show::run(&repo, &merged, &[]).unwrap();
    assert_eq!(before.merge_parent.as_ref(), Some(&feature_tip));

    write(tmp.path(), "extra.txt", "x\n");
    let amended = save_as(&repo, "merge feature, amended", true)
        .unwrap()
        .into_result()
        .unwrap()
        .hash;
    assert_ne!(amended, merged);
    let after = show::run(&repo, &amended, &[]).unwrap();
    assert_eq!(after.merge_parent.as_ref(), Some(&feature_tip));
    assert_eq!(after.parent, before.parent);
}

#[test]
fn amended_merge_still_counts_as_merged_for_merge_base() {
    let (tmp, repo) = setup();
    clean_merge_pending(tmp.path(), &repo);
    save_as(&repo, "merge feature", false).unwrap();
    write(tmp.path(), "extra.txt", "x\n");
    save_as(&repo, "amended", true).unwrap();

    let main_tip = tip(&repo, "main");
    let feature_tip = tip(&repo, "feature");
    assert_eq!(
        merge::merge_base(&repo, &main_tip, &feature_tip).unwrap(),
        Some(feature_tip.clone())
    );
    // `merge` only fast-forwards along first parents, so "merge again" is a
    // three-way run; what matters is that its base is the feature tip and it
    // therefore has nothing to change. A dropped merge parent would instead
    // re-merge the whole feature branch.
    match merge_in(&repo, "feature") {
        merge::Outcome::AlreadyUpToDate { .. } => {}
        merge::Outcome::Merged(m) => {
            assert_eq!(m.ancestor.as_deref(), Some(feature_tip.as_str()));
            assert!(m.files().is_empty(), "{:?}", m.files());
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn blame_at_an_amended_merge_credits_the_feature_snapshot() {
    let (tmp, repo) = setup();
    clean_merge_pending(tmp.path(), &repo);
    save_as(&repo, "merge feature", false).unwrap();
    write(tmp.path(), "extra.txt", "x\n");
    save_as(&repo, "amended", true).unwrap();

    let feature_tip = tip(&repo, "feature");
    let b = blame::run(&repo, Path::new("f.txt"), blame::Options::default()).unwrap();
    let origin = b.lines[0].origin.as_ref().expect("attributed");
    assert_eq!(origin.hash, feature_tip);
}

#[test]
fn amend_during_a_pending_merge_is_refused_and_changes_nothing() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let head = tmp.path().join(".velo/MERGE_HEAD");
    let marker = fs::read_to_string(&head).unwrap();
    let rows = resolve::conflict_count(&repo);

    let err = save_as(&repo, "amend", true).unwrap_err();
    assert!(matches!(
        err,
        Error::OperationInProgress {
            what: InProgress::Merge
        }
    ));
    assert_eq!(fs::read_to_string(&head).unwrap(), marker);
    assert_eq!(resolve::conflict_count(&repo), rows);
}

#[test]
fn save_over_unresolved_conflicts_is_refused() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    let count = |r: &Repo| -> i64 {
        r.conn()
            .query_row("SELECT count(*) FROM snapshots", [], |row| row.get(0))
            .unwrap()
    };
    let before = count(&repo);

    match save_as(&repo, "too soon", false).unwrap_err() {
        Error::Conflicts { paths } => assert_eq!(paths, vec![Path::new("c.txt").to_path_buf()]),
        other => panic!("expected Conflicts, got {other:?}"),
    }
    assert_eq!(count(&repo), before);
}

#[test]
fn save_after_resolving_succeeds_and_clears_conflict_rows() {
    let (tmp, repo) = setup();
    conflicted(tmp.path(), &repo);
    {
        let g = repo.write().unwrap();
        for cf in resolve::list_conflicts(&repo).unwrap() {
            resolve::take_side(&g, &cf, TakeOption::Theirs).unwrap();
        }
    }
    assert!(save_as(&repo, "merged", false).unwrap().saved());
    assert_eq!(resolve::conflict_count(&repo), 0);
    assert!(!resolve::merge_active(&repo));
}
