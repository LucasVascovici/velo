//! The Rust calls `docs/LIBRARY.md` tells a reader to make, run as written.
//!
//! The cold-start test (6.2.1): two agents learned the Rust API from compiler
//! errors because the page showed little beyond `save_tree`. This keeps what the
//! page now says true.

use std::path::Path;

use tempfile::TempDir;

use crate::commands::merge::{self, MergeCommit, Resolution};
use crate::commands::{blame, branches, history, tag};
use crate::tree::{Change, SaveChanges, SaveTree, TreeEntry};
use crate::{Author, BranchName, Repo, TagName};

fn save(
    repo: &Repo,
    branch: &BranchName,
    parent: Option<&crate::SnapshotId>,
    msg: &str,
    body: &str,
    author: Option<&Author>,
) -> crate::SnapshotId {
    repo.write()
        .unwrap()
        .save_tree(SaveTree {
            branch,
            parent,
            merge_parent: None,
            message: msg,
            entries: vec![TreeEntry::file("notes.md", body.as_bytes().to_vec())],
            meta: Default::default(),
            timestamp_ms: None,
            author,
            renames: &[],
        })
        .unwrap()
}

#[test]
fn the_documented_rust_calls_work_as_written() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path()).unwrap();
    let main: BranchName = "main".parse().unwrap();
    let ada = Author::new("ada").unwrap();

    let first = save(&repo, &main, None, "first", "one\n", Some(&ada));

    // save_changes returns Saved: the id is `.snapshot`.
    let mut spec = SaveChanges::new(&main, Some(&first), "edit");
    spec.changes = vec![Change::Put(TreeEntry::file("notes.md", b"two\n".to_vec()))];
    let second = repo.write().unwrap().save_changes(spec).unwrap().snapshot;

    // `parent: None` starts a new history holding only what is passed, and the
    // branch moves to it: the page tells the reader to pass the parent every time.
    let orphan_branch: BranchName = "orphan".parse().unwrap();
    let mut spec = SaveChanges::new(&orphan_branch, None, "root one");
    spec.changes = vec![Change::Put(TreeEntry::file(
        "a.txt",
        b"1
"
        .to_vec(),
    ))];
    let root_one = repo.write().unwrap().save_changes(spec).unwrap().snapshot;
    let mut spec = SaveChanges::new(&orphan_branch, None, "root two");
    spec.changes = vec![Change::Put(TreeEntry::file(
        "b.txt",
        b"2
"
        .to_vec(),
    ))];
    let root_two = repo.write().unwrap().save_changes(spec).unwrap().snapshot;
    assert_eq!(
        repo.branch_tip(&orphan_branch).unwrap().as_ref(),
        Some(&root_two)
    );
    let paths: Vec<String> = repo
        .tree_at(&root_two)
        .unwrap()
        .into_iter()
        .map(|f| f.path)
        .collect();
    assert_eq!(
        paths,
        ["b.txt"],
        "a.txt from {root_one} is not carried over"
    );

    // history: entries, ancestry with `from`, and the default with no position.
    let h = history::run(
        &repo,
        history::Options {
            from: Some(&second),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(h.entries.len(), 2);
    assert_eq!(
        history::run(&repo, history::Options::default())
            .unwrap()
            .entries
            .len(),
        2
    );

    // blame: origin is an Option<LineOrigin> whose author is an Option<Author>.
    let b = blame::run(
        &repo,
        Path::new("notes.md"),
        blame::Options {
            at: Some(&second),
            ..Default::default()
        },
    )
    .unwrap();
    let origin = b.lines[0].origin.as_ref().unwrap();
    assert!(origin.author.is_none(), "the edit was saved with no author");

    // A new branch by saving with a parent, and by branches::create.
    let side: BranchName = "side".parse().unwrap();
    let on_side = save(&repo, &side, Some(&first), "side", "side\n", None);
    let extra: BranchName = "extra".parse().unwrap();
    branches::create(&repo.write().unwrap(), &extra, Some(&first)).unwrap();
    assert_eq!(repo.branch_tip(&side).unwrap().as_ref(), Some(&on_side));
    assert_eq!(repo.branch_tip(&extra).unwrap().as_ref(), Some(&first));

    // Tags.
    let name: TagName = "v1".parse().unwrap();
    tag::create(&repo.write().unwrap(), &name, Some(&second), false).unwrap();

    // Rollback: a new snapshot carrying the old objects forward.
    let entries: Vec<TreeEntry> = repo
        .tree_at(&first)
        .unwrap()
        .into_iter()
        .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
        .collect();
    let back = repo
        .write()
        .unwrap()
        .save_tree(SaveTree {
            branch: &main,
            parent: Some(&second),
            merge_parent: None,
            message: "rollback",
            entries,
            meta: Default::default(),
            timestamp_ms: None,
            author: None,
            renames: &[],
        })
        .unwrap();
    assert_eq!(repo.read_file_at(&back, "notes.md").unwrap(), b"one\n");

    // Merge: plan writes nothing; commit needs a resolution for the conflict.
    let plan = merge::plan(&repo, &second, &on_side).unwrap();
    let conflicts: Vec<String> = plan.conflicts().map(|f| f.path.clone()).collect();
    assert_eq!(conflicts, ["notes.md"]);
    let refused = merge::commit(
        &repo.write().unwrap(),
        MergeCommit {
            branch: &main,
            ours: &second,
            theirs: &on_side,
            resolutions: &[],
            message: "merge",
            meta: Default::default(),
            author: None,
            timestamp_ms: None,
        },
    );
    assert!(
        matches!(refused, Err(crate::Error::Conflicts { .. })),
        "{refused:?}"
    );
    let merged = merge::commit(
        &repo.write().unwrap(),
        MergeCommit {
            branch: &main,
            ours: &second,
            theirs: &on_side,
            resolutions: &[(
                "notes.md".to_string(),
                Resolution::Content(b"both\n".to_vec()),
            )],
            message: "merge",
            meta: Default::default(),
            author: None,
            timestamp_ms: None,
        },
    )
    .unwrap();
    let detail = crate::commands::show::run(&repo, &merged, &[]).unwrap();
    assert_eq!(detail.parent.as_ref(), Some(&second));
    assert_eq!(detail.merge_parent.as_ref(), Some(&on_side));
}
