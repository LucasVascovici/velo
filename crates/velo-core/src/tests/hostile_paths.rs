//! The one path rule (`safe_path`): refused at ingestion and before any write
//! to the working tree.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::bundle::{self, Bundle, FileMapRow, RenameRow, SnapshotRow};
use crate::commands::{import, restore, snapshot_id_of};
use crate::safe_path::{check, check_for_this_platform, PathProblem};
use crate::tree::{SaveTree, TreeEntry};
use crate::{BranchName, Error, Repo, SnapshotId, SnapshotMeta};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    (tmp, repo)
}

fn main_branch() -> BranchName {
    "main".parse().unwrap()
}

fn save(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    entries: Vec<TreeEntry>,
    renames: &[(PathBuf, PathBuf)],
) -> crate::Result<SnapshotId> {
    let branch = main_branch();
    repo.write()?.save_tree(SaveTree {
        branch: &branch,
        parent,
        merge_parent: None,
        message: "m",
        entries,
        meta: SnapshotMeta::default(),
        author: None,
        timestamp_ms: Some(1),
        renames,
    })
}

fn count(repo: &Repo, table: &str) -> i64 {
    repo.conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn assert_unsafe<T: std::fmt::Debug>(result: crate::Result<T>, want: &str) -> PathProblem {
    match result {
        Err(Error::UnsafePath { path, problem }) => {
            assert!(path.contains(want), "error names '{path}', wanted '{want}'");
            problem
        }
        other => panic!("expected UnsafePath for '{want}', got {other:?}"),
    }
}

#[test]
fn the_store_rule_refuses_and_accepts_the_documented_paths() {
    for bad in [
        "../escape",
        "/etc/passwd",
        "C:/x",
        "a/../b",
        "./a",
        "a//b",
        "a/",
        "",
        ".velo/x",
        ".VELO/x",
        "sub/.velo/x",
        ".velo./x",
        ".velo /x",
        "a\0b",
    ] {
        assert!(check(bad).is_err(), "{bad:?} should be refused");
    }
    for good in [
        "a/b.txt",
        ".velocity",
        "x/.velo-notes",
        "aux.c",
        "a:b",
        "a/b c",
    ] {
        assert!(check(good).is_ok(), "{good:?} should be accepted");
    }
    assert_eq!(check("../x"), Err(PathProblem::DotDotComponent));
    assert_eq!(check("/x"), Err(PathProblem::Absolute));
    assert_eq!(check("C:/x"), Err(PathProblem::DrivePrefix));
    assert_eq!(check(".VELO/x"), Err(PathProblem::RepositoryDirectory));
}

#[test]
fn the_platform_rule_is_the_store_rule_off_windows() {
    #[cfg(not(windows))]
    for p in ["aux.c", "a:b", "trail.", "CON"] {
        assert!(check_for_this_platform(p).is_ok(), "{p}");
    }
    #[cfg(windows)]
    {
        for p in [
            "CON", "con.txt", "dir/NUL", "a:stream", "trail.", "velo~1/x", "d/com3.x",
        ] {
            assert!(check(p).is_ok(), "{p} passes the store rule");
            assert!(check_for_this_platform(p).is_err(), "{p} refused here");
        }
        assert!(check_for_this_platform("console.txt").is_ok());
    }
    assert!(check_for_this_platform("../x").is_err());
}

#[test]
fn save_tree_refuses_unsafe_paths_and_creates_nothing() {
    let (_tmp, repo) = setup();
    for bad in ["../escape", "/abs", ".velo/config", "C:/x", "a//b", ""] {
        let result = save(&repo, None, vec![TreeEntry::file(bad, b"x".to_vec())], &[]);
        assert_unsafe(result, bad);
    }
    assert_eq!(count(&repo, "snapshots"), 0);
    assert_eq!(count(&repo, "file_map"), 0);
}

#[test]
fn save_tree_refuses_a_file_that_is_also_a_directory() {
    let (_tmp, repo) = setup();
    let result = save(
        &repo,
        None,
        vec![
            TreeEntry::file("a", b"1".to_vec()),
            TreeEntry::file("a.txt", b"2".to_vec()),
            TreeEntry::file("a/b", b"3".to_vec()),
        ],
        &[],
    );
    let problem = assert_unsafe(result, "a/b");
    assert_eq!(
        problem,
        PathProblem::FileAndDirectory {
            other: "a".to_string()
        }
    );
    assert_eq!(count(&repo, "snapshots"), 0);
}

#[test]
fn save_tree_refuses_an_unsafe_rename_edge() {
    let (_tmp, repo) = setup();
    let first = save(&repo, None, vec![TreeEntry::file("a", b"1".to_vec())], &[]).unwrap();
    let renames = [(PathBuf::from("a"), PathBuf::from("../b"))];
    let result = save(
        &repo,
        Some(&first),
        vec![TreeEntry::file("b", b"1".to_vec())],
        &renames,
    );
    assert_unsafe(result, "../b");
    assert_eq!(count(&repo, "snapshots"), 1);
}

/// A one-snapshot bundle holding `rows`, with a valid id for them.
fn crafted(source: &Repo, rows: &[&str], renames: Vec<RenameRow>) -> (Bundle, String) {
    let object = source.objects().put(b"payload\n").unwrap();
    let frame = source.objects().compressed(&object).unwrap();
    let id = snapshot_id_of(
        rows.iter().map(|p| (*p, object.as_str(), 0)),
        "",
        "",
        "crafted",
        1,
        &SnapshotMeta::default(),
    );
    let bundle = Bundle {
        snapshots: vec![SnapshotRow {
            hash: id.clone(),
            message: "crafted".into(),
            branch: "main".into(),
            parent_hash: String::new(),
            merge_parent: String::new(),
            created_at_ms: 1,
        }],
        file_map: rows
            .iter()
            .map(|p| FileMapRow {
                snapshot_hash: id.clone(),
                path: (*p).to_string(),
                hash: object.clone(),
                mode: 0,
            })
            .collect(),
        meta: vec![],
        tags: vec![],
        objects: vec![(object, frame)],
        renames,
        signatures: vec![],
    };
    (bundle, id)
}

fn apply_crafted(
    rows: &[&str],
    renames: Vec<RenameRow>,
) -> (crate::Result<bundle::Applied>, i64, usize) {
    let (_src_tmp, source) = setup();
    let (bundle, _) = crafted(&source, rows, renames);
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("hostile.velobundle");
    std::fs::write(&file, bundle::encode(&bundle)).unwrap();

    let (_tmp, dest) = setup();
    let result = {
        let guard = dest.write().unwrap();
        bundle::apply(&guard, &file)
    };
    let snapshots = count(&dest, "snapshots");
    let objects = dest.objects().list().unwrap().len();
    (result, snapshots, objects)
}

#[test]
fn a_bundle_with_an_escaping_row_imports_nothing() {
    let (result, snapshots, objects) = apply_crafted(&["../escape"], vec![]);
    assert_unsafe(result, "../escape");
    assert_eq!(snapshots, 0);
    assert_eq!(objects, 0);
}

#[test]
fn a_bundle_naming_the_repository_directory_imports_nothing() {
    let (result, snapshots, objects) = apply_crafted(&["ok.txt", ".velo/config"], vec![]);
    assert_unsafe(result, ".velo/config");
    assert_eq!((snapshots, objects), (0, 0));
}

#[test]
fn a_bundle_with_a_file_and_directory_collision_imports_nothing() {
    let (result, snapshots, objects) = apply_crafted(&["a", "a/b"], vec![]);
    assert_unsafe(result, "a/b");
    assert_eq!((snapshots, objects), (0, 0));
}

#[test]
fn a_bundle_with_an_unsafe_rename_edge_imports_nothing() {
    let (result, snapshots, objects) = apply_crafted(
        &["ok.txt"],
        vec![RenameRow {
            snapshot_hash: "x".into(),
            from_path: "ok.txt".into(),
            to_path: "../out".into(),
        }],
    );
    assert_unsafe(result, "../out");
    assert_eq!((snapshots, objects), (0, 0));
}

#[test]
fn a_safe_bundle_still_applies() {
    let (result, snapshots, _) = apply_crafted(&["a/b.txt", "c"], vec![]);
    result.unwrap();
    assert_eq!(snapshots, 1);
}

#[test]
fn git_import_refuses_an_escaping_path() {
    let (_tmp, repo) = setup();
    let body = "hi\n";
    let stream = format!(
        "commit refs/heads/main\nmark :1\nauthor a <a@x.org> 1000 +0000\ncommitter a <a@x.org> 1000 +0000\ndata 3\nmsg\nM 100644 inline ../x\ndata {}\n{}\n",
        body.len(),
        body
    );
    let guard = repo.write().unwrap();
    let err = import::git_fast_export(&guard, &mut stream.as_bytes(), Default::default());
    assert_unsafe(err, "../x");
    drop(guard);
    assert_eq!(count(&repo, "snapshots"), 0);
}

fn listing(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = format!("{prefix}{}", e.file_name().to_string_lossy());
            if name == ".velo" {
                continue;
            }
            let path = e.path();
            out.push(name.clone());
            if path.is_dir() && !e.file_type().unwrap().is_symlink() {
                walk(&path, &format!("{name}/"), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out
}

/// A repository one level below `outer`, so the directory above the root can
/// be listed without other tests' temporary files in it.
fn nested() -> (TempDir, PathBuf, Repo) {
    let outer = TempDir::new().unwrap();
    let root = outer.path().join("root");
    std::fs::create_dir(&root).unwrap();
    crate::commands::init::run(&root).unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    (outer, root, repo)
}

/// Inject a `file_map` row the public API would refuse.
fn inject(repo: &Repo, snapshot: &SnapshotId, path: &str) {
    let guard = repo.write().unwrap();
    let (hash, mode): (String, i64) = guard
        .conn()
        .query_row(
            "SELECT hash, mode FROM file_map WHERE snapshot_hash = ? LIMIT 1",
            [snapshot.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    guard
        .conn()
        .execute(
            "INSERT INTO file_map (snapshot_hash, path, hash, mode) VALUES (?, ?, ?, ?)",
            rusqlite::params![snapshot.as_str(), path, hash, mode],
        )
        .unwrap();
}

#[test]
fn restore_of_an_injected_escaping_row_writes_nothing() {
    let (outer, root, repo) = nested();
    let a = save(
        &repo,
        None,
        vec![TreeEntry::file("a.txt", b"a\n".to_vec())],
        &[],
    )
    .unwrap();
    let b = save(
        &repo,
        Some(&a),
        vec![TreeEntry::file("b.txt", b"b\n".to_vec())],
        &[],
    )
    .unwrap();
    inject(&repo, &b, "../escape");

    let before_root = listing(&root);
    let before_outer = listing(outer.path());
    let guard = repo.write().unwrap();
    let err = restore::run(
        &guard,
        &b,
        restore::Options {
            force: true,
            ..Default::default()
        },
    );
    assert_unsafe(err, "../escape");
    drop(guard);
    assert_eq!(listing(&root), before_root);
    assert_eq!(listing(outer.path()), before_outer);
    assert!(!outer.path().join("escape").exists());
}

#[test]
fn switch_of_an_injected_repository_directory_row_writes_nothing() {
    let (_outer, root, repo) = nested();
    let a = save(
        &repo,
        None,
        vec![TreeEntry::file("a.txt", b"a\n".to_vec())],
        &[],
    )
    .unwrap();
    inject(&repo, &a, ".velo/config");
    let guard = repo.write().unwrap();
    let before = listing(&root);
    let err = restore::run(
        &guard,
        &a,
        restore::Options {
            force: true,
            ..Default::default()
        },
    );
    assert_unsafe(err, ".velo/config");
    drop(guard);
    assert_eq!(listing(&root), before);
}

#[test]
fn an_old_repository_holding_a_bad_row_stays_readable() {
    let (_outer, _root, repo) = nested();
    let a = save(
        &repo,
        None,
        vec![TreeEntry::file("a.txt", b"a\n".to_vec())],
        &[],
    )
    .unwrap();
    inject(&repo, &a, "../escape");
    let tree = repo.tree_at(&a).unwrap();
    assert!(tree.iter().any(|f| f.path == "../escape"));
}

#[cfg(unix)]
#[test]
fn restore_refuses_a_symlink_pointing_outside_the_root() {
    let (outer, root, repo) = nested();
    let a = save(
        &repo,
        None,
        vec![
            TreeEntry::file("a.txt", b"a\n".to_vec()),
            TreeEntry::symlink("link", "../../outside"),
        ],
        &[],
    )
    .unwrap();
    let b = save(
        &repo,
        Some(&a),
        vec![TreeEntry::file("b.txt", b"b\n".to_vec())],
        &[],
    )
    .unwrap();
    let _ = b;
    let before = listing(&root);
    let guard = repo.write().unwrap();
    let err = restore::run(
        &guard,
        &a,
        restore::Options {
            force: true,
            ..Default::default()
        },
    );
    let problem = assert_unsafe(err, "link");
    assert!(matches!(problem, PathProblem::SymlinkEscapes { .. }));
    drop(guard);
    assert_eq!(listing(&root), before);
    assert!(!outer.path().join("link").exists());
}

#[cfg(unix)]
#[test]
fn restore_refuses_to_write_through_a_working_tree_symlink() {
    let (outer, root, repo) = nested();
    let outside = outer.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let a = save(
        &repo,
        None,
        vec![TreeEntry::file("d/x", b"x\n".to_vec())],
        &[],
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, root.join("d")).unwrap();

    let guard = repo.write().unwrap();
    let err = restore::run(
        &guard,
        &a,
        restore::Options {
            force: true,
            ..Default::default()
        },
    );
    let problem = assert_unsafe(err, "d/x");
    assert!(matches!(problem, PathProblem::ThroughSymlink { .. }));
    drop(guard);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}
