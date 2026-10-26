//! `push_report` names the side branches a push publishes because the pushed
//! branch merged them.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::commands::sync::{self, Pushed};
use crate::commands::{self, merge};
use crate::transport::Spawn;
use crate::Repo;

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn save(root: &Path, file: &str, content: &str, msg: &str) {
    fs::write(root.join(file), content).unwrap();
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .expect("expected a snapshot");
}

fn origin_and_clone() -> (TempDir, PathBuf) {
    let holder = TempDir::new().unwrap();
    let origin = holder.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    commands::init::run(&origin).unwrap();
    save(&origin, "a.txt", "shared\n", "base");
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
    (holder, copy)
}

fn push(root: &Path) -> sync::PushReport {
    with_write(root, |g| {
        sync::push_report(g, "origin", None, &Spawn::new("velody"), Default::default())
    })
    .unwrap()
}

#[test]
fn a_merged_scratch_branch_is_reported_as_published() {
    let (_h, copy) = origin_and_clone();
    with_write(&copy, |g| commands::switch::run(g, "attempt-1", false)).unwrap();
    save(&copy, "s.txt", "scratch\n", "attempt");
    with_write(&copy, |g| commands::switch::run(g, "main", true)).unwrap();
    save(&copy, "m.txt", "main\n", "main work");
    with_write(&copy, |g| {
        merge::run(
            g,
            merge::Mode::Bring {
                source: "attempt-1",
            },
        )
    })
    .unwrap();
    save(&copy, "n.txt", "after\n", "merge attempt-1");

    let report = push(&copy);
    assert!(matches!(report.pushed, Pushed::Sent { .. }));
    assert_eq!(report.also_published, vec!["attempt-1".to_string()]);
}

#[test]
fn a_plain_push_publishes_no_side_branch() {
    let (_h, copy) = origin_and_clone();
    save(&copy, "m.txt", "main\n", "main work");
    let report = push(&copy);
    assert!(matches!(report.pushed, Pushed::Sent { .. }));
    assert!(report.also_published.is_empty());

    let again = push(&copy);
    assert!(matches!(again.pushed, Pushed::AlreadyUpToDate { .. }));
    assert!(again.also_published.is_empty());
}
