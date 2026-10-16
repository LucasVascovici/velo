//! `Repo::read_files_at`: batch reads of one snapshot.

use tempfile::TempDir;

use crate::commands::compact::{self, Range};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Error, InitOptions, ObjectLocation, Repo, SnapshotId};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

fn commit(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    entries: Vec<TreeEntry>,
    ts: i64,
) -> SnapshotId {
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main_branch(),
            parent,
            merge_parent: None,
            message: "batch",
            entries,
            meta: Default::default(),
            author: None,
            timestamp_ms: Some(ts),
            renames: &[],
        })
        .unwrap()
}

fn mixed(repo: &Repo) -> SnapshotId {
    commit(
        repo,
        None,
        vec![
            TreeEntry::file("a.txt", b"alpha\n".to_vec()),
            TreeEntry::file("dir/b.txt", b"beta\n".to_vec()),
            TreeEntry::file("crlf.txt", b"one\r\ntwo\r\n".to_vec()),
            TreeEntry::file("bin.dat", vec![0, 1, 2, 255, 0]),
            TreeEntry::symlink("link", "a.txt"),
        ],
        1,
    )
}

#[test]
fn bytes_equal_read_file_at_for_a_mixed_tree() {
    let (_t, repo) = setup();
    let id = mixed(&repo);
    let paths = ["a.txt", "dir/b.txt", "crlf.txt", "bin.dat", "link"];
    let got = repo.read_files_at(&id, &paths).unwrap();
    assert_eq!(got.len(), paths.len());
    for (p, (name, bytes)) in paths.iter().zip(&got) {
        assert_eq!(name, p);
        assert_eq!(bytes, &repo.read_file_at(&id, p).unwrap(), "{p}");
    }
    assert_eq!(got[4].1, b"a.txt");
}

#[test]
fn order_is_input_order_and_duplicates_repeat() {
    let (_t, repo) = setup();
    let id = mixed(&repo);
    let got = repo
        .read_files_at(&id, &["dir/b.txt", "a.txt", "dir/b.txt", "a.txt"])
        .unwrap();
    let names: Vec<&str> = got.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(names, ["dir/b.txt", "a.txt", "dir/b.txt", "a.txt"]);
    assert_eq!(got[0].1, b"beta\n");
    assert_eq!(got[2].1, b"beta\n");
    assert_eq!(got[3].1, b"alpha\n");
}

#[test]
fn the_first_missing_path_is_named() {
    let (_t, repo) = setup();
    let id = mixed(&repo);
    match repo.read_files_at(&id, &["a.txt", "nope1", "nope2"]) {
        Err(Error::InvalidInput { detail }) => {
            assert!(detail.contains("'nope1'"), "{detail}");
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn unknown_and_compacted_snapshots() {
    let (_t, repo) = setup();
    let mut prev: Option<SnapshotId> = None;
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = commit(
            &repo,
            prev.as_ref(),
            vec![TreeEntry::file("f.txt", format!("v{i}\n").into_bytes())],
            10 + i,
        );
        prev = Some(id.clone());
        ids.push(id);
    }
    let bogus = SnapshotId::from_stored("0".repeat(64));
    assert!(matches!(
        repo.read_files_at(&bogus, &["f.txt"]),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        repo.read_files_at(&bogus, &[] as &[&str]),
        Err(Error::NotFound { .. })
    ));

    let done = {
        let guard = repo.write().unwrap();
        compact::run(
            &guard,
            &main_branch(),
            &[Range {
                oldest: &ids[0],
                newest: &ids[1],
            }],
            Default::default(),
        )
        .unwrap()
    };
    let _ = done;
    assert!(matches!(
        repo.read_files_at(&ids[0], &["f.txt"]),
        Err(Error::Compacted { .. })
    ));
}

#[test]
fn works_with_database_objects() {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run_with(
        tmp.path(),
        InitOptions::new().objects(ObjectLocation::Database),
    )
    .unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let id = mixed(&repo);
    let got = repo
        .read_files_at(&id, &["crlf.txt", "link", "a.txt"])
        .unwrap();
    for (name, bytes) in &got {
        assert_eq!(bytes, &repo.read_file_at(&id, name).unwrap());
    }
    assert_eq!(got.len(), 3);
}

#[test]
fn twelve_hundred_paths_span_several_statements() {
    let (_t, repo) = setup();
    let entries: Vec<TreeEntry> = (0..1200)
        .map(|i| {
            TreeEntry::file(
                format!("d{}/f{i}.txt", i % 7),
                format!("body {i}\n").into_bytes(),
            )
        })
        .collect();
    let id = commit(&repo, None, entries, 1);
    let paths: Vec<String> = (0..1200)
        .rev()
        .map(|i| format!("d{}/f{i}.txt", i % 7))
        .collect();
    let got = repo.read_files_at(&id, &paths).unwrap();
    assert_eq!(got.len(), 1200);
    for (k, (name, bytes)) in got.iter().enumerate() {
        let i = 1199 - k;
        assert_eq!(name, &paths[k]);
        assert_eq!(bytes, format!("body {i}\n").as_bytes());
    }
}

#[test]
fn empty_input_is_empty_output() {
    let (_t, repo) = setup();
    let id = mixed(&repo);
    assert!(repo.read_files_at(&id, &[] as &[&str]).unwrap().is_empty());
}
