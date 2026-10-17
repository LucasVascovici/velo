//! The lock's holder record: who holds the write lock, reported in `Locked`.

use std::time::Duration;
use tempfile::TempDir;

use crate::{Error, LockHolder, Repo};

fn setup() -> (TempDir, Repo) {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    let repo = Repo::open(tmp.path()).unwrap();
    (tmp, repo)
}

fn holder_file(tmp: &TempDir) -> std::path::PathBuf {
    tmp.path().join(".velo/lock.holder")
}

#[test]
fn busy_lock_names_the_holder() {
    let (tmp, a) = setup();
    let a = a.lock_label("velo save x");
    let b = Repo::open(tmp.path()).unwrap();
    let _guard = a.write().unwrap();

    match b.write().unwrap_err() {
        Error::Locked { held_by, holder } => {
            let h = holder.expect("holder recorded");
            assert_eq!(held_by, Some(std::process::id()));
            assert_eq!(h.pid, std::process::id());
            assert_eq!(h.command.as_deref(), Some("velo save x"));
            let now = crate::commands::snapshot_timestamp_ms();
            assert!((now - h.started_at_ms).abs() < 60_000);
        }
        other => panic!("expected Locked, got {other:?}"),
    }
    assert_eq!(
        b.lock_holder().unwrap().command.as_deref(),
        Some("velo save x")
    );
}

#[test]
fn holder_record_is_removed_on_release() {
    let (tmp, a) = setup();
    let a = a.lock_label("velo gc");
    let b = Repo::open(tmp.path()).unwrap();
    let guard = a.write().unwrap();
    assert!(holder_file(&tmp).exists());
    drop(guard);
    assert!(!holder_file(&tmp).exists());
    assert!(b.lock_holder().is_none());
    assert!(b.write().is_ok());
}

#[test]
fn stale_or_garbage_record_does_not_block() {
    let (tmp, repo) = setup();
    std::fs::write(holder_file(&tmp), "not a pid\nnope").unwrap();
    assert!(repo.lock_holder().is_none());
    let guard = repo.write().unwrap();
    // Overwritten with our own record.
    assert_eq!(repo.lock_holder().unwrap().pid, std::process::id());
    drop(guard);

    std::fs::write(holder_file(&tmp), "999999\n1\nold command").unwrap();
    assert_eq!(repo.lock_holder().unwrap().pid, 999_999);
    let _guard = repo.write().unwrap();
    assert_eq!(repo.lock_holder().unwrap().pid, std::process::id());
}

#[test]
fn single_file_repo_writes_a_sibling_holder_record() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("repo.velo");
    let repo = Repo::create_file(&db).unwrap().lock_label("embedder");
    let guard = repo.write().unwrap();
    let record = tmp.path().join("repo.velo.lock.holder");
    assert!(record.exists());
    assert_eq!(
        repo.lock_holder().unwrap().command.as_deref(),
        Some("embedder")
    );
    drop(guard);
    assert!(!record.exists());
}

#[test]
fn write_timeout_error_carries_the_holder() {
    let (tmp, a) = setup();
    let a = a.lock_label("velo merge");
    let b = Repo::open(tmp.path()).unwrap();
    let _guard = a.write().unwrap();
    match b.write_timeout(Duration::from_millis(30)).unwrap_err() {
        Error::Locked { holder, .. } => {
            assert_eq!(holder.unwrap().command.as_deref(), Some("velo merge"));
        }
        other => panic!("expected Locked, got {other:?}"),
    }
}

#[test]
fn display_names_the_holder() {
    let with_cmd = Error::Locked {
        held_by: Some(7),
        holder: Some(LockHolder {
            pid: 7,
            started_at_ms: 1,
            command: Some("velo save".into()),
        }),
    };
    assert_eq!(
        with_cmd.to_string(),
        "another Velo operation holds the lock (pid 7, 'velo save')"
    );
    let without_cmd = Error::Locked {
        held_by: Some(7),
        holder: Some(LockHolder {
            pid: 7,
            started_at_ms: 1,
            command: None,
        }),
    };
    assert_eq!(
        without_cmd.to_string(),
        "another Velo operation holds the lock (pid 7)"
    );
    let none = Error::Locked {
        held_by: None,
        holder: None,
    };
    assert_eq!(
        none.to_string(),
        "another Velo operation is in progress in this repository"
    );
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn write_waits_out_a_lock_that_is_being_released_without_a_holder_record() {
    use fs2::FileExt;
    let (tmp, repo) = setup();
    // Hold the OS lock without a holder record, as a forked child holding an
    // inherited descriptor would, and let go shortly after.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(tmp.path().join(".velo/lock"))
        .unwrap();
    file.lock_exclusive().unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        drop(file);
    });
    repo.write().expect("write waits for the release");
    release.join().unwrap();
}
