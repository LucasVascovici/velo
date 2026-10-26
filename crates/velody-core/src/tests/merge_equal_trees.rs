//! A merge of diverged histories whose trees are equal still records a merge,
//! and "already up to date" means only that HEAD contains the other tip.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::merge::{self, MergeCommit, Outcome, Resolution};
use crate::commands::status::{self, Tracking};
use crate::commands::sync::{self, Pulled};
use crate::commands::{self};
use crate::transport::Spawn;
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn write(root: &Path, rel: &str, content: &str) {
    fs::write(root.join(rel), content).unwrap();
}

fn save(root: &Path, msg: &str) -> String {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot")
    .hash
    .into_string()
}

fn pull(root: &Path) -> Pulled {
    with_write(root, |g| {
        sync::pull(g, "origin", &Spawn::new("velody"), Default::default())
    })
    .unwrap()
}

fn push(root: &Path) -> crate::error::Result<sync::Pushed> {
    with_write(root, |g| {
        sync::push(g, "origin", None, &Spawn::new("velody"), Default::default())
    })
}

fn bring(root: &Path, source: &str) -> Outcome {
    with_write(root, |g| merge::run(g, merge::Mode::Bring { source })).unwrap()
}

fn merge_parent_of(root: &Path, hash: &str) -> String {
    let repo = Repo::open_and_migrate(root).unwrap();
    repo.conn()
        .query_row(
            "SELECT COALESCE(merge_parent, '') FROM snapshots WHERE hash = ?",
            [hash],
            |r| r.get(0),
        )
        .unwrap()
}

fn remote_tip(root: &Path) -> String {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::tracking_ref(repo.conn(), "main").unwrap().1
}

fn merge_head_exists(root: &Path) -> bool {
    root.join(".velo/MERGE_HEAD").exists()
}

fn clone_of(url: &Path, dest: &Path) {
    sync::clone(
        url.to_str().unwrap(),
        &Spawn::new("velody"),
        sync::CloneOptions {
            dir: Some(dest),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
}

/// An origin with one snapshot: (holder, origin).
fn origin() -> (TempDir, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    write(&origin, "a.txt", "shared\n");
    save(&origin, "base");
    (holder, origin)
}

fn ahead_behind(root: &Path) -> (usize, usize) {
    let repo = Repo::open_and_migrate(root).unwrap();
    match status::run(&repo, &[]).unwrap().tracking {
        Tracking::Known { ahead, behind, .. } => (ahead, behind),
        other => panic!("expected a known tracking state: {other:?}"),
    }
}

#[test]
fn equal_trees_on_diverged_histories_record_a_merge_that_pushes() {
    let (holder, origin) = origin();
    let (one, two) = (holder.path().join("one"), holder.path().join("two"));
    clone_of(&origin, &one);
    clone_of(&origin, &two);

    // The same content change, made independently.
    write(&one, "b.txt", "same\n");
    save(&one, "one made b");
    write(&two, "b.txt", "same\n");
    let ours = save(&two, "two made b");
    push(&one).unwrap();

    assert!(matches!(pull(&two), Pulled::Diverged { .. }));
    assert_eq!(ahead_behind(&two), (1, 1));

    let Outcome::Merged(result) = bring(&two, "origin/main") else {
        panic!("diverged histories must be a merge, not up to date");
    };
    assert!(result.is_clean());
    assert!(result.applied_nothing());
    assert!(merge_head_exists(&two), "the merge must be pending");

    let merged = save(&two, "Merge origin/main");
    assert_ne!(merged, ours);
    assert_eq!(merge_parent_of(&two, &merged), remote_tip(&two));
    assert!(!merge_head_exists(&two));
    assert_eq!(ahead_behind(&two).1, 0, "no longer diverged");

    push(&two).expect("the joined histories fast-forward");
}

#[test]
fn a_clone_of_a_clone_pushes_after_an_equal_tree_merge() {
    let (holder, origin) = origin();
    let (a, b) = (holder.path().join("a"), holder.path().join("b"));
    clone_of(&origin, &a);
    clone_of(&a, &b);

    write(&a, "x.txt", "same\n");
    save(&a, "a made x");
    write(&b, "x.txt", "same\n");
    save(&b, "b made x");

    assert!(matches!(pull(&b), Pulled::Diverged { .. }));
    let Outcome::Merged(result) = bring(&b, "origin/main") else {
        panic!("expected a merge");
    };
    assert!(result.applied_nothing());
    save(&b, "Merge origin/main");
    push(&b).expect("B pushes to A after the merge");
}

#[test]
fn an_ancestor_tip_is_up_to_date_and_writes_no_merge_state() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    commands::init::run(&root).unwrap();
    write(&root, "f.txt", "base\n");
    save(&root, "base");
    with_write(&root, |g| commands::switch::run(g, "feature", false)).unwrap();
    write(&root, "g.txt", "feature\n");
    save(&root, "feature edit");
    with_write(&root, |g| commands::switch::run(g, "main", true)).unwrap();
    write(&root, "h.txt", "main\n");
    save(&root, "main edit");

    // A real merge, so `feature` is reachable only through a merge parent.
    assert!(matches!(bring(&root, "feature"), Outcome::Merged(_)));
    save(&root, "Merge feature");
    write(&root, "h.txt", "main again\n");
    save(&root, "more main");

    let outcome = bring(&root, "feature");
    assert!(
        matches!(outcome, Outcome::AlreadyUpToDate { .. }),
        "{outcome:?}"
    );
    assert!(!merge_head_exists(&root));
}

fn file_save(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    message: &str,
    ts: i64,
    files: &[(&str, &str)],
) -> SnapshotId {
    let branch: BranchName = "main".parse().unwrap();
    let guard = repo.write().unwrap();
    guard
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message,
            entries: files
                .iter()
                .map(|(p, c)| TreeEntry::file(*p, c.as_bytes().to_vec()))
                .collect(),
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

fn spec<'a>(
    branch: &'a BranchName,
    ours: &'a SnapshotId,
    theirs: &'a SnapshotId,
) -> MergeCommit<'a> {
    MergeCommit {
        branch,
        ours,
        theirs,
        resolutions: &[] as &[(String, Resolution)],
        message: "merge",
        meta: SnapshotMeta::new(),
        author: None,
        timestamp_ms: Some(10),
    }
}

fn plan_and_commit_agree(repo: &Repo) {
    let both = [("a.txt", "a\n"), ("b.txt", "b\n")];
    let base = file_save(repo, None, "base", 1, &[("a.txt", "a\n")]);
    let ours = file_save(repo, Some(&base), "ours", 2, &both);
    let theirs = file_save(repo, Some(&base), "theirs", 3, &both);

    let plan = merge::plan(repo, &ours, &theirs).unwrap();
    assert!(plan.files.is_empty());
    assert_ne!(plan.base, Some(theirs.clone()));

    let branch: BranchName = "main".parse().unwrap();
    let guard = repo.write().unwrap();
    let merged = merge::commit(&guard, spec(&branch, &ours, &theirs)).unwrap();
    let mp: String = repo
        .conn()
        .query_row(
            "SELECT merge_parent FROM snapshots WHERE hash = ?",
            [merged.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mp, theirs.as_str());

    // Committing something HEAD already contains is still refused.
    assert!(merge::commit(&guard, spec(&branch, &merged, &theirs)).is_err());
}

#[test]
fn plan_and_commit_agree_on_equal_trees() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    commands::init::run(&root).unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    plan_and_commit_agree(&repo);
}

#[test]
fn plan_and_commit_agree_on_a_single_file_repository() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::create_file(&tmp.path().join("r.velo")).unwrap();
    plan_and_commit_agree(&repo);
}
