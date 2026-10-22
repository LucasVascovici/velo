//! `save_tree` checks only the `Stored` objects its parent does not name (22.3).

use crate::commands::{self, fsck::Problem};
use crate::tree::FileKind;
use crate::tree::{SaveTree, TreeEntry};
use crate::{db, BranchName, Error, ObjectHash, ObjectLocation, Repo, SnapshotId};
use crate::{InitOptions, SnapshotMeta};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn setup(location: ObjectLocation) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    commands::init::run_with(&path, InitOptions::new().objects(location)).unwrap();
    (tmp, path)
}

fn save(
    repo: &Repo,
    parent: Option<&SnapshotId>,
    entries: Vec<TreeEntry>,
) -> Result<SnapshotId, Error> {
    let branch: BranchName = "main".parse().unwrap();
    let guard = repo.write().unwrap();
    guard.save_tree(SaveTree {
        branch: &branch,
        parent,
        merge_parent: None,
        message: "m",
        entries,
        meta: SnapshotMeta::new(),
        author: None,
        timestamp_ms: Some(1_000),
        renames: &[],
    })
}

fn snapshot_rows(root: &Path) -> i64 {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    conn.query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
        .unwrap()
}

fn delete_object(root: &Path, location: ObjectLocation, hash: &str) {
    match location {
        ObjectLocation::Database => {
            let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
            conn.execute("DELETE FROM objects WHERE hash = ?", [hash])
                .unwrap();
        }
        _ => std::fs::remove_file(root.join(".velo/objects").join(hash)).unwrap(),
    }
}

const BOTH: [ObjectLocation; 2] = [ObjectLocation::Files, ObjectLocation::Database];

fn ghost() -> ObjectHash {
    ObjectHash::from_stored("ab".repeat(32))
}

#[test]
fn a_stored_object_in_neither_parent_nor_store_is_refused() {
    for location in BOTH {
        let (_tmp, root) = setup(location);
        let repo = Repo::open(&root).unwrap();
        let parent = save(&repo, None, vec![TreeEntry::file("a.txt", b"a\n".to_vec())]).unwrap();
        let before = snapshot_rows(&root);
        let err = save(
            &repo,
            Some(&parent),
            vec![
                TreeEntry::file("a.txt", b"a\n".to_vec()),
                TreeEntry::stored("ghost.txt", ghost(), FileKind::Regular),
            ],
        )
        .unwrap_err();
        assert!(
            matches!(&err, Error::MissingObject { hash } if hash == ghost().as_str()),
            "{location:?}: {err:?}"
        );
        assert_eq!(
            snapshot_rows(&root),
            before,
            "{location:?}: no snapshot row"
        );
    }
}

#[test]
fn a_root_save_with_a_missing_stored_object_is_refused() {
    for location in BOTH {
        let (_tmp, root) = setup(location);
        let repo = Repo::open(&root).unwrap();
        let before = snapshot_rows(&root);
        let err = save(
            &repo,
            None,
            vec![TreeEntry::stored("ghost.txt", ghost(), FileKind::Regular)],
        )
        .unwrap_err();
        assert!(matches!(err, Error::MissingObject { .. }), "{location:?}");
        assert_eq!(snapshot_rows(&root), before);
    }
}

#[test]
fn an_object_the_parent_references_is_trusted_and_fsck_reports_it() {
    for location in BOTH {
        let (_tmp, root) = setup(location);
        let repo = Repo::open(&root).unwrap();
        let parent = save(
            &repo,
            None,
            vec![
                TreeEntry::file("a.txt", b"alpha\n".to_vec()),
                TreeEntry::file("b.txt", b"beta\n".to_vec()),
            ],
        )
        .unwrap();
        let lost = repo
            .tree_at(&parent)
            .unwrap()
            .into_iter()
            .find(|f| f.path == "a.txt")
            .unwrap();
        delete_object(&root, location, lost.object.as_str());

        let child = save(
            &repo,
            Some(&parent),
            vec![
                TreeEntry::stored("a.txt", lost.object.clone(), FileKind::Regular),
                TreeEntry::file("b.txt", b"changed\n".to_vec()),
            ],
        );
        assert!(child.is_ok(), "{location:?}: {child:?}");

        let report = commands::fsck::check(&repo).unwrap();
        assert!(
            report.problems.iter().any(
                |p| matches!(p, Problem::MissingObject { hash } if hash == lost.object.as_str())
            ),
            "{location:?}: {:?}",
            report.problems
        );
    }
}
