//! The default `history` of a directory repository written only through the
//! library walks back from the checked-out branch's tip.
//!
//! The cold-start test (6.2.1): every agent that built a repository with
//! `save_changes` and then ran `velody history` was told "No snapshots yet on
//! branch 'main'", while `velody branches` showed `main` with a tip. Library
//! writes never set `.velo/PARENT`, and the default scope read only that.

use tempfile::TempDir;

use crate::commands::history;
use crate::tree::{Change, SaveChanges, TreeEntry};
use crate::{BranchName, Repo};

#[test]
fn default_history_falls_back_to_the_branch_tip() {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    let main: BranchName = "main".parse().unwrap();
    let first = {
        let guard = repo.write().unwrap();
        let mut spec = SaveChanges::new(&main, None, "first");
        spec.changes = vec![Change::Put(TreeEntry::file("a.txt", b"a\n".to_vec()))];
        let first = guard.save_changes(spec).unwrap().snapshot;
        let mut spec = SaveChanges::new(&main, Some(&first), "second");
        spec.changes = vec![Change::Put(TreeEntry::file("a.txt", b"b\n".to_vec()))];
        guard.save_changes(spec).unwrap();
        first
    };
    assert!(
        !tmp.path().join(".velo/PARENT").exists()
            || std::fs::read_to_string(tmp.path().join(".velo/PARENT"))
                .unwrap()
                .trim()
                .is_empty(),
        "the premise: library writes leave no checked-out position"
    );

    let h = history::run(&repo, history::Options::default()).unwrap();
    let messages: Vec<&str> = h.entries.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(messages, ["second", "first"], "{h:?}");
    assert!(h.entries.iter().any(|e| e.hash == first));
}
