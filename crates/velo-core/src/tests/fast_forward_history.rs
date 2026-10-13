//! A fast-forward merge records the target as its merge parent and carries the
//! target's author, so the feature's history and blame survive on the branch.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::history;
use crate::commands::merge::{self, Outcome};
use crate::commands::status::{self, Tracking};
use crate::commands::sync;
use crate::commands::{self, blame, fsck};
use crate::transport::Spawn;
use crate::{Author, BranchName, Repo, SnapshotId};

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn write(root: &Path, rel: &str, content: &str) {
    fs::write(root.join(rel), content).unwrap();
}

fn save_as(root: &Path, msg: &str, who: Option<&Author>) -> SnapshotId {
    with_write(root, |g| {
        commands::save::run(
            g,
            Some(msg),
            commands::save::Options {
                author: who,
                ..Default::default()
            },
        )
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot")
    .hash
}

fn switch(root: &Path, branch: &str, existing: bool) {
    with_write(root, |g| commands::switch::run(g, branch, existing)).unwrap();
}

fn bring(root: &Path, source: &str) -> Outcome {
    with_write(root, |g| merge::run(g, merge::Mode::Bring { source })).unwrap()
}

struct Repro {
    _tmp: TempDir,
    root: PathBuf,
    base: SnapshotId,
    s1: SnapshotId,
    s2: SnapshotId,
    ff: SnapshotId,
    alice: Author,
    bob: Author,
}

/// base on main; s1 (alice) and s2 (bob) on feature/x; then main fast-forwards.
fn repro() -> Repro {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    commands::init::run(&root).unwrap();
    let alice = Author::new("alice").unwrap();
    let bob = Author::new("bob").unwrap();
    write(&root, "f.txt", "one\ntwo\n");
    let base = save_as(&root, "base", None);
    switch(&root, "feature/x", false);
    write(&root, "f.txt", "one\ntwo\nthree\n");
    let s1 = save_as(&root, "s1", Some(&alice));
    write(&root, "f.txt", "one\ntwo\nthree\nfour\n");
    let s2 = save_as(&root, "s2", Some(&bob));
    switch(&root, "main", true);
    let Outcome::FastForwarded { to, .. } = bring(&root, "feature/x") else {
        panic!("expected a fast-forward");
    };
    Repro {
        _tmp: tmp,
        root,
        base,
        s1,
        s2,
        ff: SnapshotId::from_stored(to),
        alice,
        bob,
    }
}

#[test]
fn history_keeps_the_feature_snapshots() {
    let r = repro();
    let repo = Repo::open_and_migrate(&r.root).unwrap();
    let h = history::run(&repo, history::Options::default()).unwrap();
    let ids: Vec<_> = h.entries.iter().map(|e| e.hash.clone()).collect();
    assert_eq!(ids, vec![r.ff, r.s2, r.s1, r.base]);
}

#[test]
fn the_snapshot_has_both_parents_and_the_targets_author() {
    let r = repro();
    let repo = Repo::open_and_migrate(&r.root).unwrap();
    let e = repo.snapshot(&r.ff).unwrap();
    assert_eq!(e.parent, Some(r.base.clone()));
    assert_eq!(e.merge_parent, Some(r.s2.clone()));
    assert_eq!(
        repo.snapshot_meta(&r.ff).unwrap().author(),
        repo.snapshot_meta(&r.s2).unwrap().author()
    );
    assert_eq!(repo.snapshot_meta(&r.ff).unwrap().author(), Some(r.bob));
}

#[test]
fn blame_credits_the_snapshot_that_wrote_each_line() {
    let r = repro();
    let repo = Repo::open_and_migrate(&r.root).unwrap();
    let b = blame::run(&repo, Path::new("f.txt"), Default::default()).unwrap();
    for l in &b.lines {
        let o = l.origin.as_ref().expect("every line is explained");
        assert!(o.author.is_some() || o.hash == r.base, "{:?}", l.text);
    }
    let by = |t: &str| {
        b.lines
            .iter()
            .find(|l| l.text == t)
            .and_then(|l| l.origin.clone())
            .unwrap()
    };
    assert_eq!(by("three").hash, r.s1);
    assert_eq!(by("three").author, Some(r.alice.clone()));
    assert_eq!(by("four").hash, r.s2);
    assert_eq!(by("four").author, Some(r.bob.clone()));
}

#[test]
fn file_history_includes_the_feature_snapshots() {
    let r = repro();
    let repo = Repo::open_and_migrate(&r.root).unwrap();
    let path = Path::new("f.txt");
    let h = history::run(
        &repo,
        history::Options {
            paths: &[path],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(h.entries.iter().any(|e| e.hash == r.s1));
}

#[test]
fn merging_again_is_up_to_date_and_the_base_is_the_feature_tip() {
    let r = repro();
    assert!(matches!(
        bring(&r.root, "feature/x"),
        Outcome::AlreadyUpToDate { .. }
    ));
    let repo = Repo::open_and_migrate(&r.root).unwrap();
    let feature: BranchName = "feature/x".parse().unwrap();
    let main: BranchName = "main".parse().unwrap();
    let (f, m) = (
        repo.branch_tip(&feature).unwrap().unwrap(),
        repo.branch_tip(&main).unwrap().unwrap(),
    );
    assert_eq!(merge::merge_base(&repo, &m, &f).unwrap(), Some(f));
}

#[test]
fn fsck_is_healthy_after_a_fast_forward() {
    let r = repro();
    let repo = Repo::open_and_migrate(&r.root).unwrap();
    let report = fsck::check(&repo).unwrap();
    assert!(report.is_healthy(), "{:?}", report.problems);
}

#[test]
fn a_fast_forwarded_branch_is_not_behind_and_pushes() {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    write(&origin, "a.txt", "shared\n");
    save_as(&origin, "base", None);
    switch(&origin, "feature/x", false);
    write(&origin, "a.txt", "shared\nmore\n");
    save_as(&origin, "s1", None);
    write(&origin, "a.txt", "shared\nmore\neven more\n");
    save_as(&origin, "s2", None);
    switch(&origin, "main", true);

    let copy = holder.path().join("copy");
    sync::clone(
        origin.to_str().unwrap(),
        &Spawn::new("velo"),
        sync::CloneOptions {
            dir: Some(copy.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(matches!(
        bring(&copy, "origin/feature/x"),
        Outcome::FastForwarded { .. }
    ));
    let repo = Repo::open_and_migrate(&copy).unwrap();
    match status::run(&repo, &[]).unwrap().tracking {
        Tracking::Known { behind, .. } => assert_eq!(behind, 0),
        other => panic!("expected a known tracking state: {other:?}"),
    }
    // Relative to the feature tip, main must not be behind: its merge parent
    // is the target, so the feature's snapshots are in its ancestry.
    let main_tip = commands::branch_tip(repo.conn(), "main").expect("main tip");
    let feature_tip =
        commands::branch_tip(repo.conn(), "remotes/origin/feature/x").expect("feature tip");
    assert_eq!(
        commands::ahead_behind(repo.conn(), &main_tip, &feature_tip).1,
        0,
        "the fast-forwarded main is not behind the feature tip"
    );
    drop(repo);
    with_write(&copy, |g| {
        sync::push(g, "origin", None, &Spawn::new("velo"), Default::default())
    })
    .expect("the fast-forward pushes as a fast-forward");
}
