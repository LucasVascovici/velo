//! Restore seeds `index_cache` for the files it writes (15.5-a).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

use crate::commands;
use crate::Repo;

fn setup() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    commands::init::run(tmp.path()).unwrap();
    let root = tmp.path().to_path_buf();
    (tmp, root)
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(p, content).unwrap();
}

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).unwrap();
    let guard = repo.write().unwrap();
    f(&guard)
}

fn save(root: &Path, msg: &str) -> String {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap()
    .into_result()
    .unwrap()
    .hash
    .into_string()
}

fn dirty(root: &Path) -> HashMap<String, commands::FileStatus> {
    let repo = Repo::open_and_migrate(root).unwrap();
    commands::get_dirty_files(&repo)
}

fn cache_rows(root: &Path) -> HashMap<String, (i64, i64, String)> {
    let repo = Repo::open_and_migrate(root).unwrap();
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT path, mtime_ns, size, hash FROM index_cache")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn tree(root: &Path, snapshot: &str) -> HashMap<String, String> {
    let repo = Repo::open_and_migrate(root).unwrap();
    let conn = repo.conn();
    let mut stmt = conn
        .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")
        .unwrap();
    stmt.query_map([snapshot], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Every path of `snapshot` has a cache row matching both the tree and the disk.
fn assert_seeded(root: &Path, snapshot: &str) {
    let rows = cache_rows(root);
    let files = tree(root, snapshot);
    assert!(!files.is_empty());
    for (path, hash) in files {
        let (mtime, size, cached) = rows
            .get(&path)
            .unwrap_or_else(|| panic!("no cache row for {path}"));
        assert_eq!(cached, &hash, "hash for {path}");
        let meta = fs::symlink_metadata(root.join(&path)).unwrap();
        assert_eq!(*size, meta.len() as i64, "size for {path}");
        let on_disk = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        assert_eq!(*mtime, on_disk, "mtime for {path}");
    }
}

fn two_saves(root: &Path) -> (String, String) {
    write(root, "a.txt", "one");
    write(root, "dir/b.txt", "bee");
    let s1 = save(root, "s1");
    write(root, "a.txt", "two two");
    write(root, "dir/b.txt", "bee!");
    let s2 = save(root, "s2");
    (s1, s2)
}

#[test]
fn undo_seeds_cache_and_status_is_clean() {
    let (_tmp, root) = setup();
    let (s1, _) = two_saves(&root);
    with_write(&root, commands::undo::run).unwrap();
    assert_seeded(&root, &s1);
    assert!(dirty(&root).is_empty());
}

#[test]
fn redo_seeds_cache() {
    let (_tmp, root) = setup();
    let (_, s2) = two_saves(&root);
    with_write(&root, commands::undo::run).unwrap();
    with_write(&root, commands::redo::run).unwrap();
    assert_seeded(&root, &s2);
    assert!(dirty(&root).is_empty());
}

#[test]
fn switch_seeds_cache() {
    let (_tmp, root) = setup();
    let (_, s2) = two_saves(&root);
    with_write(&root, |g| commands::switch::run(g, "other", false)).unwrap();
    write(&root, "c.txt", "see");
    save(&root, "s3");
    with_write(&root, |g| commands::switch::run(g, "main", false)).unwrap();
    assert_seeded(&root, &s2);
    assert!(dirty(&root).is_empty());
}

#[test]
fn modified_after_restore_is_still_detected() {
    let (_tmp, root) = setup();
    two_saves(&root);
    with_write(&root, commands::undo::run).unwrap();
    write(&root, "a.txt", "a much longer body than before");
    let d = dirty(&root);
    assert!(d.contains_key("a.txt"), "{d:?}");
    assert!(!d.contains_key("dir/b.txt"));
}

#[test]
fn failed_restore_leaves_no_stale_row_for_the_failed_path() {
    let (_tmp, root) = setup();
    let (s1, _) = two_saves(&root);
    // Warm the cache for the current (s2) content.
    assert!(dirty(&root).is_empty());
    // Replace a.txt with a directory so writing the s1 content over it fails.
    fs::remove_file(root.join("a.txt")).unwrap();
    fs::create_dir(root.join("a.txt")).unwrap();
    fs::write(root.join("a.txt").join("x"), "x").unwrap();
    let r = with_write(&root, |g| {
        commands::restore::run(
            g,
            &crate::SnapshotId::from_stored(s1.clone()),
            commands::restore::Options {
                force: true,
                ..Default::default()
            },
        )
    });
    assert!(r.is_err(), "restore should fail on the obstructed path");
    let rows = cache_rows(&root);
    assert!(!rows.contains_key("a.txt"), "stale row for failed path");
    // The file that did land is seeded with the s1 hash.
    assert_eq!(
        rows.get("dir/b.txt").map(|r| &r.2),
        tree(&root, &s1).get("dir/b.txt")
    );
}

// ─── 15.5-b: restore writes only the delta ───────────────────────────────────

use commands::restore::{Options as RestoreOptions, Outcome, Strategy as Strat};
use std::collections::BTreeMap;

fn restore_with(root: &Path, snapshot: &str, strategy: Strat) -> Outcome {
    with_write(root, |g| {
        commands::restore::run_with(
            g,
            &crate::SnapshotId::from_stored(snapshot.to_string()),
            RestoreOptions {
                force: true,
                ..Default::default()
            },
            strategy,
            None,
        )
    })
    .unwrap()
}

fn written(outcome: &Outcome) -> usize {
    match outcome {
        Outcome::Restored { files, .. } => *files,
        other => panic!("expected a full restore, got {other:?}"),
    }
}

#[test]
fn undo_of_one_file_among_fifty_writes_one() {
    let (_tmp, root) = setup();
    for i in 0..50 {
        write(&root, &format!("d{}/f{i}.txt", i % 5), &format!("body {i}"));
    }
    let s1 = save(&root, "s1");
    write(&root, "d0/f0.txt", "changed body");
    save(&root, "s2");
    let out = restore_with(&root, &s1, Strat::Delta);
    // The 50 files plus whatever `init` tracks; only the changed one is written.
    assert_eq!(written(&out), 1);
    assert_eq!(
        fs::read_to_string(root.join("d0/f0.txt")).unwrap(),
        "body 0"
    );
    assert!(dirty(&root).is_empty());

    // The reference strategy still writes everything.
    write(&root, "d0/f0.txt", "changed body");
    let s2 = save(&root, "s2b");
    let _ = s2;
    let out = restore_with(&root, &s1, Strat::RewriteAll);
    assert_eq!(written(&out), tree(&root, &s1).len());
    assert!(written(&out) >= 50);
}

#[test]
fn untracked_and_ignored_files_survive_restore_and_undo() {
    let (_tmp, root) = setup();
    write(
        &root,
        ".veloignore",
        "*.log
",
    );
    write(&root, "a.txt", "one");
    let s1 = save(&root, "s1");
    write(&root, "ghost.txt", "only in s2");
    write(&root, "sub/ghost2.txt", "only in s2");
    save(&root, "s2");
    write(&root, "untracked.txt", "mine");
    write(&root, "sub/untracked2.txt", "mine too");
    write(&root, "noise.log", "ignored");

    // Restore drops the tracked files the target lacks and nothing else.
    let out = restore_with(&root, &s1, Strat::Delta);
    assert!(matches!(
        out,
        Outcome::Restored {
            ghosts_removed: 2,
            ..
        }
    ));
    assert!(!root.join("ghost.txt").exists());
    assert!(!root.join("sub/ghost2.txt").exists());
    for keep in ["untracked.txt", "sub/untracked2.txt", "noise.log"] {
        assert!(root.join(keep).exists(), "{keep} was deleted by restore");
    }

    // The same through undo, which hands its own scan to the restore.
    write(&root, "late.txt", "only in s3");
    save(&root, "s3");
    // Undo refuses a tree with untracked files, so only the ignored one is
    // left to survive it.
    with_write(&root, commands::undo::run).unwrap();
    assert!(!root.join("late.txt").exists());
    assert!(
        root.join("noise.log").exists(),
        "noise.log was deleted by undo"
    );
}

#[test]
fn force_overwrites_a_dirty_file_unchanged_between_the_trees() {
    let (_tmp, root) = setup();
    write(&root, "keep.txt", "original");
    write(&root, "move.txt", "one");
    let s1 = save(&root, "s1");
    write(&root, "move.txt", "two");
    save(&root, "s2");
    write(&root, "keep.txt", "hand edited, longer");
    let out = restore_with(&root, &s1, Strat::Delta);
    assert_eq!(
        fs::read_to_string(root.join("keep.txt")).unwrap(),
        "original"
    );
    assert_eq!(written(&out), 2);
    assert!(dirty(&root).is_empty());
}

#[test]
fn edit_back_to_original_is_clean_and_skipped() {
    let (_tmp, root) = setup();
    write(&root, "keep.txt", "original");
    write(&root, "move.txt", "one");
    let s1 = save(&root, "s1");
    write(&root, "move.txt", "two");
    save(&root, "s2");
    write(&root, "keep.txt", "scribble");
    write(&root, "keep.txt", "original");
    let out = restore_with(&root, &s1, Strat::Delta);
    assert_eq!(written(&out), 1);
    assert_eq!(
        fs::read_to_string(root.join("keep.txt")).unwrap(),
        "original"
    );
}

#[test]
fn deleted_file_unchanged_between_trees_is_written_back() {
    let (_tmp, root) = setup();
    write(&root, "keep.txt", "original");
    write(&root, "move.txt", "one");
    let s1 = save(&root, "s1");
    write(&root, "move.txt", "two");
    save(&root, "s2");
    fs::remove_file(root.join("keep.txt")).unwrap();
    restore_with(&root, &s1, Strat::Delta);
    assert_eq!(
        fs::read_to_string(root.join("keep.txt")).unwrap(),
        "original"
    );
}

#[test]
fn file_and_directory_clash_in_both_directions() {
    let (_tmp, root) = setup();
    write(&root, "a", "file");
    write(&root, "z.txt", "z");
    let s1 = save(&root, "file");
    fs::remove_file(root.join("a")).unwrap();
    write(&root, "a/b", "nested");
    let s2 = save(&root, "dir");
    restore_with(&root, &s1, Strat::Delta);
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "file");
    restore_with(&root, &s2, Strat::Delta);
    assert_eq!(fs::read_to_string(root.join("a/b")).unwrap(), "nested");
    assert!(dirty(&root).is_empty());
}

#[cfg(unix)]
#[test]
fn chmod_and_symlink_changes_are_restored() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, root) = setup();
    write(&root, "run.sh", "#!/bin/sh");
    write(&root, "other.txt", "o");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("other.txt", root.join("link")).unwrap();
    let s1 = save(&root, "s1");
    write(&root, "other.txt", "o2");
    save(&root, "s2");
    // A chmod does not change the hash, so the scan alone cannot see it.
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(root.join("link")).unwrap();
    fs::write(root.join("link"), "other.txt").unwrap();
    restore_with(&root, &s1, Strat::Delta);
    let mode = fs::metadata(root.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
    assert_eq!(
        fs::read_link(root.join("link")).unwrap(),
        Path::new("other.txt")
    );
}

// ─── Property: Delta is byte-identical to RewriteAll ─────────────────────────

use proptest::prelude::*;

const UNIVERSE: [&str; 6] = ["a", "a/b", "c", "d/e", "d/f", "g/h/i"];
const BODIES: [&str; 3] = ["x", "yy", "zzz"];

/// path -> (body index, mode: 0 regular, 1 exec, 2 symlink)
type Spec = BTreeMap<&'static str, (usize, u8)>;

fn spec(slots: &[Option<(usize, u8)>]) -> Spec {
    let mut m = Spec::new();
    for (path, slot) in UNIVERSE.iter().zip(slots) {
        if let Some((body, mode)) = *slot {
            let mode = if cfg!(unix) { mode } else { 0 };
            m.insert(path, (body, mode));
        }
    }
    // A path cannot be a file in one place and a directory in the same tree.
    if m.contains_key("a") {
        m.remove("a/b");
    }
    m
}

fn clear(root: &Path) {
    for e in fs::read_dir(root).unwrap() {
        let e = e.unwrap();
        if e.file_name() == ".velo" {
            continue;
        }
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            fs::remove_dir_all(e.path()).unwrap();
        } else {
            fs::remove_file(e.path()).unwrap();
        }
    }
}

fn materialise(root: &Path, spec: &Spec) {
    clear(root);
    for (path, (body, mode)) in spec {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if *mode == 2 {
                std::os::unix::fs::symlink(BODIES[*body], &full).unwrap();
                continue;
            }
            fs::write(&full, BODIES[*body]).unwrap();
            if *mode == 1 {
                fs::set_permissions(&full, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            fs::write(&full, BODIES[*body]).unwrap();
        }
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            copy_tree(&e.path(), &dest);
        } else if ft.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(e.path()).unwrap(), &dest).unwrap();
        } else {
            fs::copy(e.path(), &dest).unwrap();
        }
    }
}

/// kind/content/mode of every entry outside `.velo`.
fn snapshot_of_disk(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            if e.file_name() == ".velo" && dir == root {
                continue;
            }
            let rel = e
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let meta = fs::symlink_metadata(e.path()).unwrap();
            if meta.is_dir() {
                walk(&e.path(), root, out);
            } else if meta.file_type().is_symlink() {
                let t = fs::read_link(e.path()).unwrap();
                out.insert(rel, format!("link {}", t.display()));
            } else {
                #[cfg(unix)]
                let exec = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let exec = false;
                let body = fs::read(e.path()).unwrap();
                out.insert(rel, format!("file exec={exec} {body:?}"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Apply one dirty operation to a tracked path on disk, where it still exists.
fn dirty_op(root: &Path, path: &str, op: u8) {
    let full = root.join(path);
    // A universe path can be a directory in this tree (`a` when `a/b` exists).
    let Ok(meta) = fs::symlink_metadata(&full) else {
        return;
    };
    if meta.is_dir() {
        return;
    }
    let is_link = meta.file_type().is_symlink();
    match op {
        1 if !is_link => fs::write(&full, "edited body!!").unwrap(),
        2 if !is_link => {
            let original = fs::read(&full).unwrap();
            fs::write(&full, "scribble scribble").unwrap();
            fs::write(&full, original).unwrap();
        }
        3 => {
            fs::remove_file(&full).unwrap();
            // Deleting the last file of a directory and leaving it empty makes
            // a later file-for-directory swap fail in `restore` whichever
            // strategy runs (pre-existing), so the test tidies up like an
            // editor would and does not conflate the two.
            let mut dir = full.parent();
            while let Some(d) = dir.filter(|d| *d != root) {
                if fs::remove_dir(d).is_err() {
                    break;
                }
                dir = d.parent();
            }
        }
        #[cfg(unix)]
        4 if !is_link => {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            fs::set_permissions(&full, fs::Permissions::from_mode(mode ^ 0o111)).unwrap();
        }
        _ => {}
    }
}

fn slots() -> impl Strategy<Value = Vec<Option<(usize, u8)>>> {
    prop::collection::vec(prop::option::of((0usize..3, 0u8..3)), UNIVERSE.len())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn delta_restore_matches_rewrite_all(
        left in slots(),
        right in slots(),
        ops in prop::collection::vec(0u8..5, UNIVERSE.len()),
        untracked in any::<bool>(),
    ) {
        let (left, right) = (spec(&left), spec(&right));
        prop_assume!(!left.is_empty() && !right.is_empty() && left != right);

        let (tmp, root) = setup();
        materialise(&root, &left);
        let s1 = save(&root, "left");
        materialise(&root, &right);
        save(&root, "right");

        for (path, op) in UNIVERSE.iter().zip(&ops) {
            dirty_op(&root, path, *op);
        }
        if untracked {
            write(&root, "untracked.txt", "not in any tree");
        }

        let other = TempDir::new().unwrap();
        let copy = other.path().join("copy");
        copy_tree(&root, &copy);

        restore_with(&root, &s1, Strat::Delta);
        restore_with(&copy, &s1, Strat::RewriteAll);

        prop_assert_eq!(snapshot_of_disk(&root), snapshot_of_disk(&copy));
        drop(tmp);
    }
}
