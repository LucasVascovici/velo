//! A file we moved and they edited merges at the new path, and a missing
//! rename edge is a modify/delete conflict rather than a resurrection.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::apply::{self, FileAction};
use crate::commands::merge::{self, MergeCommit, Outcome, PlannedChange, Resolution, ThreeWay};
use crate::commands::resolve::{self, OnEdits, TakeOption};
use crate::commands::{self};
use crate::{BranchName, Repo, SnapshotId, SnapshotMeta};

const BASE: &str = "one\ntwo\nthree\nfour\nfive\nsix\n";

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn write(root: &Path, rel: &str, content: &str) {
    let full = root.join(rel);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
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

fn mv(root: &Path, from: &str, to: &str) {
    with_write(root, |g| {
        commands::mv::run(g, Path::new(from), Path::new(to))
    })
    .unwrap();
}

fn switch(root: &Path, branch: &str, force: bool) {
    with_write(root, |g| commands::switch::run(g, branch, force)).unwrap();
}

fn bring(root: &Path, source: &str) -> ThreeWay {
    match with_write(root, |g| merge::run(g, merge::Mode::Bring { source })).unwrap() {
        Outcome::Merged(t) => t,
        other => panic!("expected a three-way merge: {other:?}"),
    }
}

/// A repository with `NOTES.txt` saved on main; returns (tmp, root, base hash).
fn setup() -> (TempDir, PathBuf, String) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    commands::init::run(&root).unwrap();
    write(&root, "NOTES.txt", BASE);
    let base = save(&root, "base");
    (tmp, root, base)
}

/// Their side edits `NOTES.txt` on `side`; returns to main. Returns the side tip.
fn side_edit(root: &Path, content: &str) -> String {
    switch(root, "side", false);
    write(root, "NOTES.txt", content);
    let tip = save(root, "side edit");
    switch(root, "main", true);
    tip
}

fn side_line() -> String {
    format!("{BASE}side line\n")
}

fn branch(name: &str) -> BranchName {
    name.parse().unwrap()
}

#[test]
fn their_edit_lands_at_the_new_path() {
    let (_tmp, root, _) = setup();
    side_edit(&root, &side_line());
    mv(&root, "NOTES.txt", "docs/notes.txt");
    save(&root, "move");

    let result = bring(&root, "side");
    assert!(result.is_clean());
    let files: Vec<_> = result
        .files()
        .iter()
        .map(|f| (f.path.as_str(), f.action))
        .collect();
    assert_eq!(files, vec![("docs/notes.txt", FileAction::Updated)]);
    assert!(!root.join("NOTES.txt").exists(), "the old path came back");
    assert_eq!(read(&root, "docs/notes.txt"), side_line());

    let status = commands::status::run(&Repo::open_and_migrate(&root).unwrap(), &[]).unwrap();
    assert!(
        status.merging.is_some(),
        "the merge should still be pending"
    );
    assert!(status.conflicts.is_empty(), "{:?}", status.conflicts);
    // The merged edit is unsaved relative to our tip, so the new path shows as
    // modified; the old path must not appear anywhere.
    assert_eq!(status.modified, vec!["docs/notes.txt"]);
    assert!(status.new_files.is_empty(), "{:?}", status.new_files);
    assert!(status.deleted.is_empty(), "{:?}", status.deleted);
    assert!(!status
        .modified
        .iter()
        .chain(&status.new_files)
        .chain(&status.deleted)
        .any(|p| p == "NOTES.txt"));
}

#[test]
fn overlapping_edits_conflict_at_the_new_path() {
    let (_tmp, root, _) = setup();
    side_edit(&root, &BASE.replace("three", "theirs"));
    mv(&root, "NOTES.txt", "docs/notes.txt");
    write(&root, "docs/notes.txt", &BASE.replace("three", "ours"));
    save(&root, "move and edit");

    let result = bring(&root, "side");
    assert_eq!(result.conflicts(), vec!["docs/notes.txt"]);
    assert!(!root.join("NOTES.txt").exists());
    let rows = resolve::list_conflicts(&Repo::open_and_migrate(&root).unwrap()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, "docs/notes.txt");
}

#[test]
fn without_a_rename_edge_modify_delete_is_a_conflict() {
    let (_tmp, root, _) = setup();
    side_edit(&root, &side_line());
    fs::remove_file(root.join("NOTES.txt")).unwrap();
    write(&root, "other.txt", "x\n");
    save(&root, "delete notes");

    let result = bring(&root, "side");
    assert_eq!(result.conflicts(), vec!["NOTES.txt"]);
    assert!(!root.join("NOTES.txt").exists());
    let row = resolve::list_conflicts(&Repo::open_and_migrate(&root).unwrap())
        .unwrap()
        .remove(0);
    assert!(row.our_hash.is_empty());

    let taken = with_write(&root, |g| {
        resolve::take_side(g, &row, TakeOption::Ours, OnEdits::Refuse)
    })
    .unwrap();
    assert_eq!(taken.path, "NOTES.txt");
    assert!(
        !root.join("NOTES.txt").exists(),
        "--take ours keeps it deleted"
    );
}

#[test]
fn taking_theirs_on_modify_delete_writes_their_file() {
    let (_tmp, root, _) = setup();
    side_edit(&root, &side_line());
    fs::remove_file(root.join("NOTES.txt")).unwrap();
    save(&root, "delete notes");

    bring(&root, "side");
    let row = resolve::list_conflicts(&Repo::open_and_migrate(&root).unwrap())
        .unwrap()
        .remove(0);
    with_write(&root, |g| {
        resolve::take_side(g, &row, TakeOption::Theirs, OnEdits::Refuse)
    })
    .unwrap();
    assert_eq!(read(&root, "NOTES.txt"), side_line());
}

#[test]
fn a_rename_chain_applies_their_edit_at_the_last_name() {
    let (_tmp, root, _) = setup();
    side_edit(&root, &side_line());
    mv(&root, "NOTES.txt", "G.txt");
    save(&root, "first move");
    mv(&root, "G.txt", "docs/H.txt");
    save(&root, "second move");

    let result = bring(&root, "side");
    assert!(result.is_clean());
    assert!(!root.join("NOTES.txt").exists());
    assert!(!root.join("G.txt").exists());
    assert_eq!(read(&root, "docs/H.txt"), side_line());
}

#[test]
fn plan_and_commit_agree_with_run() {
    let (_tmp, root, _) = setup();
    let theirs = side_edit(&root, &side_line());
    mv(&root, "NOTES.txt", "docs/notes.txt");
    let ours = save(&root, "move");

    let repo = Repo::open_and_migrate(&root).unwrap();
    let (ours, theirs) = (
        SnapshotId::from_stored(&ours),
        SnapshotId::from_stored(&theirs),
    );
    let plan = merge::plan(&repo, &ours, &theirs).unwrap();
    assert_eq!(plan.files.len(), 1, "{:?}", plan.files);
    assert_eq!(plan.files[0].path, "docs/notes.txt");
    assert!(matches!(
        plan.files[0].change,
        PlannedChange::AutoMerge { .. } | PlannedChange::Take { .. }
    ));

    let main = branch("main");
    let merged = {
        let guard = repo.write().unwrap();
        merge::commit(
            &guard,
            MergeCommit {
                branch: &main,
                ours: &ours,
                theirs: &theirs,
                resolutions: &[] as &[(String, Resolution)],
                message: "merge",
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(10),
            },
        )
        .unwrap()
    };
    let tree = apply::load_tree(repo.conn(), merged.as_str()).unwrap();
    assert!(!tree.contains_key("NOTES.txt"));
    assert_eq!(
        repo.read_file_at(&merged, "docs/notes.txt").unwrap(),
        side_line().into_bytes()
    );
}

#[test]
fn commit_takes_their_content_at_the_new_path() {
    let (_tmp, root, _) = setup();
    let theirs_content = BASE.replace("three", "theirs");
    let theirs = side_edit(&root, &theirs_content);
    mv(&root, "NOTES.txt", "docs/notes.txt");
    write(&root, "docs/notes.txt", &BASE.replace("three", "ours"));
    let ours = save(&root, "move and edit");

    let repo = Repo::open_and_migrate(&root).unwrap();
    let (ours, theirs) = (
        SnapshotId::from_stored(&ours),
        SnapshotId::from_stored(&theirs),
    );
    let main = branch("main");
    let resolutions = [("docs/notes.txt".to_string(), Resolution::Theirs)];
    let merged = {
        let guard = repo.write().unwrap();
        merge::commit(
            &guard,
            MergeCommit {
                branch: &main,
                ours: &ours,
                theirs: &theirs,
                resolutions: &resolutions,
                message: "merge",
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(10),
            },
        )
        .unwrap()
    };
    let tree = apply::load_tree(repo.conn(), merged.as_str()).unwrap();
    assert!(!tree.contains_key("NOTES.txt"));
    assert_eq!(
        repo.read_file_at(&merged, "docs/notes.txt").unwrap(),
        theirs_content.into_bytes()
    );
}
