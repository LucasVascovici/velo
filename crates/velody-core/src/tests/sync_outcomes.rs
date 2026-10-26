//! Sync outcomes say what happened: ahead-only versus diverged, a refused push
//! refreshing the tracking ref, and `status` knowing about a pending merge.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::status::{self, Tracking};
use crate::commands::sync::{self, Pulled};
use crate::commands::{self, merge};
use crate::error::VelodyError;
use crate::transport::Spawn;
use crate::Repo;

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn write(root: &Path, rel: &str, content: &str) {
    fs::write(root.join(rel), content).unwrap();
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

fn pull(root: &Path) -> Pulled {
    with_write(root, |g| {
        sync::pull(g, "origin", &Spawn::new("velody"), Default::default())
    })
    .unwrap()
}

fn parent(root: &Path) -> String {
    fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap()
        .trim()
        .to_string()
}

fn remote_ref(root: &Path) -> Option<String> {
    let repo = Repo::open_and_migrate(root).unwrap();
    crate::commands::tracking_ref(repo.conn(), "main").map(|(_, h)| h)
}

/// An origin with one snapshot and a clone of it: (holder, origin, clone).
fn origin_and_clone() -> (TempDir, PathBuf, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    write(&origin, "a.txt", "shared\n");
    save(&origin, "base");
    let copy = holder.path().join("copy");
    sync::clone(
        origin.to_str().unwrap(),
        &Spawn::new("velody"),
        sync::CloneOptions {
            dir: Some(copy.as_path()),
            observer: None,
            ..Default::default()
        },
    )
    .unwrap();
    (holder, origin, copy)
}

#[test]
fn pull_when_only_ahead_is_not_divergence() {
    let (_h, origin, copy) = origin_and_clone();
    let origin_tip = parent(&origin);
    write(&copy, "b.txt", "one\n");
    save(&copy, "one");
    write(&copy, "b.txt", "two\n");
    let ours = save(&copy, "two");

    let pulled = pull(&copy);
    assert_eq!(
        pulled,
        Pulled::Ahead {
            branch: "main".into(),
            remote: "origin".into(),
            ahead: 2
        }
    );
    assert_eq!(parent(&copy), ours, "the branch must not move");
    assert_eq!(fs::read_to_string(copy.join("b.txt")).unwrap(), "two\n");
    assert_eq!(remote_ref(&copy), Some(origin_tip));
}

#[test]
fn pull_when_diverged_reports_both_counts() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "theirs.txt", "t\n");
    save(&origin, "theirs");
    write(&copy, "ours.txt", "o\n");
    save(&copy, "ours");

    assert_eq!(
        pull(&copy),
        Pulled::Diverged {
            branch: "main".into(),
            remote: "origin".into(),
            ahead: 1,
            behind: 1
        }
    );
}

#[test]
fn a_refused_push_refreshes_the_tracking_ref() {
    let (_h, origin, copy) = origin_and_clone();
    write(&origin, "theirs.txt", "t\n");
    let remote_tip = save(&origin, "theirs");
    write(&copy, "ours.txt", "o\n");
    save(&copy, "ours");

    let err = with_write(&copy, |g| {
        sync::push(g, "origin", None, &Spawn::new("velody"), Default::default())
    })
    .unwrap_err();
    assert!(matches!(err, VelodyError::NotFastForward { .. }));

    assert_eq!(remote_ref(&copy), Some(remote_tip));
    let repo = Repo::open_and_migrate(&copy).unwrap();
    let st = status::run(&repo, &[]).unwrap();
    assert!(
        !matches!(
            st.tracking,
            Tracking::Known {
                ahead,
                behind: 0,
                ..
            } if ahead > 0
        ),
        "status must not advertise a push that will be refused: {:?}",
        st.tracking
    );
}

#[test]
fn status_knows_the_pending_merge() {
    let holder = TempDir::new().unwrap();
    let root = holder.path().to_path_buf();
    commands::init::run(&root).unwrap();
    write(&root, "f.txt", "base\n");
    save(&root, "base");
    with_write(&root, |g| commands::switch::run(g, "feature", false)).unwrap();
    write(&root, "f.txt", "feature\n");
    save(&root, "feature edit");
    with_write(&root, |g| commands::switch::run(g, "main", true)).unwrap();
    write(&root, "f.txt", "main\n");
    save(&root, "main edit");

    let repo = Repo::open_and_migrate(&root).unwrap();
    assert_eq!(status::run(&repo, &[]).unwrap().merging, None);

    with_write(&root, |g| {
        merge::run(g, merge::Mode::Bring { source: "feature" })
    })
    .unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    assert_eq!(
        status::run(&repo, &[]).unwrap().merging.as_deref(),
        Some("feature")
    );

    with_write(&root, |g| merge::run(g, merge::Mode::Abort)).unwrap();
    let repo = Repo::open_and_migrate(&root).unwrap();
    assert_eq!(status::run(&repo, &[]).unwrap().merging, None);
}
