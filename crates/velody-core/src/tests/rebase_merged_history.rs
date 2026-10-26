//! Rebase skips what the target reaches through a merge's second parent.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::merge::{self, Mode as MergeMode};
use crate::commands::rebase::{self, Mode, Outcome};
use crate::commands::{save, switch};
use crate::{Repo, SnapshotId};

fn setup() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    crate::commands::init::run(&path).unwrap();
    (tmp, path)
}

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    f(&guard)
}

fn write(root: &Path, rel: &str, body: &str) {
    fs::write(root.join(rel), body).unwrap();
}

fn save_as(root: &Path, msg: &str) -> String {
    with_write(root, |g| save::run(g, Some(msg), save::Options::default()))
        .unwrap()
        .into_result()
        .expect("a snapshot")
        .hash
        .into_string()
}

fn go(root: &Path, branch: &str, create: bool) {
    with_write(root, |g| switch::run(g, branch, create)).unwrap();
}

fn merge_in(root: &Path, source: &str) {
    with_write(root, |g| merge::run(g, MergeMode::Bring { source })).unwrap();
}

fn rebase_onto(root: &Path, onto: &str) -> Outcome {
    with_write(root, |g| {
        rebase::run(
            g,
            Mode::Start {
                onto: &SnapshotId::from_stored(onto),
            },
            None,
        )
    })
    .unwrap()
}

fn tip(root: &Path, branch: &str) -> String {
    with_write(root, |g| crate::commands::branch_tip(g.conn(), branch)).expect("branch tip")
}

/// `f` merged main (so main's X is in f), then main merged `f` (so F1 is
/// main's second-parent ancestor), then `f` saves W.
fn merged_both_ways() -> (TempDir, PathBuf) {
    let (tmp, root) = setup();
    write(&root, "a.txt", "a\n");
    save_as(&root, "A");
    go(&root, "f", true);
    write(&root, "f1.txt", "f1\n");
    save_as(&root, "F1");
    go(&root, "main", false);
    write(&root, "x.txt", "x\n");
    save_as(&root, "X");
    go(&root, "f", false);
    merge_in(&root, "main");
    save_as(&root, "merge main");
    go(&root, "main", false);
    write(&root, "y.txt", "y\n");
    save_as(&root, "Y");
    merge_in(&root, "f");
    save_as(&root, "merge f");
    go(&root, "f", false);
    write(&root, "w.txt", "w\n");
    save_as(&root, "W");
    (tmp, root)
}

#[test]
fn replays_only_what_the_target_does_not_reach() {
    let (_tmp, root) = merged_both_ways();
    let main_tip = tip(&root, "main");
    let Outcome::Completed { replayed, .. } = rebase_onto(&root, &main_tip) else {
        panic!("expected a completed rebase");
    };
    let msgs: Vec<&str> = replayed.iter().map(|r| r.message.as_str()).collect();
    assert_eq!(msgs, vec!["W"]);
    for (f, b) in [
        ("a.txt", "a\n"),
        ("f1.txt", "f1\n"),
        ("x.txt", "x\n"),
        ("y.txt", "y\n"),
        ("w.txt", "w\n"),
    ] {
        assert_eq!(fs::read_to_string(root.join(f)).unwrap(), b, "{f}");
    }
}

#[test]
fn branch_contained_through_a_merge_parent_is_up_to_date() {
    let (_tmp, root) = setup();
    write(&root, "a.txt", "a\n");
    save_as(&root, "A");
    go(&root, "f", true);
    write(&root, "f1.txt", "f1\n");
    save_as(&root, "F1");
    go(&root, "main", false);
    write(&root, "x.txt", "x\n");
    save_as(&root, "X");
    merge_in(&root, "f");
    save_as(&root, "merge f");
    go(&root, "f", false);
    let main_tip = tip(&root, "main");
    assert!(matches!(
        rebase_onto(&root, &main_tip),
        Outcome::AlreadyUpToDate
    ));
}

#[test]
fn head_containing_the_target_through_a_merge_parent_is_up_to_date() {
    let (_tmp, root) = setup();
    write(&root, "a.txt", "a\n");
    save_as(&root, "A");
    go(&root, "f", true);
    write(&root, "f1.txt", "f1\n");
    save_as(&root, "F1");
    go(&root, "main", false);
    write(&root, "x.txt", "x\n");
    let x = save_as(&root, "X");
    go(&root, "f", false);
    merge_in(&root, "main");
    save_as(&root, "merge main");
    write(&root, "w.txt", "w\n");
    save_as(&root, "W");
    assert!(matches!(rebase_onto(&root, &x), Outcome::AlreadyUpToDate));
}
