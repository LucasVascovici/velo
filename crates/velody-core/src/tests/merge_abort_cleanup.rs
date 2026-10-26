//! `merge --abort` returns the working tree to the pre-merge snapshot,
//! removing files the merge added and the directories that leaves empty.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands::{self, merge, save, switch};
use crate::{Error, Repo, SnapshotId};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn save_as(repo: &Repo, msg: &str) -> String {
    let g = repo.write().unwrap();
    save::run(&g, Some(msg), save::Options::default())
        .unwrap()
        .into_result()
        .expect("snapshot")
        .hash
        .into_string()
}

/// A conflicted merge of `other` into `main`; `other` also adds
/// `docs/guide.md`. Returns the pre-merge snapshot.
fn setup() -> (TempDir, Repo, String) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let root = tmp.path();
    write(root, "a.txt", "one\ntwo\nthree\n");
    write(root, "b.txt", "x\ny\nz\n");
    write(root, ".veloignore", "*.log\n");
    save_as(&repo, "base");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "other", false).unwrap();
    }
    write(root, "a.txt", "one\nOTHER\nthree\n");
    write(root, "b.txt", "x\ny\nOTHER\n");
    write(root, "docs/guide.md", "guide\n");
    save_as(&repo, "other edit");
    {
        let g = repo.write().unwrap();
        switch::run(&g, "main", true).unwrap();
    }
    write(root, "a.txt", "one\nMAIN\nthree\n");
    write(root, "b.txt", "MAIN\ny\nz\n");
    let pre = save_as(&repo, "main edit");
    (tmp, repo, pre)
}

fn bring(repo: &Repo) {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Bring { source: "other" }).unwrap();
}

fn abort(repo: &Repo) {
    let g = repo.write().unwrap();
    merge::run(&g, merge::Mode::Abort).unwrap();
}

/// Every non-ignored file on disk (outside `.velo`), sorted, with contents.
fn disk(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace("\\", "/");
            if rel == ".velo" || rel.ends_with(".log") {
                continue;
            }
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                out.push((rel, fs::read_to_string(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn tree(repo: &Repo, pre: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = repo
        .tree_at(&SnapshotId::from_stored(pre))
        .unwrap()
        .into_iter()
        .map(|f| {
            let bytes = repo.read_object(&f.object).unwrap();
            (f.path, String::from_utf8(bytes).unwrap())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn abort_removes_added_file_and_empty_dir() {
    let (tmp, repo, pre) = setup();
    bring(&repo);
    assert!(tmp.path().join("docs/guide.md").exists());
    abort(&repo);
    assert!(!tmp.path().join("docs").exists());
    assert_eq!(disk(tmp.path()), tree(&repo, &pre));
    assert!(commands::get_dirty_files(&repo).is_empty());
}

#[test]
fn abort_after_every_conflict_is_resolved() {
    let (tmp, repo, pre) = setup();
    bring(&repo);
    {
        let g = repo.write().unwrap();
        g.conn().execute("DELETE FROM conflict_files", []).unwrap();
    }
    assert!(tmp.path().join(".velo/MERGE_HEAD").exists());
    abort(&repo);
    assert!(!tmp.path().join("docs").exists());
    assert_eq!(disk(tmp.path()), tree(&repo, &pre));
}

#[test]
fn untracked_file_blocks_the_merge() {
    let (tmp, repo, _pre) = setup();
    write(tmp.path(), "docs/guide.md", "mine\n");
    let g = repo.write().unwrap();
    let err = merge::run(&g, merge::Mode::Bring { source: "other" }).unwrap_err();
    assert!(matches!(err, Error::DirtyWorkingTree { .. }), "{err:?}");
}

#[test]
fn ignored_file_survives_abort() {
    let (tmp, repo, pre) = setup();
    write(tmp.path(), "notes.log", "keep\n");
    bring(&repo);
    abort(&repo);
    assert_eq!(
        fs::read_to_string(tmp.path().join("notes.log")).unwrap(),
        "keep\n"
    );
    assert_eq!(disk(tmp.path()), tree(&repo, &pre));
}

#[test]
fn file_made_during_the_merge_survives_abort() {
    let (tmp, repo, _pre) = setup();
    bring(&repo);
    write(tmp.path(), "scratch.txt", "mine\n");
    abort(&repo);
    assert_eq!(
        fs::read_to_string(tmp.path().join("scratch.txt")).unwrap(),
        "mine\n"
    );
    assert!(!tmp.path().join("docs").exists());
}
