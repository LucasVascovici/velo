//! `sync::published`: does a rewrite touch snapshots a remote already has?

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::commands::{resolve_snapshot_id, save, show, sync};
use crate::{Repo, SnapshotId};

fn save_file(root: &Path, content: &str, msg: &str) {
    fs::write(root.join("f.txt"), content).unwrap();
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    save::run(&guard, Some(msg), save::Options::default()).unwrap();
}

fn tip(repo: &Repo) -> SnapshotId {
    resolve_snapshot_id(repo, "main").unwrap()
}

#[test]
fn pushed_tip_is_published_and_local_only_is_not() {
    let remote = TempDir::new().unwrap();
    crate::commands::init::run(remote.path()).unwrap();
    let local = TempDir::new().unwrap();
    crate::commands::init::run(local.path()).unwrap();
    save_file(local.path(), "one", "one");

    let repo = Repo::open_and_migrate(local.path()).unwrap();
    {
        let g = repo.write().unwrap();
        crate::commands::remote::add(&g, "origin", remote.path().to_str().unwrap()).unwrap();
        sync::push(
            &g,
            "origin",
            None,
            &crate::transport::Spawn::new("velo"),
            Default::default(),
        )
        .unwrap();
    }
    let pushed_tip = tip(&repo);
    assert_eq!(
        sync::published(&repo, &pushed_tip, None)
            .unwrap()
            .as_deref(),
        Some("origin/main")
    );

    save_file(local.path(), "two", "two");
    let repo = Repo::open_and_migrate(local.path()).unwrap();
    let new_tip = tip(&repo);
    // A base that excludes everything pushed leaves only the local snapshot.
    let base = show::run(&repo, &new_tip, &[]).unwrap().parent.unwrap();
    assert_eq!(sync::published(&repo, &new_tip, Some(&base)).unwrap(), None);
    // Without a base the pushed history underneath is at stake.
    assert_eq!(
        sync::published(&repo, &new_tip, None).unwrap().as_deref(),
        Some("origin/main")
    );
}
