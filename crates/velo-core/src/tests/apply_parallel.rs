//! Merge apply fans out in parallel (15.7-e): a 300-file merge mixing every
//! outcome must produce exactly what the serial implementation did.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands;
use crate::commands::apply::{self, FileAction, Tree};
use crate::Repo;

const N: usize = 300;

fn setup(db: bool) -> TempDir {
    let tmp = TempDir::new().unwrap();
    if db {
        commands::init::run_with(
            tmp.path(),
            crate::InitOptions::new().objects(crate::ObjectLocation::Database),
        )
        .unwrap();
    } else {
        commands::init::run(tmp.path()).unwrap();
    }
    tmp
}

fn body(i: usize, first: &str, last: &str) -> String {
    format!("{first}\nmid a {i}\nmid b {i}\nmid c {i}\n{last}\n")
}

fn run(db: bool) {
    let tmp = setup(db);
    let root = tmp.path();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    let objects = guard.repo().objects();
    let put = |s: &str| objects.put(s.as_bytes()).unwrap();

    let mut anc = Tree::new();
    let mut our = Tree::new();
    let mut thr = Tree::new();
    let mut expect: BTreeMap<String, (FileAction, Option<String>)> = BTreeMap::new();
    let mode = 0o644;

    for i in 0..N {
        let path = format!("d{}/f{i:03}.txt", i % 7);
        let base = body(i, "first", "last");
        match i % 7 {
            0 => {
                // Added by theirs.
                let t = body(i, "theirs new", "last");
                thr.insert(path.clone(), (put(&t), mode));
                expect.insert(path, (FileAction::Added, Some(t)));
            }
            1 => {
                // Updated by theirs only.
                let t = body(i, "theirs", "last");
                anc.insert(path.clone(), (put(&base), mode));
                our.insert(path.clone(), (put(&base), mode));
                thr.insert(path.clone(), (put(&t), mode));
                expect.insert(path, (FileAction::Updated, Some(t)));
            }
            2 => {
                // Both edit, far apart: auto-merged.
                let o = body(i, "ours", "last");
                let t = body(i, "first", "theirs");
                anc.insert(path.clone(), (put(&base), mode));
                our.insert(path.clone(), (put(&o), mode));
                thr.insert(path.clone(), (put(&t), mode));
                let merged = body(i, "ours", "theirs");
                expect.insert(path, (FileAction::AutoMerged, Some(merged)));
            }
            3 => {
                // Deleted by theirs, untouched by ours.
                anc.insert(path.clone(), (put(&base), mode));
                our.insert(path.clone(), (put(&base), mode));
                expect.insert(path, (FileAction::Deleted, None));
            }
            4 => {
                // Theirs deleted, ours modified: ours kept.
                let o = body(i, "ours", "last");
                anc.insert(path.clone(), (put(&base), mode));
                our.insert(path.clone(), (put(&o), mode));
                expect.insert(path, (FileAction::KeptOurs, Some(o)));
            }
            5 => {
                // Same line edited differently: conflict, working copy untouched.
                let o = body(i, "ours", "last");
                let t = body(i, "theirs", "last");
                anc.insert(path.clone(), (put(&base), mode));
                our.insert(path.clone(), (put(&o), mode));
                thr.insert(path.clone(), (put(&t), mode));
                expect.insert(path, (FileAction::Conflicted, Some(o)));
            }
            _ => {
                // Unchanged everywhere: not reported.
                for t in [&mut anc, &mut our, &mut thr] {
                    t.insert(path.clone(), (put(&base), mode));
                }
            }
        }
    }
    // The working tree starts as "ours".
    for (path, (hash, _)) in &our {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(&full, objects.get(hash).unwrap()).unwrap();
    }
    drop(objects);

    let applied = apply::reconcile_tree(&guard, &anc, &our, &thr).unwrap();

    let got: Vec<(String, FileAction)> = applied
        .files
        .iter()
        .map(|f| (f.path.clone(), f.action))
        .collect();
    let want: Vec<(String, FileAction)> =
        expect.iter().map(|(p, (a, _))| (p.clone(), *a)).collect();
    assert_eq!(got, want, "outcomes and their order");

    for (path, (action, content)) in &expect {
        let full = root.join(path);
        match content {
            Some(c) => assert_eq!(
                fs::read_to_string(&full).unwrap(),
                *c,
                "{path} ({action:?})"
            ),
            None => assert!(!full.exists(), "{path} should be deleted"),
        }
    }

    applied.record_conflicts(guard.repo().conn()).unwrap();
    let mut stmt = guard
        .repo()
        .conn()
        .prepare(
            "SELECT path, ancestor_hash, our_hash, their_hash FROM conflict_files ORDER BY path",
        )
        .unwrap();
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let want_rows: Vec<(String, String, String, String)> = expect
        .iter()
        .filter(|(_, (a, _))| *a == FileAction::Conflicted)
        .map(|(p, _)| {
            (
                p.clone(),
                anc[p].0.clone(),
                our[p].0.clone(),
                thr[p].0.clone(),
            )
        })
        .collect();
    assert!(!want_rows.is_empty());
    assert_eq!(rows, want_rows);
}

#[test]
fn mixed_300_file_merge_matches_serial_expectations_on_files() {
    run(false);
}

#[test]
fn mixed_300_file_merge_matches_serial_expectations_on_database() {
    run(true);
}

#[test]
fn a_write_failure_names_every_path() {
    let tmp = setup(false);
    let root = tmp.path();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    let objects = guard.repo().objects();
    let mut thr = Tree::new();
    for p in ["blocked/a.txt", "blocked/b.txt"] {
        thr.insert(p.to_string(), (objects.put(p.as_bytes()).unwrap(), 0o644));
    }
    drop(objects);
    // A file where the directory must go makes both writes fail.
    fs::write(Path::new(root).join("blocked"), "x").unwrap();
    let err = apply::reconcile_tree(&guard, &Tree::new(), &Tree::new(), &thr)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("blocked/a.txt") && err.contains("blocked/b.txt"),
        "{err}"
    );
}

#[test]
fn a_file_becoming_a_directory_removes_the_file_before_writing_under_it() {
    let tmp = setup(false);
    let root = tmp.path();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    let objects = guard.repo().objects();
    let old = objects.put(b"old\n").unwrap();
    let new = objects.put(b"new\n").unwrap();
    drop(objects);
    let mut anc = Tree::new();
    anc.insert("a".to_string(), (old, 0o644));
    let our = anc.clone();
    let mut thr = Tree::new();
    thr.insert("a/b".to_string(), (new, 0o644));
    fs::write(root.join("a"), "old\n").unwrap();

    let applied = apply::reconcile_tree(&guard, &anc, &our, &thr).unwrap();

    assert_eq!(fs::read_to_string(root.join("a/b")).unwrap(), "new\n");
    let got: Vec<(String, FileAction)> = applied
        .files
        .iter()
        .map(|f| (f.path.clone(), f.action))
        .collect();
    assert_eq!(
        got,
        vec![
            ("a".to_string(), FileAction::Deleted),
            ("a/b".to_string(), FileAction::Added)
        ]
    );
}
