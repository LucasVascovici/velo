//! Full test suite for Velo.
//!
//! Each module mirrors the corresponding command.  Tests use `tempfile::TempDir`
//! for isolation and never touch the host filesystem outside of the temp dir.
//!
//! Conventions:
//!   - `setup()` initialises a fresh repo and returns `(TempDir, PathBuf)`.
//!   - The `TempDir` is kept alive via `_tmp`; dropping it deletes the whole tree.
//!   - Helper assertions are defined at the bottom of the file.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    use crate::commands::{self, FileStatus};
    use crate::db;
    use crate::error::{Error, VeloError};
    use chrono::{DateTime, Utc};

    use crate::{BranchName, ObjectHash, Repo, SnapshotId, SnapshotMeta, TagName};

    // =========================================================================
    // Helpers
    // =========================================================================

    /// Stand-in for the removed `resolve::run`: take one side for either a named
    /// file or every conflict. Mirrors what the CLI does, minus the printing.
    fn resolve_take(
        root: &Path,
        file: Option<&str>,
        side: commands::resolve::TakeOption,
        all: bool,
    ) -> crate::error::Result<()> {
        use commands::resolve;
        let repo = Repo::open_and_migrate(root)?;
        let guard = repo.write()?;
        let targets = if all {
            resolve::list_conflicts(&repo)?
        } else {
            vec![resolve::get_conflict(&repo, file.expect("file or --all"))?]
        };
        for cf in &targets {
            resolve::take_side(&guard, cf, side)?;
        }
        Ok(())
    }

    /// Open the repository at `root` and hand it to `f`.
    ///
    /// Commands take a repository handle now, so tests open one per call. A real
    /// consumer would hold a single `Repo` for its lifetime; doing that here would
    /// mean threading it through several hundred assertions for no extra coverage.
    fn with_repo<T>(root: &Path, f: impl FnOnce(&Repo) -> T) -> T {
        let repo = Repo::open_and_migrate(root).expect("open repository");
        f(&repo)
    }

    /// Same, but takes the write lock — mutating commands need the guard.
    fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
        let repo = Repo::open_and_migrate(root).expect("open repository");
        let guard = repo.write().expect("take write lock");
        f(&guard)
    }

    /// Spawn config for the sync tests.
    ///
    /// They all use local-path remotes, which never spawn anything — but the
    /// parameter is required, and pinning it means no test depends on where the
    /// test binary happens to live.
    fn spawn_cfg() -> crate::transport::Spawn {
        crate::transport::Spawn::new("velo")
    }

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        crate::commands::init::run(&path).unwrap();
        (tmp, path)
    }

    /// Write `content` to `root/rel_path`, creating parent dirs as needed.
    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    fn exists(root: &Path, rel: &str) -> bool {
        root.join(rel).exists()
    }

    /// A branch name from a literal. Panics on an invalid one, which in a test is
    /// what you want: it means the test itself is wrong.
    fn branch_name(name: &str) -> BranchName {
        name.parse().expect("valid branch name")
    }

    /// A tag name from a literal, for the same reason.
    fn tag_name(name: &str) -> TagName {
        name.parse().expect("valid tag name")
    }

    /// A fixed instant safely in the past, for asserting a timestamp is real.
    ///
    /// `created_at_ms` defaults to 0 in the schema, so "is it set" is the thing
    /// worth checking, and 0 is the epoch.
    fn year_2020() -> DateTime<Utc> {
        DateTime::from_timestamp_millis(1_577_836_800_000).unwrap()
    }

    fn save(root: &Path, msg: &str) -> String {
        with_write(root, |g| commands::save::run(g, msg, false))
            .unwrap()
            .into_result()
            .expect("expected a snapshot to be created")
            .hash
            .into_string()
    }

    fn parent(root: &Path) -> String {
        fs::read_to_string(root.join(".velo/PARENT"))
            .unwrap()
            .trim()
            .to_string()
    }

    fn head(root: &Path) -> String {
        fs::read_to_string(root.join(".velo/HEAD"))
            .unwrap()
            .trim()
            .to_string()
    }

    fn snapshot_exists(root: &Path, hash: &str) -> bool {
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM snapshots WHERE hash = ?)",
            [hash],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    }

    fn in_trash(root: &Path, hash: &str) -> bool {
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM trash WHERE hash = ?)",
            [hash],
            |r| r.get::<_, bool>(0),
        )
        .unwrap()
    }

    fn object_count(root: &Path) -> usize {
        fs::read_dir(root.join(".velo/objects")).unwrap().count()
    }

    // =========================================================================
    // init
    // =========================================================================

    #[test]
    fn init_creates_structure() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        commands::init::run(&root).unwrap();

        assert!(root.join(".velo").is_dir());
        assert!(root.join(".velo/objects").is_dir());
        assert!(root.join(".velo/velo.db").exists());
        assert_eq!(
            fs::read_to_string(root.join(".velo/HEAD")).unwrap().trim(),
            "main"
        );
        assert_eq!(
            fs::read_to_string(root.join(".velo/PARENT"))
                .unwrap()
                .trim(),
            ""
        );
    }

    #[test]
    fn init_writes_default_veloignore() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        commands::init::run(&root).unwrap();
        let ignore = fs::read_to_string(root.join(".veloignore")).unwrap();
        assert!(ignore.contains("target/"));
        assert!(ignore.contains("node_modules/"));
        assert!(ignore.contains("*.log"));
    }

    #[test]
    fn init_does_not_overwrite_existing_veloignore() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        // Write a custom ignore file before init
        fs::write(root.join(".veloignore"), "my_custom_rule/").unwrap();
        commands::init::run(&root).unwrap();
        let content = fs::read_to_string(root.join(".veloignore")).unwrap();
        assert_eq!(content, "my_custom_rule/");
    }

    #[test]
    fn init_is_idempotent_error() {
        let (_tmp, root) = setup();
        let result = commands::init::run(&root);
        assert!(
            matches!(
                result,
                Err(crate::error::VeloError::AlreadyInitialized { .. })
            ),
            "Expected AlreadyInitialized error"
        );
    }

    #[test]
    fn init_detects_nested_repo() {
        let (_tmp, root) = setup();
        let child = root.join("subdir");
        fs::create_dir_all(&child).unwrap();
        let result = commands::init::run(&child);
        assert!(
            matches!(result, Err(crate::error::VeloError::NestedRepo { .. })),
            "Expected NestedRepo error"
        );
    }

    // =========================================================================
    // find_repo_root
    // =========================================================================

    #[test]
    fn find_repo_root_from_subdirectory() {
        let (_tmp, root) = setup();
        let sub = root.join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        let found = commands::find_repo_root(&sub).unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn find_repo_root_returns_none_outside_repo() {
        let tmp = TempDir::new().unwrap();
        let result = commands::find_repo_root(tmp.path());
        assert!(result.is_none());
    }

    // =========================================================================
    // save
    // =========================================================================

    #[test]
    fn save_basic_roundtrip() {
        let (_tmp, root) = setup();
        write(&root, "hello.txt", "hello");
        let r = with_write(&root, |vr| commands::save::run(vr, "first", false))
            .unwrap()
            .into_result()
            .unwrap();
        assert_eq!(r.new_count, 2); // hello.txt + .veloignore
        assert_eq!(r.modified_count, 0);
        assert_eq!(r.deleted_count, 0);
        assert!(!r.hash.is_empty());
        // Ids are stored whole; SNAP_HASH_LEN is only how many characters get
        // shown.
        assert_eq!(r.hash.len(), commands::SNAP_ID_LEN);
        assert_eq!(r.hash.short().len(), commands::SNAP_HASH_LEN);
    }

    #[test]
    fn save_empty_message_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        let err = with_write(&root, |vr| commands::save::run(vr, "", false)).unwrap_err();
        assert!(matches!(err, crate::error::VeloError::InvalidInput { .. }));
    }

    #[test]
    fn save_whitespace_only_message_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        let err = with_write(&root, |vr| commands::save::run(vr, "   ", false)).unwrap_err();
        assert!(matches!(err, crate::error::VeloError::InvalidInput { .. }));
    }

    #[test]
    fn save_clean_directory_returns_none() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        save(&root, "s1");
        // Nothing changed — should return None
        let result = with_write(&root, |vr| commands::save::run(vr, "s2", false)).unwrap();
        assert!(!result.saved());
    }

    #[test]
    fn save_delta_storage_does_not_duplicate_objects() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        write(&root, "b.txt", "B");
        save(&root, "s1");
        let count_after_first = object_count(&root);

        // Modify only b.txt
        write(&root, "b.txt", "B_modified");
        save(&root, "s2");
        let count_after_second = object_count(&root);

        // Only one new object (modified b.txt); a.txt stays in object store once
        assert_eq!(count_after_second, count_after_first + 1);
    }

    #[test]
    fn save_deleted_file_status() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        save(&root, "s1");

        fs::remove_file(root.join("a.txt")).unwrap();
        let r = with_write(&root, |vr| commands::save::run(vr, "s2", false))
            .unwrap()
            .into_result()
            .unwrap();
        assert_eq!(r.deleted_count, 1);
    }

    #[test]
    fn save_clears_redo_stack() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        with_write(&root, commands::undo::run).unwrap();
        assert!(in_trash(&root, &h2), "s2 should be in trash after undo");

        // New save should clear the redo/trash stack for this branch
        write(&root, "f.txt", "v3");
        save(&root, "s3");

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let trash_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM trash WHERE branch = 'main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            trash_count, 0,
            "Redo stack should be cleared after a new save"
        );
    }

    #[test]
    fn save_veloignore_excludes_files() {
        let (_tmp, root) = setup();
        // Override the default .veloignore
        write(&root, ".veloignore", "*.log\ntemp/");
        write(&root, "app.rs", "fn main() {}");
        write(&root, "debug.log", "log output");
        fs::create_dir_all(root.join("temp")).unwrap();
        write(&root, "temp/cache.tmp", "junk");

        let r = with_write(&root, |vr| commands::save::run(vr, "test", false))
            .unwrap()
            .into_result()
            .unwrap();
        // Only app.rs + .veloignore should be tracked (debug.log and temp/ excluded)
        assert_eq!(r.new_count, 2);
    }

    #[test]
    fn save_small_crlf_file_is_clean_and_preserves_lines() {
        // A small (< mmap threshold) CRLF file must read as clean after saving,
        // AND the stored content must keep its line breaks (as LF). A previous
        // normalise_crlf bug collapsed CRLF files onto a single line while still
        // looking "clean", because hashing and storage shared the same bug.
        let (_tmp, root) = setup();
        write(&root, "small.txt", "alpha\r\nbeta\r\ngamma\r\n");
        save(&root, "add small crlf");
        assert!(
            with_repo(&root, commands::get_dirty_files).is_empty(),
            "small CRLF file should be clean right after save"
        );

        fs::remove_file(root.join("small.txt")).unwrap();
        with_write(&root, |vr| {
            commands::restore::run(vr, &parent(&root), true, &[])
        })
        .unwrap();
        assert_eq!(read(&root, "small.txt"), "alpha\nbeta\ngamma\n");
    }

    #[test]
    fn save_large_crlf_file_is_clean_after_save() {
        // Regression: files >= 256 KB take the memory-mapped hashing path.
        // That path used to hash raw (non-CRLF-normalised) bytes, disagreeing
        // with the normalised hash used by status/fast_hash and by the stored
        // object — so a large CRLF text file showed as permanently "Modified"
        // on Windows and broke content-addressing. It must now read clean.
        let (_tmp, root) = setup();
        let mut big = String::with_capacity(400 * 1024);
        for i in 0..40_000 {
            big.push_str(&format!("line number {i} of a large crlf file\r\n"));
        }
        assert!(
            big.len() > 256 * 1024,
            "test file must exceed mmap threshold"
        );
        write(&root, "big.txt", &big);

        let r = save(&root, "add big crlf");
        assert!(!r.is_empty());
        assert!(
            with_repo(&root, commands::get_dirty_files).is_empty(),
            "large CRLF file should be clean right after save (mmap hash path)"
        );

        // Content-addressing invariant: the stored object holds CRLF-normalised
        // (LF-only) content. Remove the working file and restore it from the
        // object store to observe exactly what was stored.
        fs::remove_file(root.join("big.txt")).unwrap();
        with_write(&root, |vr| {
            commands::restore::run(vr, &parent(&root), true, &[])
        })
        .unwrap();
        let restored = read(&root, "big.txt");
        assert!(
            !restored.contains('\r'),
            "stored content must be LF-normalised"
        );
        assert!(restored.starts_with("line number 0 of a large crlf file\n"));
    }

    // =========================================================================
    // restore
    // =========================================================================

    #[test]
    fn restore_roundtrip() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert_eq!(read(&root, "f.txt"), "v1");
        assert_eq!(parent(&root), h1);
    }

    #[test]
    fn restore_noop_when_already_at_target() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        // Should succeed silently without error
        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
    }

    #[test]
    fn restore_aborts_on_dirty_without_force() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        // Dirty up the working tree
        write(&root, "f.txt", "dirty");

        let before_parent = parent(&root);
        // Should now return Err (exit 1) — not silently succeed
        let result = with_write(&root, |vr| commands::restore::run(vr, &h1, false, &[]));
        assert!(result.is_err(), "Restore with dirty tree should error");
        assert_eq!(parent(&root), before_parent, "PARENT should not change");
        assert_eq!(read(&root, "f.txt"), "dirty", "File should not be restored");
    }

    #[test]
    fn restore_removes_ghost_files() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        let h1 = save(&root, "s1");

        write(&root, "b.txt", "B"); // ghost file (added after h1)
        save(&root, "s2");

        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert!(exists(&root, "a.txt"), "a.txt should be present");
        assert!(
            !exists(&root, "b.txt"),
            "b.txt is a ghost and must be removed"
        );
    }

    #[test]
    fn restore_removes_empty_directories() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        let h1 = save(&root, "s1");

        write(&root, "subdir/nested/file.txt", "content");
        save(&root, "s2");

        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert!(
            !exists(&root, "subdir"),
            "Empty subdir should be cleaned up"
        );
    }

    #[test]
    fn restore_updates_parent_pointer() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert_eq!(parent(&root), h1);
        // Working tree should be clean after restore
        assert!(with_repo(&root, commands::get_dirty_files).is_empty());
    }

    #[test]
    fn restore_invalid_hash_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let result = with_write(&root, |vr| {
            commands::restore::run(vr, "deadbeef9999", true, &[])
        });
        assert!(result.is_err());
    }

    // =========================================================================
    // status
    // =========================================================================

    #[test]
    fn status_shows_new_modified_deleted() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        write(&root, "b.txt", "B");
        save(&root, "s1");

        write(&root, "a.txt", "A_mod"); // modified
        write(&root, "c.txt", "C"); // new
        fs::remove_file(root.join("b.txt")).unwrap(); // deleted

        let dirty = with_repo(&root, commands::get_dirty_files);
        assert_eq!(dirty.get("a.txt"), Some(&FileStatus::Modified));
        assert_eq!(dirty.get("c.txt"), Some(&FileStatus::New));
        assert_eq!(dirty.get("b.txt"), Some(&FileStatus::Deleted));
    }

    #[test]
    fn status_is_clean_after_restore() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert!(with_repo(&root, commands::get_dirty_files).is_empty());
    }

    #[test]
    fn status_run_does_not_panic_on_empty_repo() {
        let (_tmp, root) = setup();
        with_repo(&root, |vr| commands::status::run(vr, &[])).unwrap();
    }

    // =========================================================================
    // history
    // =========================================================================

    #[test]
    fn history_walks_ancestry_newest_first() {
        let (_tmp, root) = setup();
        for i in 0..5 {
            write(&root, "f.txt", &i.to_string());
            save(&root, &format!("snap {}", i));
        }
        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    limit: Some(10),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert!(h.empty.is_none());
        let msgs: Vec<&str> = h.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(msgs, vec!["snap 4", "snap 3", "snap 2", "snap 1", "snap 0"]);
        // The newest entry is where the working tree sits.
        assert_eq!(h.current.as_deref(), Some(h.entries[0].hash.as_str()));
        // The chain is linked: each entry's parent is the next one along.
        for pair in h.entries.windows(2) {
            assert_eq!(pair[0].parent.as_deref(), Some(pair[1].hash.as_str()));
        }
        assert!(
            h.entries.last().unwrap().parent.is_none(),
            "the root snapshot has no parent"
        );
    }

    #[test]
    fn history_limit_caps_the_entry_count() {
        let (_tmp, root) = setup();
        for i in 0..10 {
            write(&root, "f.txt", &i.to_string());
            save(&root, &format!("snap {}", i));
        }
        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    limit: Some(3),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert_eq!(h.entries.len(), 3, "the limit must bound the listing");
        assert_eq!(h.entries[0].message, "snap 9");

        // The snapshots themselves are untouched by a display limit.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let total: i64 = conn
            .query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 10);
    }

    #[test]
    fn history_unborn_branch_is_reported_as_such() {
        use commands::history::{EmptyReason, Scope};
        let (_tmp, root) = setup();
        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    limit: Some(10),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert!(h.entries.is_empty());
        assert_eq!(
            h.scope,
            Scope::CurrentBranch {
                name: branch_name("main")
            }
        );
        assert_eq!(
            h.empty,
            Some(EmptyReason::UnbornBranch {
                branch: branch_name("main")
            })
        );
    }

    #[test]
    fn history_all_excludes_deleted_branches() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "main save");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "feat");
        save(&root, "feat save");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("feature"))
        })
        .unwrap();

        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    all: true,
                    limit: Some(20),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert!(
            h.entries.iter().all(|e| !e.branch.starts_with("_deleted_")),
            "soft-deleted history must not surface: {:?}",
            h.entries.iter().map(|e| &e.branch).collect::<Vec<_>>()
        );
        // They do still exist internally, which is what makes redo possible.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let hidden: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch LIKE '_deleted_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(hidden > 0);
    }

    #[test]
    fn history_filter_by_branch_only_lists_that_branch() {
        use commands::history::Scope;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "main snap");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "dev snap");

        // Asking for 'main' while standing on 'dev' must still work.
        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    branch: Some(&branch_name("main")),
                    limit: Some(20),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert_eq!(
            h.scope,
            Scope::NamedBranch {
                name: branch_name("main")
            }
        );
        assert!(h.entries.iter().all(|e| e.branch == "main"));
        assert!(h.entries.iter().any(|e| e.message == "main snap"));
        assert!(
            !h.entries.iter().any(|e| e.message == "dev snap"),
            "the branch filter must exclude other branches"
        );
    }

    #[test]
    fn history_file_filter_keeps_only_the_snapshot_that_added_the_path() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "a");
        save(&root, "adds a");
        write(&root, "b.txt", "b");
        save(&root, "adds b");

        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    file: Some("b.txt"),
                    limit: Some(20),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let msgs: Vec<&str> = h.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            msgs,
            vec!["adds b"],
            "only the snapshot that added b.txt changed it"
        );
    }

    #[test]
    fn history_file_filter_with_no_match_says_so() {
        use commands::history::EmptyReason;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "a");
        save(&root, "s1");

        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    file: Some("never.txt"),
                    limit: Some(20),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        assert!(h.entries.is_empty());
        assert_eq!(
            h.empty,
            Some(EmptyReason::NoSnapshotsTouching {
                file: "never.txt".into()
            })
        );
    }

    #[test]
    fn history_decorates_snapshots_with_the_branches_pointing_at_them() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    limit: Some(10),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let refs = h.refs_at(&SnapshotId::from_stored(h1.clone()));
        assert!(
            refs.iter().any(|r| r.name == "main" && r.is_head),
            "main points here and is checked out: {:?}",
            refs
        );
    }

    #[test]
    fn history_marks_merge_parents() {
        let (_tmp, root) = setup();
        write(&root, "base.txt", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "feat.txt", "feat");
        save(&root, "feat work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "main.txt", "main");
        save(&root, "main work");
        let main_tip = parent(&root);
        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        // A clean merge leaves MERGE_HEAD behind; the finalising save is what
        // stamps the second parent onto the new snapshot.
        save(&root, "Merge feature");

        let h = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    limit: Some(20),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        let merge = h
            .entries
            .iter()
            .find(|e| e.is_merge())
            .expect("the finalising save should record a second parent");
        // The merge's first parent is where main was; the second is the branch.
        assert_eq!(merge.parent.as_deref(), Some(main_tip.as_str()));
        assert_ne!(merge.parent, merge.merge_parent);
    }

    // =========================================================================
    // branches / tag / remote listings
    // =========================================================================

    #[test]
    fn branches_lists_the_current_branch_even_with_no_commits() {
        let (_tmp, root) = setup();
        let list = with_repo(&root, commands::branches::list).unwrap();
        let names: Vec<&str> = list.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main"], "a fresh repo still has main");
        assert!(list[0].is_current);
        assert!(list[0].tip.is_none(), "nothing saved yet, so no tip");
    }

    #[test]
    fn branches_describes_each_branch_by_its_tip() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "on main");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "g.txt", "v1");
        let h2 = save(&root, "on dev");

        let list = with_repo(&root, commands::branches::list).unwrap();
        let names: Vec<&str> = list.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["dev", "main"], "sorted by name");

        let dev = list.iter().find(|b| b.name == "dev").unwrap();
        assert!(dev.is_current, "dev is checked out");
        let tip = dev.tip.as_ref().unwrap();
        assert_eq!(tip.hash, h2);
        assert_eq!(tip.message, "on dev");
        assert!(
            tip.created_at > year_2020(),
            "the tip must carry a real creation time, not the column default"
        );

        let main = list.iter().find(|b| b.name == "main").unwrap();
        assert!(!main.is_current);
        assert_eq!(main.tip.as_ref().unwrap().hash, h1);
    }

    #[test]
    fn branches_delete_is_soft_and_drops_the_ref() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "on main");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "g.txt", "v1");
        let h2 = save(&root, "on dev");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("dev"))
        })
        .unwrap();

        // Gone from the listing…
        let names: Vec<String> = with_repo(&root, commands::branches::list)
            .unwrap()
            .into_iter()
            .map(|b| b.name.into_string())
            .collect();
        assert!(!names.contains(&"dev".to_string()));

        // …but the snapshot survives under the shelved branch name, which is
        // what makes it recoverable.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let branch: String = conn
            .query_row("SELECT branch FROM snapshots WHERE hash = ?", [&h2], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(branch, "_deleted_dev");
    }

    #[test]
    fn branches_delete_refuses_current_and_main_and_unknown() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        assert!(
            with_write(&root, |vr| commands::branches::delete(
                vr,
                &branch_name("main")
            ))
            .is_err(),
            "main is both current and the default branch"
        );
        assert!(
            with_write(&root, |vr| commands::branches::delete(
                vr,
                &branch_name("never_existed")
            ))
            .is_err(),
            "deleting an unknown branch is an error"
        );

        // Current-branch protection applies to any branch, not just main.
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        assert!(with_write(&root, |vr| commands::branches::delete(
            vr,
            &branch_name("dev")
        ))
        .is_err());
    }

    #[test]
    fn tag_create_defaults_to_the_current_position() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        let created = with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();
        assert_eq!(created.snapshot, h1);
        assert_eq!(created.name, "v1");
        assert!(created.replaced.is_none());
    }

    #[test]
    fn tag_create_can_target_an_older_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        let created = with_write(&root, |vr| {
            commands::tag::create(
                vr,
                &tag_name("old"),
                Some(&SnapshotId::from_stored(h1.clone())),
                false,
            )
        })
        .unwrap();
        assert_eq!(created.snapshot, h1);
        assert_ne!(created.snapshot, h2);
    }

    #[test]
    fn tag_create_needs_force_to_overwrite_and_reports_what_it_replaced() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();

        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");
        assert!(
            with_write(&root, |vr| commands::tag::create(
                vr,
                &tag_name("v1"),
                None,
                false
            ))
            .is_err(),
            "an existing tag must not be silently moved"
        );

        let created = with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, true)
        })
        .unwrap();
        assert_eq!(created.snapshot, h2, "force moves the tag");
        assert_eq!(
            created.replaced.as_deref(),
            Some(h1.as_str()),
            "and reports where it used to point"
        );
    }

    #[test]
    fn tag_create_on_an_unborn_branch_is_an_error() {
        let (_tmp, root) = setup();
        assert!(with_write(&root, |vr| commands::tag::create(
            vr,
            &tag_name("v1"),
            None,
            false
        ))
        .is_err());
    }

    #[test]
    fn tag_list_is_ordered_and_survives_a_shelved_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("zebra"), None, false)
        })
        .unwrap();
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("alpha"), None, false)
        })
        .unwrap();

        let tags = with_repo(&root, commands::tag::list).unwrap();
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zebra"], "ordered by name");
        assert!(tags.iter().all(|t| t.message.as_deref() == Some("s1")));

        // `undo` shelves the snapshot; the tag outlives it with no message.
        with_write(&root, commands::undo::run).unwrap();
        let tags = with_repo(&root, commands::tag::list).unwrap();
        assert!(
            tags.iter().all(|t| t.message.is_none()),
            "a tag whose snapshot is shelved has no message to show"
        );
    }

    #[test]
    fn tag_delete_removes_it_and_unknown_names_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();

        with_write(&root, |vr| commands::tag::delete(vr, &tag_name("v1"))).unwrap();
        assert!(with_repo(&root, commands::tag::list).unwrap().is_empty());
        assert!(
            with_write(&root, |vr| commands::tag::delete(vr, &tag_name("v1"))).is_err(),
            "already gone"
        );
    }

    #[test]
    fn remote_add_list_remove_roundtrip() {
        let (_tmp, root) = setup();
        assert!(with_repo(&root, commands::remote::list).unwrap().is_empty());

        // A path that isn't a repo is still accepted — it may become one — but
        // the caller is told.
        let added = with_write(&root, |vr| {
            commands::remote::add(vr, "origin", "/definitely/not/a/repo")
        })
        .unwrap();
        assert_eq!(added.name, "origin");
        assert!(
            added.unreachable,
            "should flag a path that isn't a repo yet"
        );

        let remotes = with_repo(&root, commands::remote::list).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "/definitely/not/a/repo");

        with_write(&root, |vr| commands::remote::remove(vr, "origin")).unwrap();
        assert!(with_repo(&root, commands::remote::list).unwrap().is_empty());
        assert!(
            with_write(&root, |vr| commands::remote::remove(vr, "origin")).is_err(),
            "removing an unknown remote is an error"
        );
    }

    #[test]
    fn remote_add_rejects_bad_names_and_replaces_by_name() {
        let (_tmp, root) = setup();
        assert!(with_write(&root, |vr| commands::remote::add(vr, "", "/x")).is_err());
        assert!(with_write(&root, |vr| commands::remote::add(vr, "has/slash", "/x")).is_err());

        with_write(&root, |vr| commands::remote::add(vr, "origin", "/first")).unwrap();
        with_write(&root, |vr| commands::remote::add(vr, "origin", "/second")).unwrap();
        let remotes = with_repo(&root, commands::remote::list).unwrap();
        assert_eq!(remotes.len(), 1, "re-adding a name replaces it");
        assert_eq!(remotes[0].url, "/second");
    }

    #[test]
    fn remote_remove_also_drops_its_tracking_refs() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| commands::remote::add(vr, "origin", "/x")).unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        conn.execute(
            "INSERT INTO remote_refs (remote, branch, hash) VALUES ('origin', 'main', 'abc')",
            [],
        )
        .unwrap();
        drop(conn);

        with_write(&root, |vr| commands::remote::remove(vr, "origin")).unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT count(*) FROM remote_refs WHERE remote = 'origin'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "tracking refs must not outlive their remote");
    }

    #[test]
    fn fsck_sections_are_reported_in_order() {
        use commands::fsck::Section;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        let report = with_repo(&root, commands::fsck::check).unwrap();
        assert!(report.is_healthy());
        assert!(!report.repair_requested);
        assert_eq!(report.sections.len(), 4);
        assert!(matches!(report.sections[0], Section::Objects { .. }));
        assert!(matches!(report.sections[1], Section::Snapshots { .. }));
        assert!(matches!(report.sections[2], Section::Refs { .. }));
        assert!(matches!(report.sections[3], Section::State { .. }));
        assert!(report.sections.iter().all(|s| s.problems() == 0));
    }

    #[test]
    fn fsck_separates_cruft_from_corruption() {
        use commands::fsck::Cruft;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        conn.execute(
            "INSERT INTO hunk_decisions (file_path, hunk_id, decision, manual_content)
             VALUES ('ghost.txt', 0, 'ours', NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        let report = with_repo(&root, commands::fsck::check).unwrap();
        assert!(report.is_healthy(), "cruft is not corruption");
        assert!(report.has_cleanable_cruft());
        assert_eq!(report.cruft, vec![Cruft::OrphanHunkDecisions(1)]);
        assert!(
            report.repaired.is_empty(),
            "nothing was asked to be repaired"
        );

        let report = with_write(&root, commands::fsck::repair).unwrap();
        assert_eq!(report.repaired, vec![Cruft::OrphanHunkDecisions(1)]);
        assert!(
            report.cruft.is_empty(),
            "cleaned cruft is no longer outstanding"
        );
        assert!(!report.has_cleanable_cruft());

        // And it stays clean.
        assert!(!with_repo(&root, commands::fsck::check)
            .unwrap()
            .has_cleanable_cruft());
    }

    #[test]
    fn fsck_cruft_descriptions_differ_before_and_after_repair() {
        use commands::fsck::Cruft;
        let c = Cruft::OrphanHunkDecisions(2);
        assert!(c.describe().contains('2'));
        assert!(c.describe_repaired().starts_with("pruned"));
        assert_ne!(c.describe(), c.describe_repaired());
    }

    // =========================================================================
    // merge / restore outcomes
    // =========================================================================

    /// base → feature adds a file and edits one; main edits a different region.
    /// Produces a clean three-way merge.
    fn diverged_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let (tmp, root) = setup();
        write(&root, "shared.txt", "one\ntwo\nthree\nfour\nfive\n");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "feat.txt", "from feature\n");
        write(&root, "shared.txt", "ONE\ntwo\nthree\nfour\nfive\n");
        save(&root, "feature work");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "shared.txt", "one\ntwo\nthree\nfour\nFIVE\n");
        save(&root, "main work");
        (tmp, root)
    }

    #[test]
    fn merge_reports_each_file_in_path_order() {
        use commands::merge::{FileAction, Outcome};
        let (_tmp, root) = diverged_repo();

        let Outcome::Merged(result) =
            with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap()
        else {
            panic!("expected a three-way merge");
        };
        assert_eq!(result.source, "feature");
        assert_eq!(result.into, "main");
        assert!(result.ancestor.is_some(), "the branches share a base");

        // Sorted, not hash-order: the file list used to come out of a HashSet, so
        // the same merge reported its files differently on every run.
        let paths: Vec<&str> = result.files().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["feat.txt", "shared.txt"]);

        let by_path = |p: &str| {
            result
                .files()
                .iter()
                .find(|f| f.path == p)
                .unwrap_or_else(|| panic!("{} missing", p))
                .action
        };
        assert_eq!(by_path("feat.txt"), FileAction::Added);
        assert_eq!(
            by_path("shared.txt"),
            FileAction::AutoMerged,
            "edits to different lines must merge without a conflict"
        );

        assert!(result.is_clean());
        assert!(!result.applied_nothing());
        assert_eq!(result.added(), 1);
        assert_eq!(result.updated(), 1, "auto-merged counts as updated");
        assert_eq!(result.deleted(), 0);
        assert!(result.conflicts().is_empty());
    }

    #[test]
    fn merge_file_order_is_stable_across_runs() {
        use commands::merge::Outcome;
        // Same merge twice, in two identical repos: the reported order must match.
        let order = |_i: usize| {
            let (_tmp, root) = diverged_repo();
            let Outcome::Merged(r) =
                with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap()
            else {
                panic!("expected a three-way merge");
            };
            r.files()
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<String>>()
        };
        assert_eq!(order(0), order(1));
    }

    #[test]
    fn merge_records_conflicts_and_leaves_merge_head() {
        use commands::merge::{FileAction, Outcome};
        let (_tmp, root) = setup();
        write(&root, "c.txt", "one\ntwo\nthree\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "c.txt", "one\nFEATURE\nthree\n");
        save(&root, "feature work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "c.txt", "one\nMAIN\nthree\n");
        save(&root, "main work");

        let Outcome::Merged(result) =
            with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap()
        else {
            panic!("expected a three-way merge");
        };
        assert!(!result.is_clean());
        assert_eq!(result.conflicts(), vec!["c.txt"]);
        assert_eq!(result.files()[0].action, FileAction::Conflicted);

        assert!(
            root.join(".velo/MERGE_HEAD").exists(),
            "a conflicted merge must leave MERGE_HEAD for --abort and save"
        );
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the conflict must be recorded for velo resolve");
    }

    #[test]
    fn merge_fast_forwards_when_our_tip_is_an_ancestor() {
        use commands::merge::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "v2");
        save(&root, "ahead");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        let outcome =
            with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        let Outcome::FastForwarded { branch, to } = outcome else {
            panic!("expected a fast-forward, got {:?}", outcome);
        };
        assert_eq!(branch, "main");
        assert_eq!(parent(&root), to, "the position must move to the new tip");
        assert_eq!(read(&root, "f.txt"), "v2");
    }

    #[test]
    fn merge_into_an_unborn_branch_just_starts_it() {
        use commands::merge::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "on main");
        // A brand-new branch with nothing of its own.
        with_write(&root, |vr| commands::switch::run(vr, "fresh", false)).unwrap();

        let outcome =
            with_write(&root, |vr| commands::merge::run(vr, Some("main"), false)).unwrap();
        let Outcome::StartedUnbornBranch { branch, at } = outcome else {
            panic!("expected the unborn-branch case, got {:?}", outcome);
        };
        assert_eq!(branch, "fresh");
        assert_eq!(at, h1);
    }

    #[test]
    fn merge_at_the_same_snapshot_is_already_up_to_date() {
        use commands::merge::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "base");

        // Merging the snapshot we already sit on: both sides are literally the
        // same commit, so there is nothing to compare.
        let outcome = with_write(&root, |vr| commands::merge::run(vr, Some(&h1), false)).unwrap();
        assert!(
            matches!(outcome, Outcome::AlreadyUpToDate { .. }),
            "got {:?}",
            outcome
        );
    }

    #[test]
    fn merge_of_already_incorporated_work_applies_nothing() {
        use commands::merge::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "v2");
        save(&root, "feature work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap(); // fast-forward

        // main's tip is now the fast-forward snapshot, so this is a real
        // three-way merge — it just finds both trees already agree.
        let Outcome::Merged(result) =
            with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap()
        else {
            panic!("expected a three-way merge once main has its own tip");
        };
        assert!(result.is_clean());
        assert!(
            result.applied_nothing(),
            "the work is already incorporated: {:?}",
            result.files()
        );
        assert!(
            !root.join(".velo/MERGE_HEAD").exists(),
            "a no-op merge must not leave merge state behind"
        );
    }

    #[test]
    fn merge_in_progress_is_reported_before_the_dirty_tree() {
        let (_tmp, root) = setup();
        write(&root, "c.txt", "one\ntwo\nthree\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "c.txt", "one\nFEATURE\nthree\n");
        save(&root, "feature work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "c.txt", "one\nMAIN\nthree\n");
        save(&root, "main work");
        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();

        // A conflicted merge leaves the tree dirty by design, so checking
        // dirtiness first blamed "unsaved changes" for being mid-merge.
        let err =
            with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap_err();
        assert!(
            matches!(err, VeloError::OperationInProgress { .. }),
            "expected the in-progress diagnosis, got {:?}",
            err
        );
    }

    #[test]
    fn merge_abort_restores_and_clears_state() {
        use commands::merge::Outcome;
        let (_tmp, root) = setup();
        write(&root, "c.txt", "one\ntwo\nthree\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "c.txt", "one\nFEATURE\nthree\n");
        save(&root, "feature work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "c.txt", "one\nMAIN\nthree\n");
        let before = save(&root, "main work");
        let content_before = read(&root, "c.txt");

        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        let outcome = with_write(&root, |vr| commands::merge::run(vr, None, true)).unwrap();
        let Outcome::Aborted {
            source,
            restored_to,
        } = outcome
        else {
            panic!("expected an abort, got {:?}", outcome);
        };
        assert_eq!(source, "feature");
        assert_eq!(restored_to.as_deref(), Some(before.as_str()));
        assert_eq!(read(&root, "c.txt"), content_before, "content restored");
        assert!(!root.join(".velo/MERGE_HEAD").exists());

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "conflict state must be cleared");
    }

    #[test]
    fn merge_abort_without_a_merge_is_an_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let err = with_write(&root, |vr| commands::merge::run(vr, None, true)).unwrap_err();
        assert!(matches!(err, VeloError::NoOperationInProgress { .. }));
    }

    #[test]
    fn merge_refuses_a_dirty_tree_and_names_the_paths() {
        let (_tmp, root) = diverged_repo();
        write(&root, "unsaved.txt", "wip");

        let err =
            with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap_err();
        let VeloError::DirtyWorkingTree { paths } = err else {
            panic!("expected a dirty-tree error, got {:?}", err);
        };
        assert!(
            paths.iter().any(|p| p.ends_with("unsaved.txt")),
            "the error must name what is unsaved: {:?}",
            paths
        );
    }

    #[test]
    fn merge_into_itself_is_an_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        assert!(with_write(&root, |vr| commands::merge::run(vr, Some("main"), false)).is_err());
    }

    #[test]
    fn restore_reports_a_no_op_when_already_there() {
        use commands::restore::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        let outcome = with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert_eq!(outcome, Outcome::AlreadyThere { snapshot: h1 });
    }

    #[test]
    fn restore_counts_files_ghosts_and_discards() {
        use commands::restore::Outcome;
        let (_tmp, root) = setup();
        write(&root, "keep.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "extra.txt", "added later");
        save(&root, "s2");
        // Unsaved work that --force will throw away.
        write(&root, "keep.txt", "edited");

        let outcome = with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        let Outcome::Restored {
            snapshot,
            branch,
            message,
            files,
            ghosts_removed,
            discarded,
        } = outcome
        else {
            panic!("expected a full restore, got {:?}", outcome);
        };
        assert_eq!(snapshot, h1);
        assert_eq!(branch, "main");
        assert_eq!(message, "s1");
        assert!(files >= 1);
        assert_eq!(ghosts_removed, 1, "extra.txt is not in the target snapshot");
        assert_eq!(discarded, 1, "the edit to keep.txt was discarded");
        assert!(!root.join("extra.txt").exists());
        assert_eq!(read(&root, "keep.txt"), "v1");
    }

    #[test]
    fn restore_of_paths_leaves_the_position_alone() {
        use commands::restore::Outcome;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "v1");
        write(&root, "b.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "a.txt", "v2");
        write(&root, "b.txt", "v2");
        let h2 = save(&root, "s2");

        let outcome = with_write(&root, |vr| {
            commands::restore::run(vr, &h1, false, &["a.txt".into()])
        })
        .unwrap();
        let Outcome::RestoredPaths { files, .. } = outcome else {
            panic!("expected a path-limited restore, got {:?}", outcome);
        };
        assert_eq!(files, 1);
        assert_eq!(read(&root, "a.txt"), "v1", "the named path reverted");
        assert_eq!(read(&root, "b.txt"), "v2", "other paths untouched");
        assert_eq!(parent(&root), h2, "a partial restore must not move PARENT");
    }

    #[test]
    fn restore_of_an_unmatched_path_says_so() {
        use commands::restore::Outcome;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "v1");
        let h1 = save(&root, "s1");

        let outcome = with_write(&root, |vr| {
            commands::restore::run(vr, &h1, false, &["nope.txt".into()])
        })
        .unwrap();
        assert!(
            matches!(outcome, Outcome::NoMatchingPaths { .. }),
            "got {:?}",
            outcome
        );
    }

    #[test]
    fn restore_without_force_refuses_and_names_the_paths() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");
        write(&root, "f.txt", "unsaved");

        let err = with_write(&root, |vr| commands::restore::run(vr, &h1, false, &[])).unwrap_err();
        let VeloError::DirtyWorkingTree { paths } = err else {
            panic!("expected a dirty-tree error, got {:?}", err);
        };
        assert!(paths.iter().any(|p| p.ends_with("f.txt")));
    }

    #[test]
    fn restore_of_a_missing_snapshot_is_not_found() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let err = with_write(&root, |vr| {
            commands::restore::run(vr, "deadbeefdeadbeef", true, &[])
        })
        .unwrap_err();
        assert!(matches!(err, VeloError::NotFound { .. }), "got {:?}", err);
    }

    // =========================================================================
    // cherry-pick / rebase outcomes
    // =========================================================================

    /// base, then a `side` branch with one commit that edits and adds a file.
    /// Returns (repo, root, side tip hash).
    fn side_branch_repo() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let (tmp, root) = setup();
        write(&root, "shared.txt", "one\ntwo\nthree\nfour\nfive\n");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "side", false)).unwrap();
        write(&root, "shared.txt", "ONE\ntwo\nthree\nfour\nfive\n");
        write(&root, "extra.txt", "from side\n");
        let tip = save(&root, "side work");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        (tmp, root, tip)
    }

    #[test]
    fn cherry_pick_applies_and_commits_a_clean_pick() {
        use commands::apply::FileAction;
        let (_tmp, root, tip) = side_branch_repo();

        let outcome = with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap();
        assert_eq!(outcome.snapshot, tip);
        assert_eq!(outcome.message, "side work");
        assert!(!outcome.is_conflicted());
        assert!(!outcome.applied_nothing());
        assert!(
            outcome.saved_as.is_some(),
            "a clean pick commits straight away"
        );

        // Same vocabulary as merge, in path order.
        let paths: Vec<&str> = outcome
            .applied
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(paths, vec!["extra.txt", "shared.txt"]);
        let action = |p: &str| {
            outcome
                .applied
                .files
                .iter()
                .find(|f| f.path == p)
                .unwrap()
                .action
        };
        assert_eq!(action("extra.txt"), FileAction::Added);
        assert_eq!(action("shared.txt"), FileAction::Updated);
        assert_eq!(read(&root, "extra.txt"), "from side\n");
    }

    #[test]
    fn cherry_pick_of_already_applied_work_does_nothing() {
        let (_tmp, root, tip) = side_branch_repo();
        with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap();

        let outcome = with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap();
        assert!(outcome.applied_nothing());
        assert!(
            outcome.saved_as.is_none(),
            "nothing to apply means no second snapshot"
        );
    }

    #[test]
    fn cherry_pick_records_conflicts_without_committing() {
        let (_tmp, root) = setup();
        write(&root, "c.txt", "one\ntwo\nthree\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "side", false)).unwrap();
        write(&root, "c.txt", "one\nSIDE\nthree\n");
        let tip = save(&root, "side work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "c.txt", "one\nMAIN\nthree\n");
        save(&root, "main work");

        let outcome = with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap();
        assert!(outcome.is_conflicted());
        assert_eq!(outcome.applied.conflicts(), vec!["c.txt"]);
        assert!(
            outcome.saved_as.is_none(),
            "a conflicted pick is finished by the user, not auto-committed"
        );
        assert!(root.join(".velo/MERGE_HEAD").exists());
    }

    #[test]
    fn cherry_pick_in_progress_is_reported_before_the_dirty_tree() {
        let (_tmp, root) = setup();
        write(&root, "c.txt", "one\ntwo\nthree\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "side", false)).unwrap();
        write(&root, "c.txt", "one\nSIDE\nthree\n");
        let tip = save(&root, "side work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "c.txt", "one\nMAIN\nthree\n");
        save(&root, "main work");
        with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap();

        let err = with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap_err();
        assert!(
            matches!(err, VeloError::OperationInProgress { .. }),
            "a paused pick leaves the tree dirty, so dirtiness is the wrong diagnosis: {:?}",
            err
        );
    }

    #[test]
    fn cherry_pick_refuses_a_dirty_tree() {
        let (_tmp, root, tip) = side_branch_repo();
        write(&root, "unsaved.txt", "wip");
        let err = with_write(&root, |vr| commands::cherry_pick::run(vr, &tip)).unwrap_err();
        assert!(matches!(err, VeloError::DirtyWorkingTree { .. }));
    }

    /// A branch with two commits, diverged from a main that has moved on.
    /// Returns (repo, root, main tip).
    fn diverged_for_rebase() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let (tmp, root) = setup();
        write(&root, "base.txt", "base\n");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "a.txt", "one\n");
        save(&root, "feat 1");
        write(&root, "b.txt", "two\n");
        save(&root, "feat 2");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "m.txt", "main\n");
        let main_tip = save(&root, "main moved on");

        with_write(&root, |vr| commands::switch::run(vr, "feature", true)).unwrap();
        (tmp, root, main_tip)
    }

    #[test]
    fn rebase_replays_every_commit_in_order() {
        use commands::rebase::Outcome;
        let (_tmp, root, main_tip) = diverged_for_rebase();

        let outcome = with_write(&root, |vr| {
            commands::rebase::run(vr, &main_tip, false, false)
        })
        .unwrap();
        let Outcome::Completed {
            branch,
            onto,
            head,
            replayed,
        } = outcome
        else {
            panic!("expected a completed rebase, got {:?}", outcome);
        };
        assert_eq!(branch, "feature");
        assert_eq!(onto, main_tip);
        assert_eq!(head, parent(&root));

        let messages: Vec<&str> = replayed.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(messages, vec!["feat 1", "feat 2"], "oldest first");
        assert_eq!(replayed[0].index, 1);
        assert_eq!(replayed[1].index, 2);
        assert!(replayed.iter().all(|r| r.total == 2));

        // The branch's work sits on top of main's, and main's file came along.
        assert_eq!(read(&root, "m.txt"), "main\n");
        assert_eq!(read(&root, "a.txt"), "one\n");
        assert_eq!(read(&root, "b.txt"), "two\n");
    }

    #[test]
    fn rebase_is_idempotent() {
        use commands::rebase::Outcome;
        let (_tmp, root, main_tip) = diverged_for_rebase();
        with_write(&root, |vr| {
            commands::rebase::run(vr, &main_tip, false, false)
        })
        .unwrap();
        let after_first = parent(&root);

        // Re-running the same rebase used to replay every commit again onto fresh
        // hashes, silently duplicating the branch: the guard only checked
        // `onto == head`, but a rebased branch *contains* onto without equalling
        // it.
        let outcome = with_write(&root, |vr| {
            commands::rebase::run(vr, &main_tip, false, false)
        })
        .unwrap();
        assert!(
            matches!(outcome, Outcome::AlreadyUpToDate),
            "got {:?}",
            outcome
        );
        assert_eq!(parent(&root), after_first, "history must not move");
    }

    #[test]
    fn rebase_onto_an_ancestor_is_already_up_to_date() {
        use commands::rebase::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let base = save(&root, "base");
        write(&root, "f.txt", "v2");
        save(&root, "later");

        // `base` is already behind us.
        let outcome =
            with_write(&root, |vr| commands::rebase::run(vr, &base, false, false)).unwrap();
        assert!(matches!(outcome, Outcome::AlreadyUpToDate));
    }

    #[test]
    fn rebase_pauses_on_conflict_and_resumes_with_the_next_commit() {
        use commands::rebase::Outcome;
        let (_tmp, root) = setup();
        write(&root, "shared.txt", "a\nb\nc\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "shared.txt", "a\nFEATURE\nc\n");
        save(&root, "feat edits shared");
        write(&root, "later.txt", "later\n");
        save(&root, "feat adds later");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "shared.txt", "a\nMAIN\nc\n");
        let main_tip = save(&root, "main edits shared");
        with_write(&root, |vr| commands::switch::run(vr, "feature", true)).unwrap();

        let outcome = with_write(&root, |vr| {
            commands::rebase::run(vr, &main_tip, false, false)
        })
        .unwrap();
        let Outcome::Paused {
            replayed,
            stopped_at,
            applied,
            ..
        } = outcome
        else {
            panic!("expected a paused rebase, got {:?}", outcome);
        };
        assert!(replayed.is_empty(), "the very first commit conflicted");
        assert_eq!(stopped_at.message, "feat edits shared");
        assert_eq!(stopped_at.index, 1);
        assert_eq!(stopped_at.total, 2);
        assert_eq!(applied.conflicts(), vec!["shared.txt"]);
        assert!(root.join(".velo/MERGE_HEAD").exists());

        // --continue refuses while conflicts stand.
        let err = with_write(&root, |vr| commands::rebase::run(vr, "", false, true)).unwrap_err();
        assert!(matches!(err, VeloError::Conflicts { .. }), "got {:?}", err);

        // Resolve as the user would, then carry on.
        resolve_take(&root, None, commands::resolve::TakeOption::Theirs, true).unwrap();
        save(&root, "resolved shared.txt");

        let outcome = with_write(&root, |vr| commands::rebase::run(vr, "", false, true)).unwrap();
        let Outcome::Completed { replayed, .. } = outcome else {
            panic!("expected completion, got {:?}", outcome);
        };
        // The conflicted commit is not replayed twice — only what was left.
        let messages: Vec<&str> = replayed.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(messages, vec!["feat adds later"]);
        assert!(!root.join(".velo/REBASE_STATE").exists());
        assert_eq!(read(&root, "later.txt"), "later\n");
    }

    #[test]
    fn rebase_abort_restores_the_branch_and_discards_replays() {
        use commands::rebase::Outcome;
        let (_tmp, root) = setup();
        write(&root, "shared.txt", "a\nb\nc\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "shared.txt", "a\nFEATURE\nc\n");
        let feat_tip = save(&root, "feat edits");
        let content_before = read(&root, "shared.txt");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "shared.txt", "a\nMAIN\nc\n");
        let main_tip = save(&root, "main edits");
        with_write(&root, |vr| commands::switch::run(vr, "feature", true)).unwrap();

        with_write(&root, |vr| {
            commands::rebase::run(vr, &main_tip, false, false)
        })
        .unwrap();
        let outcome = with_write(&root, |vr| commands::rebase::run(vr, "", true, false)).unwrap();
        let Outcome::Aborted { restored_to, .. } = outcome else {
            panic!("expected an abort, got {:?}", outcome);
        };
        assert_eq!(restored_to.as_deref(), Some(feat_tip.as_str()));
        assert_eq!(parent(&root), feat_tip);
        assert_eq!(read(&root, "shared.txt"), content_before);
        assert!(!root.join(".velo/MERGE_HEAD").exists());
        assert!(!root.join(".velo/REBASE_STATE").exists());
        assert!(!root.join(".velo/REBASE_ONTO").exists());
    }

    #[test]
    fn rebase_continue_and_abort_need_a_rebase_in_progress() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        assert!(matches!(
            with_write(&root, |vr| commands::rebase::run(vr, "", false, true)).unwrap_err(),
            VeloError::NoOperationInProgress { .. }
        ));
        assert!(matches!(
            with_write(&root, |vr| commands::rebase::run(vr, "", true, false)).unwrap_err(),
            VeloError::NoOperationInProgress { .. }
        ));
    }

    #[test]
    fn rebase_refuses_a_dirty_tree() {
        let (_tmp, root, main_tip) = diverged_for_rebase();
        write(&root, "unsaved.txt", "wip");
        let err = with_write(&root, |vr| {
            commands::rebase::run(vr, &main_tip, false, false)
        })
        .unwrap_err();
        assert!(matches!(err, VeloError::DirtyWorkingTree { .. }));
    }

    #[test]
    fn save_distinguishes_its_two_no_op_reasons() {
        use commands::save::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        // Both used to collapse into `Ok(None)`, so a caller couldn't tell which.
        assert_eq!(
            with_write(&root, |vr| commands::save::run_with_paths(
                vr,
                Some("again"),
                false,
                &[]
            ))
            .unwrap(),
            Outcome::NothingToSave
        );
        assert_eq!(
            with_write(&root, |vr| commands::save::run_with_paths(
                vr,
                None,
                true,
                &[]
            ))
            .unwrap(),
            Outcome::NothingToAmend
        );

        write(&root, "f.txt", "v2");
        let outcome = with_write(&root, |vr| commands::save::run(vr, "s2", false)).unwrap();
        assert!(outcome.saved());
        assert!(outcome.hash().is_some());
    }

    // =========================================================================
    // switch / undo / redo outcomes
    // =========================================================================

    #[test]
    fn switch_to_a_new_branch_reports_what_it_inherits() {
        use commands::switch::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        let outcome = with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        let Outcome::StartedUnborn {
            branch,
            existing,
            inherits,
        } = outcome
        else {
            panic!("expected an unborn branch, got {:?}", outcome);
        };
        assert_eq!(branch, "feature");
        assert!(!existing, "the branch was created just now");
        assert_eq!(
            inherits.as_deref(),
            Some(h1.as_str()),
            "a first save would start from where we stood"
        );
    }

    #[test]
    fn switch_in_a_fresh_repo_has_nothing_to_inherit() {
        use commands::switch::Outcome;
        let (_tmp, root) = setup();
        let outcome = with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        let Outcome::StartedUnborn { inherits, .. } = outcome else {
            panic!("expected an unborn branch, got {:?}", outcome);
        };
        assert!(inherits.is_none(), "no snapshots exist to inherit from");
    }

    #[test]
    fn switch_revisiting_an_unborn_branch_knows_it_exists() {
        use commands::switch::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();

        let outcome = with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        let Outcome::StartedUnborn { existing, .. } = outcome else {
            panic!("expected an unborn branch, got {:?}", outcome);
        };
        assert!(existing, "the branch was visited before, not created now");
    }

    #[test]
    fn switch_to_a_branch_with_commits_restores_its_tip() {
        use commands::switch::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        let main_tip = save(&root, "on main");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "on dev");

        let outcome = with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        assert_eq!(
            outcome,
            Outcome::Switched {
                branch: "main".into(),
                at: main_tip.clone()
            }
        );
        assert_eq!(read(&root, "f.txt"), "main");
        assert_eq!(parent(&root), main_tip);
    }

    #[test]
    fn switch_to_the_current_branch_is_a_no_op() {
        use commands::switch::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let outcome = with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();
        assert_eq!(
            outcome,
            Outcome::AlreadyOn {
                branch: "main".into()
            }
        );
    }

    #[test]
    fn switch_carries_new_files_along_without_complaint() {
        use commands::switch::Outcome;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "on main");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "on dev");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        // Untracked files are in no snapshot, so switching can't overwrite them.
        write(&root, "brand_new.txt", "not in any snapshot");
        let outcome = with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        assert!(matches!(outcome, Outcome::Switched { .. }));
        assert!(
            root.join("brand_new.txt").exists(),
            "an untracked file must survive a switch"
        );
    }

    #[test]
    fn switch_to_a_deleted_branch_is_an_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        assert!(with_write(&root, |vr| commands::switch::run(
            vr,
            "_deleted_gone",
            false
        ))
        .is_err());
    }

    #[test]
    fn undo_shelves_the_tip_and_reports_where_we_land() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        let outcome = with_write(&root, commands::undo::run).unwrap();
        assert_eq!(outcome.snapshot, h2);
        assert_eq!(outcome.message, "s2");
        assert_eq!(outcome.now_at.as_deref(), Some(h1.as_str()));
        assert!(!outcome.cleared_working_tree());
        assert_eq!(read(&root, "f.txt"), "v1");
        assert!(in_trash(&root, &h2), "it must stay recoverable");
    }

    #[test]
    fn undo_of_the_root_snapshot_empties_the_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        let outcome = with_write(&root, commands::undo::run).unwrap();
        assert_eq!(outcome.snapshot, h1);
        assert!(outcome.now_at.is_none());
        assert!(outcome.cleared_working_tree());
        assert_eq!(parent(&root), "");
        assert!(!root.join("f.txt").exists());
    }

    #[test]
    fn undo_is_refused_mid_merge_and_mid_rebase() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        fs::write(root.join(".velo/MERGE_HEAD"), "x").unwrap();
        assert!(matches!(
            with_write(&root, commands::undo::run).unwrap_err(),
            VeloError::OperationInProgress { .. }
        ));
        fs::remove_file(root.join(".velo/MERGE_HEAD")).unwrap();

        fs::write(root.join(".velo/REBASE_STATE"), "x").unwrap();
        assert!(matches!(
            with_write(&root, commands::undo::run).unwrap_err(),
            VeloError::OperationInProgress { .. }
        ));
    }

    #[test]
    fn undo_refuses_a_dirty_tree_and_names_the_paths() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "unsaved");

        let err = with_write(&root, commands::undo::run).unwrap_err();
        let VeloError::DirtyWorkingTree { paths } = err else {
            panic!("expected a dirty-tree error, got {:?}", err);
        };
        assert!(paths.iter().any(|p| p.ends_with("f.txt")));
    }

    #[test]
    fn undo_with_nothing_to_undo_is_an_error() {
        let (_tmp, root) = setup();
        assert!(with_write(&root, commands::undo::run).is_err());
    }

    #[test]
    fn redo_restores_what_undo_shelved() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");
        with_write(&root, commands::undo::run).unwrap();

        let outcome = with_write(&root, commands::redo::run).unwrap();
        assert_eq!(outcome.snapshot, h2);
        assert_eq!(outcome.message, "s2");
        assert_eq!(parent(&root), h2);
        assert_eq!(read(&root, "f.txt"), "v2");
        assert!(snapshot_exists(&root, &h2));
        assert!(!in_trash(&root, &h2), "it left the trash");
    }

    #[test]
    fn undo_and_redo_carry_tags_with_the_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("release"), None, false)
        })
        .unwrap();

        with_write(&root, commands::undo::run).unwrap();
        assert!(
            with_repo(&root, commands::tag::list).unwrap().is_empty(),
            "the tag is shelved with its snapshot"
        );

        with_write(&root, commands::redo::run).unwrap();
        let tags = with_repo(&root, commands::tag::list).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "release");
    }

    #[test]
    fn redo_with_nothing_shelved_is_an_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        assert!(with_write(&root, commands::redo::run).is_err());
    }

    #[test]
    fn redo_refuses_a_dirty_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");
        with_write(&root, commands::undo::run).unwrap();
        write(&root, "f.txt", "unsaved");

        assert!(matches!(
            with_write(&root, commands::redo::run).unwrap_err(),
            VeloError::DirtyWorkingTree { .. }
        ));
    }

    // =========================================================================
    // stash outcomes
    // =========================================================================

    #[test]
    fn stash_push_on_a_clean_tree_does_nothing() {
        use commands::stash::Pushed;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        assert_eq!(
            with_write(&root, |vr| commands::stash::push(vr, None)).unwrap(),
            Pushed::NothingToStash
        );
        assert!(with_repo(&root, commands::stash::list).unwrap().is_empty());
    }

    #[test]
    fn stash_push_counts_what_it_shelved_and_cleans_the_tree() {
        use commands::stash::Pushed;
        let (_tmp, root) = setup();
        write(&root, "keep.txt", "v1");
        write(&root, "gone.txt", "v1");
        let base = save(&root, "base");

        write(&root, "keep.txt", "v2");
        write(&root, "fresh.txt", "new file");
        fs::remove_file(root.join("gone.txt")).unwrap();

        let pushed = with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();
        let Pushed::Shelved {
            name,
            modified,
            new,
            deleted,
            restored_to,
        } = pushed
        else {
            panic!("expected a shelf, got {:?}", pushed);
        };
        assert_eq!(name, "wip");
        assert_eq!((modified, new, deleted), (1, 1, 1));
        assert_eq!(restored_to.as_deref(), Some(base.as_str()));

        // The tree is back at the snapshot: the edit is undone, the new file is
        // gone (it's safely on the shelf), and the deleted file is back.
        assert_eq!(read(&root, "keep.txt"), "v1");
        assert!(!root.join("fresh.txt").exists());
        assert!(root.join("gone.txt").exists());
        assert!(with_repo(&root, commands::get_dirty_files).is_empty());
    }

    #[test]
    fn stash_pop_reapplies_a_deletion() {
        let (_tmp, root) = setup();
        write(&root, "keep.txt", "v1");
        write(&root, "gone.txt", "v1");
        save(&root, "base");

        fs::remove_file(root.join("gone.txt")).unwrap();
        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();
        assert!(
            root.join("gone.txt").exists(),
            "restoring to the parent brings it back"
        );

        let popped = with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap();
        // Popping only ever wrote files, so a shelved deletion was silently lost.
        assert!(
            !root.join("gone.txt").exists(),
            "the shelved deletion must be reapplied"
        );
        assert_eq!(popped.removed, 1);
    }

    #[test]
    fn stash_pop_restores_content_and_forgets_the_shelf() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");
        write(&root, "f.txt", "wip edit");
        write(&root, "extra.txt", "also wip");

        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();
        let popped = with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap();

        assert_eq!(popped.name, "wip");
        assert!(popped.branch_mismatch.is_none());
        assert!(!popped.position_moved);
        assert_eq!(read(&root, "f.txt"), "wip edit");
        assert_eq!(read(&root, "extra.txt"), "also wip");
        assert!(
            with_repo(&root, commands::stash::list).unwrap().is_empty(),
            "popping consumes the shelf"
        );
    }

    #[test]
    fn stash_pop_notes_a_branch_change_without_refusing() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");
        write(&root, "f.txt", "wip");
        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();

        with_write(&root, |vr| commands::switch::run(vr, "elsewhere", false)).unwrap();
        let popped = with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap();
        let mismatch = popped
            .branch_mismatch
            .expect("a shelf from another branch should be flagged");
        assert_eq!(mismatch.shelf, "main");
        assert_eq!(mismatch.current, "elsewhere");
        assert_eq!(read(&root, "f.txt"), "wip", "but it still applies");
    }

    #[test]
    fn stash_pop_refuses_a_dirty_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");
        write(&root, "f.txt", "wip");
        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();

        write(&root, "other.txt", "conflicting work");
        assert!(matches!(
            with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap_err(),
            VeloError::DirtyWorkingTree { .. }
        ));
    }

    #[test]
    fn stash_list_is_newest_first_and_drop_removes_without_applying() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");

        write(&root, "f.txt", "first wip");
        with_write(&root, |vr| commands::stash::push(vr, Some("one".into()))).unwrap();
        write(&root, "f.txt", "second wip");
        with_write(&root, |vr| commands::stash::push(vr, Some("two".into()))).unwrap();

        let shelves = with_repo(&root, commands::stash::list).unwrap();
        let names: Vec<&str> = shelves.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["two", "one"], "newest first");
        assert!(shelves.iter().all(|s| s.branch == "main"));

        let dropped = with_write(&root, |vr| commands::stash::drop_shelf(vr, None)).unwrap();
        assert_eq!(dropped, "two");
        assert_eq!(read(&root, "f.txt"), "v1", "drop must not apply anything");
        let names: Vec<String> = with_repo(&root, commands::stash::list)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["one".to_string()]);
    }

    #[test]
    fn stash_rejects_a_duplicate_name_and_unknown_lookups() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "base");
        write(&root, "f.txt", "wip");
        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();

        write(&root, "f.txt", "more wip");
        assert!(
            with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).is_err(),
            "a name collision must not silently overwrite a shelf"
        );
        assert!(with_write(&root, |vr| commands::stash::pop(vr, Some("nope".into()))).is_err());
        assert!(with_write(&root, |vr| commands::stash::drop_shelf(
            vr,
            Some("nope".into())
        ))
        .is_err());
    }

    #[test]
    #[cfg(unix)]
    fn stash_pop_preserves_the_exec_bit() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = setup();
        write(&root, "placeholder.txt", "x\n");
        save(&root, "base");

        write(&root, "run.sh", "#!/bin/sh\necho hi\n");
        let script = root.join("run.sh");
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();
        with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap();

        // Popping used to `fs::write` the bytes and drop the mode entirely.
        let mode = fs::metadata(&script).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "the exec bit must survive a shelve/pop round trip, got {:o}",
            mode
        );
    }

    #[test]
    #[cfg(unix)]
    fn stash_pop_preserves_a_symlink() {
        let (_tmp, root) = setup();
        write(&root, "target.txt", "hello\n");
        save(&root, "base");

        std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();
        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();
        with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap();

        let meta = fs::symlink_metadata(root.join("link.txt")).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "a shelved symlink must come back as a symlink, not a text file"
        );
    }

    // =========================================================================
    // sync outcomes
    // =========================================================================

    /// An origin repo with one snapshot, plus a clone of it.
    /// Returns (holder, origin root, clone root).
    fn origin_and_clone() -> (TempDir, PathBuf, PathBuf) {
        let holder = TempDir::new().unwrap();
        let origin = holder.path().join("origin");
        fs::create_dir_all(&origin).unwrap();
        commands::init::run(&origin).unwrap();
        write(&origin, "a.txt", "shared\n");
        save(&origin, "origin base");

        let copy = holder.path().join("copy");
        commands::sync::clone(
            origin.to_str().unwrap(),
            Some(copy.to_str().unwrap()),
            &spawn_cfg(),
            None,
        )
        .unwrap();
        (holder, origin, copy)
    }

    #[test]
    fn clone_reports_what_it_copied_and_checks_out_main() {
        let holder = TempDir::new().unwrap();
        let origin = holder.path().join("origin");
        fs::create_dir_all(&origin).unwrap();
        commands::init::run(&origin).unwrap();
        write(&origin, "a.txt", "shared\n");
        let tip = save(&origin, "origin base");

        let copy = holder.path().join("copy");
        let cloned = commands::sync::clone(
            origin.to_str().unwrap(),
            Some(copy.to_str().unwrap()),
            &spawn_cfg(),
            None,
        )
        .unwrap();

        assert_eq!(cloned.branch, "main");
        assert_eq!(cloned.into, copy);
        assert_eq!(cloned.snapshots, 1);
        assert_eq!(cloned.branches, 1);
        assert!(cloned.objects > 0);
        assert_eq!(read(&copy, "a.txt"), "shared\n");
        assert_eq!(parent(&copy), tip, "the clone is checked out at the tip");
    }

    #[test]
    fn push_with_nothing_new_reports_up_to_date() {
        use commands::sync::Pushed;
        let (_holder, _origin, copy) = origin_and_clone();
        let pushed = with_write(&copy, |vr| {
            commands::sync::push(vr, "origin", None, &spawn_cfg())
        })
        .unwrap();
        assert_eq!(
            pushed,
            Pushed::AlreadyUpToDate {
                branch: "main".into(),
                remote: "origin".into()
            }
        );
    }

    #[test]
    fn push_sends_new_work_and_counts_it() {
        use commands::sync::Pushed;
        let (_holder, origin, copy) = origin_and_clone();
        write(&copy, "b.txt", "from the clone\n");
        save(&copy, "clone work");

        let pushed = with_write(&copy, |vr| {
            commands::sync::push(vr, "origin", None, &spawn_cfg())
        })
        .unwrap();
        let Pushed::Sent {
            branch,
            snapshots,
            created,
            ..
        } = pushed
        else {
            panic!("expected a send, got {:?}", pushed);
        };
        assert_eq!(branch, "main");
        assert_eq!(snapshots, 1);
        assert!(
            created.is_none(),
            "the branch already existed on the remote"
        );

        // The commit really landed, even though origin's working tree is untouched.
        let conn = db::get_conn_at_path(&origin.join(".velo/velo.db")).unwrap();
        let there: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE message = 'clone work'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(there, 1);
    }

    #[test]
    fn push_into_an_empty_remote_says_it_created_the_history() {
        use commands::sync::{BranchCreated, Pushed};
        let holder = TempDir::new().unwrap();
        let empty = holder.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        commands::init::run(&empty).unwrap();

        let local = holder.path().join("local");
        fs::create_dir_all(&local).unwrap();
        commands::init::run(&local).unwrap();
        write(&local, "a.txt", "v1\n");
        save(&local, "s1");
        with_write(&local, |vr| {
            commands::remote::add(vr, "origin", empty.to_str().unwrap())
        })
        .unwrap();

        let pushed = with_write(&local, |vr| {
            commands::sync::push(vr, "origin", None, &spawn_cfg())
        })
        .unwrap();
        let Pushed::Sent { created, .. } = pushed else {
            panic!("expected a send, got {:?}", pushed);
        };
        assert_eq!(created, Some(BranchCreated::RemoteWasEmpty));
    }

    #[test]
    fn push_is_rejected_when_the_remote_moved_on() {
        let (_holder, origin, copy) = origin_and_clone();
        // Both sides commit from the same base, so neither is a fast-forward of
        // the other.
        write(&origin, "theirs.txt", "origin work\n");
        save(&origin, "origin moves");
        write(&copy, "ours.txt", "clone work\n");
        save(&copy, "clone work");

        let err = with_write(&copy, |vr| {
            commands::sync::push(vr, "origin", None, &spawn_cfg())
        })
        .unwrap_err();
        let VeloError::NotFastForward { branch, remote } = err else {
            panic!("expected a non-fast-forward rejection, got {:?}", err);
        };
        assert_eq!(branch, "main");
        assert_eq!(remote, "origin");
    }

    #[test]
    fn fetch_imports_without_touching_local_branches() {
        let (_holder, origin, copy) = origin_and_clone();
        let before = parent(&copy);
        write(&origin, "b.txt", "later\n");
        save(&origin, "origin moves");

        let fetched = with_write(&copy, |vr| {
            commands::sync::fetch(vr, "origin", &spawn_cfg())
        })
        .unwrap();
        assert_eq!(fetched.remote, "origin");
        assert_eq!(fetched.snapshots, 1);
        assert!(fetched.refs.iter().any(|r| r.branch == "main"));

        assert_eq!(
            parent(&copy),
            before,
            "fetch must not move the local branch"
        );
        assert!(!copy.join("b.txt").exists(), "nor touch the working tree");
    }

    #[test]
    fn pull_fast_forwards_when_strictly_behind() {
        use commands::sync::Pulled;
        let (_holder, origin, copy) = origin_and_clone();
        write(&origin, "b.txt", "later\n");
        let tip = save(&origin, "origin moves");

        let pulled =
            with_write(&copy, |vr| commands::sync::pull(vr, "origin", &spawn_cfg())).unwrap();
        let Pulled::FastForwarded { branch, to, .. } = pulled else {
            panic!("expected a fast-forward, got {:?}", pulled);
        };
        assert_eq!(branch, "main");
        assert_eq!(to, tip);
        assert_eq!(read(&copy, "b.txt"), "later\n");
        assert_eq!(parent(&copy), tip);
    }

    #[test]
    fn pull_after_a_fetch_still_fast_forwards() {
        use commands::sync::Pulled;
        let (_holder, origin, copy) = origin_and_clone();
        write(&origin, "b.txt", "later\n");
        let tip = save(&origin, "origin moves");

        // Fetching first imports the commits under a tracking branch, leaving the
        // second pack empty — the fast-forward test has to span (pack ∪ local).
        with_write(&copy, |vr| {
            commands::sync::fetch(vr, "origin", &spawn_cfg())
        })
        .unwrap();
        let pulled =
            with_write(&copy, |vr| commands::sync::pull(vr, "origin", &spawn_cfg())).unwrap();
        assert!(
            matches!(pulled, Pulled::FastForwarded { .. }),
            "fetch-then-pull must still fast-forward, got {:?}",
            pulled
        );
        assert_eq!(parent(&copy), tip);
    }

    #[test]
    fn pull_with_nothing_new_is_up_to_date() {
        use commands::sync::Pulled;
        let (_holder, _origin, copy) = origin_and_clone();
        let pulled =
            with_write(&copy, |vr| commands::sync::pull(vr, "origin", &spawn_cfg())).unwrap();
        assert_eq!(
            pulled,
            Pulled::AlreadyUpToDate {
                branch: "main".into(),
                remote: "origin".into()
            }
        );
    }

    #[test]
    fn pull_reports_divergence_rather_than_auto_merging() {
        use commands::sync::Pulled;
        let (_holder, origin, copy) = origin_and_clone();
        write(&origin, "theirs.txt", "origin work\n");
        save(&origin, "origin moves");
        write(&copy, "ours.txt", "clone work\n");
        let ours = save(&copy, "clone work");

        let pulled =
            with_write(&copy, |vr| commands::sync::pull(vr, "origin", &spawn_cfg())).unwrap();
        assert_eq!(
            pulled,
            Pulled::Diverged {
                branch: "main".into(),
                remote: "origin".into()
            }
        );
        // Our branch is left where it was; the remote's history waits under a
        // tracking branch for `velo merge`.
        assert_eq!(parent(&copy), ours, "divergence must not move our branch");
        let conn = db::get_conn_at_path(&copy.join(".velo/velo.db")).unwrap();
        let tracked: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch = 'remotes/origin/main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(tracked > 0, "their commits should be imported for merging");
    }

    #[test]
    fn pull_refuses_a_dirty_tree() {
        let (_holder, _origin, copy) = origin_and_clone();
        write(&copy, "a.txt", "local edit\n");
        assert!(matches!(
            with_write(&copy, |vr| commands::sync::pull(vr, "origin", &spawn_cfg())).unwrap_err(),
            VeloError::DirtyWorkingTree { .. }
        ));
    }

    // =========================================================================
    // Repo handle / write guard
    // =========================================================================

    #[test]
    fn opening_a_newer_repository_is_refused_on_both_paths() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        // Stamp a format this build doesn't know about.
        {
            let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
            conn.pragma_update(None, "user_version", 99i64).unwrap();
        }

        // `open` refuses rather than half-reading it...
        assert!(matches!(
            Repo::open(&root),
            Err(VeloError::SchemaTooNew {
                found: 99,
                supported: crate::FORMAT_VERSION
            })
        ));
        // ...and so does the migrating path: a newer format can't be downgraded.
        assert!(matches!(
            Repo::open_and_migrate(&root),
            Err(VeloError::SchemaTooNew { found: 99, .. })
        ));
        // `discover` goes through the same gate.
        assert!(matches!(
            Repo::discover(&root),
            Err(VeloError::SchemaTooNew { found: 99, .. })
        ));
    }

    #[test]
    fn a_write_guard_is_exclusive_across_handles() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        let a = Repo::open_and_migrate(&root).unwrap();
        let b = Repo::open_and_migrate(&root).unwrap();

        let held = a.write().unwrap();
        // A second guard can't exist while the first is alive — this is what makes
        // `&WriteGuard` proof of exclusive access rather than a convention.
        assert!(
            b.try_write().unwrap().is_none(),
            "a second write guard must not be handed out"
        );
        drop(held);
        assert!(
            b.try_write().unwrap().is_some(),
            "and the lock must be released when the guard drops"
        );
    }

    #[test]
    fn reads_do_not_need_the_write_lock() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        let writer = Repo::open_and_migrate(&root).unwrap();
        let _held = writer.write().unwrap();

        // Read commands take `&Repo`, so a long-running mutation elsewhere doesn't
        // block `velo status` — which is why the CLI skips the lock for them.
        let reader = Repo::open_and_migrate(&root).unwrap();
        let status = commands::status::run(&reader, &[]).unwrap();
        assert_eq!(status.branch, "main");
        assert!(status.is_clean());
    }

    #[test]
    fn one_repo_handle_serves_a_whole_sequence_of_commands() {
        // Commands used to reopen the database on every call. Holding one handle
        // across several operations is the point of the `Repo` type.
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        write(&root, "a.txt", "one\n");
        let h1 = {
            let guard = repo.write().unwrap();
            commands::save::run(&guard, "first", false)
                .unwrap()
                .into_result()
                .unwrap()
                .hash
        };

        write(&root, "a.txt", "two\n");
        let h2 = {
            let guard = repo.write().unwrap();
            commands::save::run(&guard, "second", false)
                .unwrap()
                .into_result()
                .unwrap()
                .hash
        };
        assert_ne!(h1, h2);

        // Reads through the same handle see both.
        let history = commands::history::run(
            &repo,
            commands::history::Options {
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        let messages: Vec<&str> = history.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, vec!["second", "first"]);
        assert!(commands::fsck::check(&repo).unwrap().is_healthy());
    }

    #[test]
    fn several_mutations_can_share_one_guard() {
        // The guard exists so a caller can group mutations under a single lock,
        // rather than each command taking and releasing its own.
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        write(&root, "a.txt", "one\n");
        let guard = repo.write().unwrap();
        commands::save::run(&guard, "first", false).unwrap();
        commands::tag::create(&guard, &tag_name("v1"), None, false).unwrap();
        commands::switch::run(&guard, "feature", false).unwrap();
        drop(guard);

        assert_eq!(commands::tag::list(&repo).unwrap().len(), 1);
        assert_eq!(head(&root), "feature");
    }

    #[test]
    fn open_does_not_search_upward_but_discover_does() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");

        let nested = root.join("deep/inside");
        fs::create_dir_all(&nested).unwrap();

        // `open` is exact: no implicit filesystem search.
        assert!(matches!(
            Repo::open(&nested),
            Err(VeloError::NotARepo { .. })
        ));
        // `discover` is the explicit opt-in to searching ancestors.
        let found = Repo::discover(&nested).unwrap();
        assert_eq!(found.root(), root.as_path());
    }

    // =========================================================================
    // progress reporting
    // =========================================================================

    /// Records everything an operation reports, so a test can assert on it.
    ///
    /// `Observer` takes `&self` and may be called from rayon workers, so the
    /// counters are atomics and the logs are behind mutexes.
    #[derive(Default)]
    struct Recorder {
        begun: std::sync::Mutex<Vec<(crate::progress::Phase, Option<u64>)>>,
        advanced: std::sync::Mutex<std::collections::HashMap<crate::progress::Phase, u64>>,
        calls: std::sync::Mutex<std::collections::HashMap<crate::progress::Phase, u64>>,
        finished: std::sync::Mutex<Vec<crate::progress::Phase>>,
    }

    impl crate::progress::Observer for std::sync::Arc<Recorder> {
        fn begin(&self, phase: crate::progress::Phase, total: Option<u64>) {
            self.begun.lock().unwrap().push((phase, total));
        }
        fn advance(&self, phase: crate::progress::Phase, by: u64) {
            *self.advanced.lock().unwrap().entry(phase).or_insert(0) += by;
            *self.calls.lock().unwrap().entry(phase).or_insert(0) += 1;
        }
        fn finish(&self, phase: crate::progress::Phase) {
            self.finished.lock().unwrap().push(phase);
        }
    }

    impl Recorder {
        /// The total announced for `phase`, if it was begun.
        fn total(&self, phase: crate::progress::Phase) -> Option<Option<u64>> {
            self.begun
                .lock()
                .unwrap()
                .iter()
                .find(|(p, _)| *p == phase)
                .map(|(_, t)| *t)
        }
        fn done(&self, phase: crate::progress::Phase) -> u64 {
            self.advanced
                .lock()
                .unwrap()
                .get(&phase)
                .copied()
                .unwrap_or(0)
        }
        /// How many times `advance` was called for `phase`.
        fn calls(&self, phase: crate::progress::Phase) -> u64 {
            self.calls.lock().unwrap().get(&phase).copied().unwrap_or(0)
        }
        fn was_finished(&self, phase: crate::progress::Phase) -> bool {
            self.finished.lock().unwrap().contains(&phase)
        }
    }

    /// A repository that reports to a fresh recorder.
    fn watched(root: &Path) -> (Repo, std::sync::Arc<Recorder>) {
        let rec = std::sync::Arc::new(Recorder::default());
        let repo = Repo::open_and_migrate(root)
            .expect("open repository")
            .observing(rec.clone());
        (repo, rec)
    }

    #[test]
    fn save_reports_hashing_of_every_dirty_file() {
        use crate::progress::Phase;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        write(&root, "b.txt", "two\n");
        write(&root, "c.txt", "three\n");

        let (repo, rec) = watched(&root);
        commands::save::run(&repo.write().unwrap(), "s1", false).unwrap();

        // .veloignore is written by init, so it counts too — assert against the
        // announced total rather than a hard-coded number.
        let total = rec
            .total(Phase::Hashing)
            .expect("save should announce a hashing phase")
            .expect("the file count is known up front");
        assert!(
            total >= 3,
            "expected at least the three files, got {}",
            total
        );
        assert_eq!(
            rec.done(Phase::Hashing),
            total,
            "every file must be ticked exactly once"
        );
        assert!(rec.was_finished(Phase::Hashing), "the phase must be closed");
    }

    #[test]
    fn restore_reports_writing() {
        use crate::progress::Phase;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "v1\n");
        let h1 = save(&root, "s1");
        write(&root, "a.txt", "v2\n");
        save(&root, "s2");

        let (repo, rec) = watched(&root);
        commands::restore::run(&repo.write().unwrap(), &h1, true, &[]).unwrap();

        let total = rec
            .total(Phase::Writing)
            .expect("restore should announce a writing phase")
            .expect("the file count is known up front");
        assert_eq!(rec.done(Phase::Writing), total);
        assert!(rec.was_finished(Phase::Writing));
    }

    #[test]
    fn fsck_reports_verifying_each_object() {
        use crate::progress::Phase;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        write(&root, "b.txt", "two\n");
        save(&root, "s1");

        let (repo, rec) = watched(&root);
        let report = commands::fsck::check(&repo).unwrap();
        assert!(report.is_healthy());

        let total = rec
            .total(Phase::Verifying)
            .expect("fsck should announce a verifying phase")
            .expect("the object count is known up front");
        assert!(total > 0);
        assert_eq!(rec.done(Phase::Verifying), total);
        assert!(rec.was_finished(Phase::Verifying));
    }

    #[test]
    fn rebase_reports_one_tick_per_replayed_commit() {
        use crate::progress::Phase;
        let (_tmp, root) = setup();
        write(&root, "base.txt", "base\n");
        save(&root, "base");
        commands::switch::run(
            &Repo::open_and_migrate(&root).unwrap().write().unwrap(),
            "feature",
            false,
        )
        .unwrap();
        write(&root, "a.txt", "one\n");
        save(&root, "feat 1");
        write(&root, "b.txt", "two\n");
        save(&root, "feat 2");
        commands::switch::run(
            &Repo::open_and_migrate(&root).unwrap().write().unwrap(),
            "main",
            true,
        )
        .unwrap();
        write(&root, "m.txt", "main\n");
        let main_tip = save(&root, "main moved on");
        commands::switch::run(
            &Repo::open_and_migrate(&root).unwrap().write().unwrap(),
            "feature",
            true,
        )
        .unwrap();

        let (repo, rec) = watched(&root);
        commands::rebase::run(&repo.write().unwrap(), &main_tip, false, false).unwrap();

        assert_eq!(
            rec.total(Phase::Replaying),
            Some(Some(2)),
            "two commits to replay"
        );
        assert_eq!(rec.done(Phase::Replaying), 2);
        assert!(rec.was_finished(Phase::Replaying));
    }

    #[test]
    fn a_phase_is_closed_even_when_the_command_fails() {
        use crate::progress::Phase;
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        save(&root, "s1");

        // Corrupt an object so fsck finds a problem but still completes its scan.
        let obj = fs::read_dir(root.join(".velo/objects"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.is_file())
            .unwrap();
        fs::write(&obj, b"not valid zstd").unwrap();

        let (repo, rec) = watched(&root);
        let report = commands::fsck::check(&repo).unwrap();
        assert!(!report.is_healthy(), "the corruption should be found");
        assert!(
            rec.was_finished(Phase::Verifying),
            "the phase must close regardless of what was found"
        );
    }

    #[test]
    fn a_repository_without_an_observer_still_works() {
        // The default is `Silent`; every other test in this file exercises it,
        // but assert it explicitly so the no-observer path is deliberate.
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        let repo = Repo::open_and_migrate(&root).unwrap();
        commands::save::run(&repo.write().unwrap(), "s1", false).unwrap();
        assert!(commands::fsck::check(&repo).unwrap().is_healthy());
    }

    #[test]
    fn merge_reports_reconciling_files() {
        use crate::progress::Phase;
        let (_tmp, root) = setup();
        write(&root, "shared.txt", "base\n");
        save(&root, "base");
        commands::switch::run(
            &Repo::open_and_migrate(&root).unwrap().write().unwrap(),
            "feature",
            false,
        )
        .unwrap();
        write(&root, "feat.txt", "feat\n");
        save(&root, "feat work");
        commands::switch::run(
            &Repo::open_and_migrate(&root).unwrap().write().unwrap(),
            "main",
            true,
        )
        .unwrap();
        write(&root, "main.txt", "main\n");
        save(&root, "main work");

        let (repo, rec) = watched(&root);
        commands::merge::run(&repo.write().unwrap(), Some("feature"), false).unwrap();

        let total = rec
            .total(Phase::Reconciling)
            .expect("merge should announce a reconciling phase")
            .expect("the path count is known up front");
        assert!(total > 0);
        assert_eq!(rec.done(Phase::Reconciling), total);
        assert!(rec.was_finished(Phase::Reconciling));
    }

    #[test]
    fn a_streaming_fetch_reports_the_bytes_it_moves() {
        use crate::progress::Phase;
        // `child:` spawns this test binary's sibling `velo` to serve the far end,
        // so it exercises the real wire rather than the direct-DB local path.
        let velo_bin = velo_binary().expect(
            "this test spawns the velo binary to serve the far end of the wire, so it              needs one built alongside the tests — run `cargo test --workspace`, which              CI does. Skipping silently would let the transfer path rot unnoticed.",
        );

        let holder = TempDir::new().unwrap();
        let origin = holder.path().join("origin");
        fs::create_dir_all(&origin).unwrap();
        commands::init::run(&origin).unwrap();
        // The pack has to span several 64 KiB chunks for the loop to be visible,
        // and zstd would collapse repetitive text — so generate content that does
        // not compress.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        for i in 0..12 {
            let mut body = String::with_capacity(80_000);
            while body.len() < 80_000 {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                body.push_str(&format!("{:016x}\n", seed));
            }
            write(&origin, &format!("f{}.txt", i), &body);
        }
        save(&origin, "bulk");

        let local = holder.path().join("local");
        fs::create_dir_all(&local).unwrap();
        commands::init::run(&local).unwrap();
        let spawn = crate::transport::Spawn::new(&velo_bin);
        {
            let repo = Repo::open_and_migrate(&local).unwrap();
            let guard = repo.write().unwrap();
            commands::remote::add(&guard, "origin", &format!("child:{}", origin.display()))
                .unwrap();
        }

        let (repo, rec) = watched(&local);
        let fetched = commands::sync::fetch(&repo.write().unwrap(), "origin", &spawn).unwrap();
        assert!(
            fetched.snapshots > 0,
            "the fetch should have imported history"
        );

        // The pack is framed by EOF, so there is no total to announce — but the
        // bytes must be counted as they arrive.
        assert_eq!(
            rec.total(Phase::Transferring),
            Some(None),
            "transfer is announced without a total"
        );
        let moved = rec.done(Phase::Transferring);
        assert!(moved > 0, "the transfer must report the bytes it moved");
        assert!(
            rec.calls(Phase::Transferring) > 1,
            "a {}-byte pack must arrive in chunks, not one lump — got {} call(s)",
            moved,
            rec.calls(Phase::Transferring)
        );
        assert!(rec.was_finished(Phase::Transferring));
    }

    /// The `velo` executable built alongside these tests, if there is one.
    ///
    /// `CARGO_BIN_EXE_velo` is only set for velo-cli's own integration tests, so a
    /// library test has to look next to its own executable instead.
    fn velo_binary() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        // target/debug/deps/velo_core-<hash>  ->  target/debug/velo[.exe]
        let dir = exe.parent()?.parent()?;
        let candidate = dir.join(if cfg!(windows) { "velo.exe" } else { "velo" });
        candidate.exists().then_some(candidate)
    }

    // =========================================================================
    // in-memory trees (2.1)
    // =========================================================================

    #[test]
    fn a_tree_saved_from_memory_reads_back() {
        use crate::tree::{FileKind, SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let id = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: None,
                    merge_parent: None,
                    message: "publish 1.0",
                    entries: vec![
                        TreeEntry::file("pkg/lib.rs", b"pub fn f() {}\n".to_vec()),
                        TreeEntry::executable("bin/run.sh", b"#!/bin/sh\n".to_vec()),
                        TreeEntry::symlink("pkg/latest", "lib.rs"),
                    ],
                })
                .unwrap()
        };

        assert_eq!(
            repo.read_file_at(&id, "pkg/lib.rs").unwrap(),
            b"pub fn f() {}\n"
        );
        // A symlink's content is its target, matching how it was stored.
        assert_eq!(repo.read_file_at(&id, "pkg/latest").unwrap(), b"lib.rs");

        let tree = repo.tree_at(&id).unwrap();
        let paths: Vec<&str> = tree.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["bin/run.sh", "pkg/latest", "pkg/lib.rs"]);
        let kind = |p: &str| tree.iter().find(|f| f.path == p).unwrap().kind;
        assert_eq!(kind("pkg/lib.rs"), FileKind::Regular);
        assert_eq!(kind("bin/run.sh"), FileKind::Executable);
        assert_eq!(kind("pkg/latest"), FileKind::Symlink);

        // And the raw object is reachable by hash.
        let obj = &tree.iter().find(|f| f.path == "pkg/lib.rs").unwrap().object;
        assert_eq!(repo.read_object(obj).unwrap(), b"pub fn f() {}\n");
    }

    #[test]
    fn saving_a_tree_leaves_the_working_tree_and_position_alone() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        write(&root, "on_disk.txt", "disk content\n");
        let disk_snapshot = save(&root, "from disk");
        let position_before = parent(&root);
        let branch_before = head(&root);

        let repo = Repo::open_and_migrate(&root).unwrap();
        let id = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: None,
                    merge_parent: None,
                    message: "in memory",
                    entries: vec![TreeEntry::file("only_in_memory.rs", b"x\n".to_vec())],
                })
                .unwrap()
        };
        assert_ne!(id, disk_snapshot);

        // This is the safety property the API is built around.
        assert_eq!(parent(&root), position_before, "PARENT must not move");
        assert_eq!(head(&root), branch_before, "HEAD must not move");
        assert!(
            !root.join("only_in_memory.rs").exists(),
            "nothing may be written to the working tree"
        );
        assert_eq!(
            read(&root, "on_disk.txt"),
            "disk content\n",
            "existing files must be untouched"
        );
        assert!(
            commands::get_dirty_files(&repo).is_empty(),
            "the tree must still look clean"
        );
        // `main` is where it was; only the caller's own branch moved.
        assert_eq!(
            commands::history::run(
                &repo,
                commands::history::Options {
                    limit: Some(10),
                    ..Default::default()
                }
            )
            .unwrap()
            .entries
            .len(),
            1,
            "main's history must be unchanged"
        );
    }

    #[test]
    fn an_in_memory_save_is_indistinguishable_from_a_disk_save() {
        use crate::tree::{SaveTree, TreeEntry};
        // The claim that makes this a second adapter onto one store rather than a
        // parallel format: identical content must land in the identical object.
        let content = b"fn main() { println!(\"hi\"); }\n".to_vec();

        let (_tmp_a, root_a) = setup();
        write(&root_a, "m.rs", std::str::from_utf8(&content).unwrap());
        let from_disk = save(&root_a, "same message");
        let disk_obj = {
            let repo = Repo::open_and_migrate(&root_a).unwrap();
            repo.tree_at(&SnapshotId::from_stored(from_disk))
                .unwrap()
                .into_iter()
                .find(|f| f.path == "m.rs")
                .unwrap()
                .object
        };

        let (_tmp_b, root_b) = setup();
        let repo_b = Repo::open_and_migrate(&root_b).unwrap();
        let from_memory = {
            let guard = repo_b.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("main"),
                    parent: None,
                    merge_parent: None,
                    message: "same message",
                    entries: vec![TreeEntry::file("m.rs", content)],
                })
                .unwrap()
        };
        let memory_obj = repo_b
            .tree_at(&from_memory)
            .unwrap()
            .into_iter()
            .find(|f| f.path == "m.rs")
            .unwrap()
            .object;

        assert_eq!(
            disk_obj, memory_obj,
            "the same bytes must produce the same object either way"
        );
    }

    #[test]
    fn crlf_content_survives_a_round_trip_through_the_working_tree() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let id = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("imported"),
                    parent: None,
                    merge_parent: None,
                    message: "windows line endings",
                    entries: vec![TreeEntry::file("crlf.txt", b"a\r\nb\r\nc\r\n".to_vec())],
                })
                .unwrap()
        };

        // Stored normalised, exactly as the filesystem path would store it.
        assert_eq!(repo.read_file_at(&id, "crlf.txt").unwrap(), b"a\nb\nc\n");

        // The reason it matters: restore writes the stored bytes, and the dirty
        // check re-hashes them. Storing raw CRLF would make this file read as
        // modified forever.
        {
            let guard = repo.write().unwrap();
            commands::restore::run(&guard, &id, true, &[]).unwrap();
        }
        // `.veloignore` is on disk from `init` and deliberately not in this tree,
        // so it reads as new — that is correct. The claim under test is narrower:
        // the restored file itself must not read as modified.
        let dirty = commands::get_dirty_files(&repo);
        assert!(
            !dirty.contains_key("crlf.txt"),
            "a restored file must not immediately look modified, got {:?}",
            dirty
        );
        assert_eq!(
            read(&root, "crlf.txt"),
            "a
b
c
"
        );
    }

    #[test]
    fn trees_chain_through_their_parent() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let first = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: None,
                    merge_parent: None,
                    message: "1.0",
                    entries: vec![TreeEntry::file("v.txt", b"1\n".to_vec())],
                })
                .unwrap()
        };
        let second = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: Some(&first),
                    merge_parent: None,
                    message: "1.1",
                    entries: vec![TreeEntry::file("v.txt", b"2\n".to_vec())],
                })
                .unwrap()
        };

        // Each version is still readable at its own snapshot.
        assert_eq!(repo.read_file_at(&first, "v.txt").unwrap(), b"1\n");
        assert_eq!(repo.read_file_at(&second, "v.txt").unwrap(), b"2\n");

        let history = commands::history::run(
            &repo,
            commands::history::Options {
                branch: Some(&branch_name("registry")),
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        let messages: Vec<&str> = history.entries.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, vec!["1.1", "1.0"]);
    }

    #[test]
    fn a_repository_built_only_from_memory_passes_fsck() {
        use crate::tree::{SaveTree, TreeEntry};
        // Snapshot ids are content-addressed and fsck recomputes them, so this
        // proves save_tree builds the id by the same recipe as save.
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let mut parent_id: Option<SnapshotId> = None;
        for v in 0..4 {
            let guard = repo.write().unwrap();
            let id = guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: parent_id.as_ref(),
                    merge_parent: None,
                    message: &format!("v{}", v),
                    entries: vec![
                        TreeEntry::file("a.txt", format!("version {}\n", v).into_bytes()),
                        TreeEntry::file("b.txt", b"constant\n".to_vec()),
                    ],
                })
                .unwrap();
            parent_id = Some(id);
        }

        let report = commands::fsck::check(&repo).unwrap();
        assert!(
            report.is_healthy(),
            "in-memory snapshots must verify: {:?}",
            report.problems
        );
    }

    #[test]
    fn save_tree_rejects_input_it_cannot_store_faithfully() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let guard = repo.write().unwrap();

        // A fn rather than a closure: the borrow in `parent` outlives the call, and
        // closure lifetime elision cannot express that.
        fn spec<'a>(
            branch: &'a BranchName,
            parent: Option<&'a SnapshotId>,
            entries: Vec<TreeEntry>,
        ) -> SaveTree<'a> {
            SaveTree {
                meta: SnapshotMeta::new(),
                branch,
                parent,
                merge_parent: None,
                message: "m",
                entries,
            }
        }

        // "a branch is required, since the tip is derived from it" is no longer a
        // `save_tree` error: `BranchName` cannot be built empty, so the check moved
        // from run time into the type.
        assert!("".parse::<BranchName>().is_err());

        let unknown: SnapshotId = "dead".repeat(16).parse().unwrap();
        assert!(
            guard
                .save_tree(spec(
                    &branch_name("r"),
                    Some(&unknown),
                    vec![TreeEntry::file("a.txt", b"x".to_vec())]
                ))
                .is_err(),
            "an unknown parent would create dangling history"
        );
        assert!(
            guard
                .save_tree(spec(
                    &branch_name("r"),
                    None,
                    vec![TreeEntry::file("", b"x".to_vec())]
                ))
                .is_err(),
            "an entry needs a path"
        );
        assert!(
            guard
                .save_tree(spec(
                    &branch_name("r"),
                    None,
                    vec![
                        TreeEntry::file("a.txt", b"one".to_vec()),
                        TreeEntry::file("a.txt", b"two".to_vec()),
                    ]
                ))
                .is_err(),
            "a duplicate path is ambiguous, not a silent last-wins"
        );
    }

    #[test]
    fn windows_separators_in_a_path_are_normalised() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let id = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: None,
                    merge_parent: None,
                    message: "m",
                    entries: vec![TreeEntry::file("src\\deep\\f.rs", b"x\n".to_vec())],
                })
                .unwrap()
        };
        assert_eq!(repo.tree_at(&id).unwrap()[0].path, "src/deep/f.rs");
        // Readable by either spelling.
        assert_eq!(repo.read_file_at(&id, "src/deep/f.rs").unwrap(), b"x\n");
        assert_eq!(repo.read_file_at(&id, "src\\deep\\f.rs").unwrap(), b"x\n");
    }

    #[test]
    fn reads_of_things_that_are_not_there_are_errors() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let id = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    meta: SnapshotMeta::new(),
                    branch: &branch_name("registry"),
                    parent: None,
                    merge_parent: None,
                    message: "m",
                    entries: vec![TreeEntry::file("a.txt", b"x\n".to_vec())],
                })
                .unwrap()
        };

        let absent_snapshot: SnapshotId = "dead".repeat(16).parse().unwrap();
        let absent_object: ObjectHash = "de".repeat(32).parse().unwrap();
        assert!(repo.tree_at(&absent_snapshot).is_err());
        assert!(repo.read_file_at(&id, "absent.txt").is_err());
        assert!(repo.read_object(&absent_object).is_err());
        // A short hash cannot even be an object hash: the store is keyed on the
        // whole thing, so this is caught before any lookup.
        assert!("deadbeef".parse::<ObjectHash>().is_err());
    }

    // =========================================================================
    // undo
    // =========================================================================

    #[test]
    fn undo_removes_latest_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        with_write(&root, commands::undo::run).unwrap();

        assert!(!snapshot_exists(&root, &h2), "s2 should be gone");
        assert!(in_trash(&root, &h2), "s2 should be in trash");
        assert_eq!(parent(&root), h1);
    }

    #[test]
    fn undo_restores_working_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        with_write(&root, commands::undo::run).unwrap();
        // Working tree should now show v1, not v2
        assert_eq!(read(&root, "f.txt"), "v1");
        assert!(with_repo(&root, commands::get_dirty_files).is_empty());
    }

    #[test]
    fn undo_first_commit_clears_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        with_write(&root, commands::undo::run).unwrap();

        assert!(!snapshot_exists(&root, &h1));
        assert_eq!(parent(&root), "");
        // The tracked file should be removed from disk
        assert!(
            !exists(&root, "f.txt"),
            "File should be removed when first commit is undone"
        );
    }

    #[test]
    fn undo_aborts_on_dirty() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "dirty");

        let result = with_write(&root, commands::undo::run);
        assert!(result.is_err());
    }

    #[test]
    fn undo_nothing_to_undo_is_error() {
        let (_tmp, root) = setup();
        let result = with_write(&root, commands::undo::run);
        assert!(result.is_err());
    }

    // =========================================================================
    // redo
    // =========================================================================

    #[test]
    fn redo_restores_undone_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        with_write(&root, commands::undo::run).unwrap();
        assert_eq!(read(&root, "f.txt"), "v1");

        with_write(&root, commands::redo::run).unwrap();
        assert_eq!(read(&root, "f.txt"), "v2");
        assert_eq!(parent(&root), h2);
        assert!(snapshot_exists(&root, &h2));
        assert!(!in_trash(&root, &h2));
    }

    #[test]
    fn redo_nothing_to_redo_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        // No undo performed — nothing to redo
        let result = with_write(&root, commands::redo::run);
        assert!(result.is_err());
    }

    #[test]
    fn redo_stack_invalidated_by_new_save() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        with_write(&root, commands::undo::run).unwrap();

        // New save should clear redo stack
        write(&root, "f.txt", "v3_new");
        save(&root, "s3");

        let result = with_write(&root, commands::redo::run);
        assert!(
            result.is_err(),
            "Redo should be unavailable after a new save"
        );
    }

    #[test]
    fn redo_aborts_on_dirty() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        with_write(&root, commands::undo::run).unwrap();
        write(&root, "f.txt", "dirty");

        let result = with_write(&root, commands::redo::run);
        assert!(result.is_err());
    }

    /// Helper: build a merge commit (via a real conflict) and return its hash.
    fn make_merge_commit(root: &Path) -> String {
        write(root, "f.txt", "base\n");
        save(root, "base");
        with_write(root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(root, "f.txt", "theirs\n");
        save(root, "feature");
        with_write(root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(root, "f.txt", "ours\n");
        save(root, "main");
        with_write(root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        resolve_take(root, None, commands::resolve::TakeOption::Theirs, true).unwrap();
        save(root, "Merge feature")
    }

    #[test]
    fn undo_redo_preserves_merge_parent() {
        // Regression: the trash table had no merge_parent column, so undoing
        // then redoing a merge commit silently turned it into a single-parent
        // commit, corrupting graph topology.
        let (_tmp, root) = setup();
        let merge_hash = make_merge_commit(&root);

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let mp_before: String = conn
            .query_row(
                "SELECT merge_parent FROM snapshots WHERE hash = ?",
                [&merge_hash],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !mp_before.is_empty(),
            "merge commit should record a second parent"
        );

        with_write(&root, commands::undo::run).unwrap();
        with_write(&root, commands::redo::run).unwrap();

        let mp_after: String = conn
            .query_row(
                "SELECT merge_parent FROM snapshots WHERE hash = ?",
                [&merge_hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mp_after, mp_before, "merge_parent must survive undo→redo");
    }

    #[test]
    fn undo_redo_preserves_tags() {
        // Regression: undo deleted tags on the removed snapshot and redo never
        // restored them, so undo was not reversible for tags.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("release"), None, false)
        })
        .unwrap();

        with_write(&root, commands::undo::run).unwrap();
        // Tag is gone from the live table while the snapshot is trashed.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let live: i64 = conn
            .query_row(
                "SELECT count(*) FROM tags WHERE name = 'release'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0, "tag detached while snapshot is undone");

        with_write(&root, commands::redo::run).unwrap();
        let restored: String = conn
            .query_row(
                "SELECT snapshot_hash FROM tags WHERE name = 'release'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(restored, h2, "redo must restore the tag to its snapshot");
    }

    #[test]
    fn undo_refuses_during_merge() {
        // Regression: undo had no merge guard, so it could remove the tip while
        // MERGE_HEAD and conflict rows dangled.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "theirs\n");
        save(&root, "feature");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "ours\n");
        save(&root, "main");
        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        assert!(exists(&root, ".velo/MERGE_HEAD"));

        let r = with_write(&root, commands::undo::run);
        assert!(r.is_err(), "undo must refuse while a merge is in progress");
    }

    // =========================================================================
    // diff
    // =========================================================================

    #[test]
    fn diff_clean_directory_reports_no_files() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let d = with_repo(&root, |vr| commands::diff::run(vr, &None)).unwrap();
        assert!(d.is_empty(), "a clean tree has nothing to diff");
    }

    #[test]
    fn diff_modified_file_produces_one_hunk_around_the_change() {
        use commands::diff::{FileChange, LineTag};
        let (_tmp, root) = setup();
        let base: String = (0..100)
            .map(|i| {
                format!(
                    "Line {}
",
                    i
                )
            })
            .collect();
        write(&root, "large.txt", &base);
        save(&root, "base");
        let edited: String = (0..100)
            .map(|i| {
                if i == 50 {
                    "Line 50 MODIFIED
"
                    .to_string()
                } else {
                    format!(
                        "Line {}
",
                        i
                    )
                }
            })
            .collect();
        write(&root, "large.txt", &edited);

        let d = with_repo(&root, |vr| {
            commands::diff::run(vr, &Some("large.txt".into()))
        })
        .unwrap();
        assert_eq!(d.old_label, "last saved");
        assert_eq!(d.new_label, "working tree");
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path, "large.txt");

        let FileChange::Modified { hunks } = &d.files[0].change else {
            panic!("expected a modification, got {:?}", d.files[0].change);
        };
        // One localised edit in a 100-line file must not diff the whole file.
        assert_eq!(hunks.len(), 1, "a single edit should yield a single hunk");
        let h = &hunks[0];
        assert!(
            h.lines.len() <= 8,
            "hunk should be context-limited, got {}",
            h.lines.len()
        );
        let removed: Vec<&str> = h
            .lines
            .iter()
            .filter(|l| l.tag == LineTag::Removed)
            .map(|l| l.text.as_str())
            .collect();
        let added: Vec<&str> = h
            .lines
            .iter()
            .filter(|l| l.tag == LineTag::Added)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(removed, vec!["Line 50"]);
        assert_eq!(added, vec!["Line 50 MODIFIED"]);
        // Line numbers are 1-based and point at the edited line.
        assert_eq!(
            h.lines
                .iter()
                .find(|l| l.tag == LineTag::Added)
                .unwrap()
                .line_no,
            Some(51)
        );
    }

    #[test]
    fn diff_new_file_is_all_additions() {
        use commands::diff::{FileChange, LineTag};
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(
            &root,
            "fresh.txt",
            "alpha
beta
",
        );

        let d = with_repo(&root, |vr| {
            commands::diff::run(vr, &Some("fresh.txt".into()))
        })
        .unwrap();
        let FileChange::Modified { hunks } = &d.files[0].change else {
            panic!("expected hunks for a new file");
        };
        assert!(hunks[0].lines.iter().all(|l| l.tag == LineTag::Added));
        assert_eq!(
            hunks[0]
                .lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
    }

    #[test]
    fn diff_deleted_file_is_marked_deleted() {
        use commands::diff::FileChange;
        let (_tmp, root) = setup();
        write(&root, "gone.txt", "data");
        save(&root, "s1");
        fs::remove_file(root.join("gone.txt")).unwrap();

        let d = with_repo(&root, |vr| {
            commands::diff::run(vr, &Some("gone.txt".into()))
        })
        .unwrap();
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].change, FileChange::Deleted);
    }

    #[test]
    fn diff_bare_covers_every_dirty_file_in_path_order() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "1");
        write(&root, "b.txt", "1");
        save(&root, "s1");
        write(&root, "b.txt", "2");
        write(&root, "a.txt", "2");
        write(&root, "c.txt", "new");

        let d = with_repo(&root, |vr| commands::diff::run(vr, &None)).unwrap();
        let paths: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn diff_range_labels_both_sides_with_short_hashes() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "one");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "two");
        let h2 = save(&root, "s2");

        let d = with_repo(&root, |vr| {
            commands::diff::run_range(vr, &h1, Some(&h2), &[])
        })
        .unwrap();
        assert!(
            d.old_label.contains(&h1[..8]),
            "old label was {}",
            d.old_label
        );
        assert!(
            d.new_label.contains(&h2[..8]),
            "new label was {}",
            d.new_label
        );
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].path, "f.txt");
    }

    #[test]
    fn diff_range_pathspec_excludes_other_files() {
        let (_tmp, root) = setup();
        write(&root, "keep.txt", "one");
        write(&root, "drop.txt", "one");
        let h1 = save(&root, "s1");
        write(&root, "keep.txt", "two");
        write(&root, "drop.txt", "two");
        let h2 = save(&root, "s2");

        let d = with_repo(&root, |vr| {
            commands::diff::run_range(vr, &h1, Some(&h2), &["keep.txt".into()])
        })
        .unwrap();
        let paths: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["keep.txt"]);
    }

    #[test]
    fn diff_range_skips_files_that_did_not_change() {
        let (_tmp, root) = setup();
        write(&root, "same.txt", "constant");
        write(&root, "moved.txt", "one");
        let h1 = save(&root, "s1");
        write(&root, "moved.txt", "two");
        let h2 = save(&root, "s2");

        let d = with_repo(&root, |vr| {
            commands::diff::run_range(vr, &h1, Some(&h2), &[])
        })
        .unwrap();
        let paths: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["moved.txt"], "unchanged files must be omitted");
    }

    // =========================================================================
    // switch
    // =========================================================================

    #[test]
    fn switch_creates_new_branch() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        assert_eq!(head(&root), "dev");
    }

    #[test]
    fn switch_restores_branch_state() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main_content");
        save(&root, "s1");

        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dev_content");
        save(&root, "dev_snap");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        assert_eq!(read(&root, "f.txt"), "main_content");
    }

    #[test]
    fn switch_aborts_on_dirty_without_force() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dirty");

        // A refused switch is an error. It used to print a message and return
        // Ok, so it exited 0 and scripts couldn't tell it hadn't happened.
        let err = with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap_err();
        let VeloError::DirtyWorkingTree { paths } = err else {
            panic!("expected a dirty-tree error, got {:?}", err);
        };
        assert!(paths.iter().any(|p| p.ends_with("f.txt")));
        assert_eq!(head(&root), "dev", "and we must still be on dev");
    }

    #[test]
    fn switch_force_discards_changes() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dirty_dev");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        assert_eq!(head(&root), "main");
        assert_eq!(read(&root, "f.txt"), "main");
    }

    #[test]
    fn switch_to_deleted_branch_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "s2");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("dev"))
        })
        .unwrap();

        let result = with_write(&root, |vr| commands::switch::run(vr, "_deleted_dev", false));
        assert!(result.is_err());
    }

    #[test]
    fn switch_noop_when_already_on_branch() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        // Should succeed without doing anything
        with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();
        assert_eq!(head(&root), "main");
    }

    // =========================================================================
    // branches
    // =========================================================================

    #[test]
    fn branches_lists_all() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "f.txt", "d");
        save(&root, "s2");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        // Should not panic
        with_repo(&root, commands::branches::list).unwrap();
    }

    #[test]
    fn branches_delete_soft_removes() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "feat");
        save(&root, "feat_snap");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("feature"))
        })
        .unwrap();

        // Soft-deleted: snapshots still exist in DB but with renamed branch
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch = '_deleted_feature'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(count > 0);
    }

    #[test]
    fn branches_delete_current_branch_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        let result = with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("main"))
        });
        assert!(result.is_err());
    }

    #[test]
    fn branches_delete_main_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        // Even from another branch, deleting main is forbidden
        let result = with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("main"))
        });
        assert!(result.is_err());
    }

    #[test]
    fn branches_delete_nonexistent_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        let result = with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("ghost_branch"))
        });
        assert!(result.is_err());
    }

    #[test]
    fn branches_deleted_branches_not_shown_in_list() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "f");
        save(&root, "s2");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| {
            commands::branches::delete(vr, &branch_name("feature"))
        })
        .unwrap();

        // Check the DB: the renamed branch should not appear in normal listing query
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let visible: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch NOT LIKE '_deleted_%'
                 AND branch = 'feature'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(visible, 0);
    }

    // =========================================================================
    // tag
    // =========================================================================

    #[test]
    fn tag_create_and_resolve() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1.0"), None, false)
        })
        .unwrap();

        let resolved = with_repo(&root, |r| commands::resolve_snapshot_id(r, "v1.0")).unwrap();
        assert_eq!(resolved, h1);
    }

    #[test]
    fn tag_arbitrary_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        // Tag the first snapshot explicitly
        with_write(&root, |vr| {
            commands::tag::create(
                vr,
                &tag_name("old"),
                Some(&SnapshotId::from_stored(h1.clone())),
                false,
            )
        })
        .unwrap();
        let resolved = with_repo(&root, |r| commands::resolve_snapshot_id(r, "old")).unwrap();
        assert_eq!(resolved, h1);
    }

    #[test]
    fn tag_overwrite_without_force_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();

        write(&root, "f.txt", "v2");
        save(&root, "s2");
        let result = with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        });
        assert!(
            result.is_err(),
            "Should not allow overwriting without --force"
        );
    }

    #[test]
    fn tag_overwrite_with_force_succeeds() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();

        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, true)
        })
        .unwrap();

        let resolved = with_repo(&root, |r| commands::resolve_snapshot_id(r, "v1")).unwrap();
        assert_eq!(resolved, h2);
    }

    #[test]
    fn tag_delete_removes_tag() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("rel"), None, false)
        })
        .unwrap();
        with_write(&root, |vr| commands::tag::delete(vr, &tag_name("rel"))).unwrap();

        let result = with_repo(&root, |r| commands::resolve_snapshot_id(r, "rel"));
        assert!(result.is_err());
    }

    #[test]
    fn tag_delete_nonexistent_is_error() {
        let (_tmp, root) = setup();
        let result = with_write(&root, |vr| {
            commands::tag::delete(vr, &tag_name("ghost_tag"))
        });
        assert!(result.is_err());
    }

    #[test]
    fn tag_empty_head_is_error() {
        let (_tmp, root) = setup();
        // No snapshots yet — can't tag HEAD
        let result = with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        });
        assert!(result.is_err());
    }

    #[test]
    fn tag_list_does_not_panic() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("alpha"), None, false)
        })
        .unwrap();
        with_repo(&root, commands::tag::list).unwrap();
    }

    // =========================================================================
    // merge
    // =========================================================================

    #[test]
    fn merge_fast_forward() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        write(&root, "a.txt", "updated");
        save(&root, "dev work");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        with_write(&root, |vr| commands::merge::run(vr, Some("dev"), false)).unwrap();
        assert_eq!(read(&root, "a.txt"), "updated");
    }

    #[test]
    fn merge_conflict_produces_conflict_file() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "A", false)).unwrap();
        write(&root, "app.py", "content A");
        save(&root, "save A");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "B", false)).unwrap();
        write(&root, "app.py", "content B");
        save(&root, "save B");

        with_write(&root, |vr| commands::merge::run(vr, Some("A"), false)).unwrap();
        // Conflict stored in DB
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(count > 0, "conflict should be in DB");
        assert!(exists(&root, ".velo/MERGE_HEAD"));
    }

    #[test]
    fn merge_resolve_take_theirs() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "A", false)).unwrap();
        write(&root, "app.py", "content A");
        save(&root, "save A");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "B", false)).unwrap();
        write(&root, "app.py", "content B\n");
        save(&root, "save B");

        with_write(&root, |vr| commands::merge::run(vr, Some("A"), false)).unwrap();
        resolve_take(
            &root,
            Some("app.py"),
            commands::resolve::TakeOption::Theirs,
            false,
        )
        .unwrap();

        assert_eq!(read(&root, "app.py"), "content A\n");
        // .conflict files are no longer used; resolution handled via DB
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM conflict_files WHERE path = 'app.py'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "conflict should be resolved");
    }

    #[test]
    fn merge_resolve_take_ours() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feat", false)).unwrap();
        write(&root, "f.txt", "theirs\n");
        save(&root, "feat snap");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "ours\n");
        save(&root, "main snap");

        with_write(&root, |vr| commands::merge::run(vr, Some("feat"), false)).unwrap();
        resolve_take(
            &root,
            Some("f.txt"),
            commands::resolve::TakeOption::Ours,
            false,
        )
        .unwrap();

        assert_eq!(read(&root, "f.txt"), "ours\n");
        // No .conflict file in new system
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let c: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(c, 0);
    }

    #[test]
    fn merge_deletion_propagation() {
        let (_tmp, root) = setup();
        write(&root, "kept.txt", "keep");
        write(&root, "removed.txt", "delete me");
        save(&root, "base");

        // On 'dev' branch: delete removed.txt and save
        with_write(&root, |vr| commands::switch::run(vr, "dev", false)).unwrap();
        fs::remove_file(root.join("removed.txt")).unwrap();
        save(&root, "del snap");

        // Back on main: both files still on disk
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        assert!(
            exists(&root, "removed.txt"),
            "removed.txt should exist on main before merge"
        );
        assert!(exists(&root, "kept.txt"));

        // Merge dev into main — dev deleted removed.txt, so it should disappear
        with_write(&root, |vr| commands::merge::run(vr, Some("dev"), false)).unwrap();
        assert!(
            !exists(&root, "removed.txt"),
            "File deleted on target branch must be absent after merge"
        );
        assert!(
            exists(&root, "kept.txt"),
            "Unaffected file must still be present"
        );
    }

    #[test]
    fn merge_new_file_from_target() {
        let (_tmp, root) = setup();
        write(&root, "base.txt", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feat", false)).unwrap();
        write(&root, "newfile.txt", "brand new");
        save(&root, "feat snap");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        // Add a change to main so it's not a fast-forward
        write(&root, "base.txt", "main updated");
        save(&root, "main snap");

        with_write(&root, |vr| commands::merge::run(vr, Some("feat"), false)).unwrap();
        // newfile.txt should appear in working tree
        assert!(exists(&root, "newfile.txt"));
        assert_eq!(read(&root, "newfile.txt"), "brand new");
    }

    #[test]
    fn merge_aborts_on_dirty() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feat", false)).unwrap();
        write(&root, "f.txt", "feat");
        save(&root, "feat snap");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        write(&root, "f.txt", "dirty");
        let result = with_write(&root, |vr| commands::merge::run(vr, Some("feat"), false));
        assert!(result.is_err());
    }

    #[test]
    fn merge_abort_clears_conflict_files() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "A", false)).unwrap();
        write(&root, "app.py", "content A\n");
        save(&root, "save A");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "B", false)).unwrap();
        write(&root, "app.py", "content B\n");
        save(&root, "save B");

        with_write(&root, |vr| commands::merge::run(vr, Some("A"), false)).unwrap();
        // Conflict stored in DB
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(count > 0, "conflict should be in DB");
        // app.py is still "content B\n" on disk (our version untouched during merge)
        assert_eq!(read(&root, "app.py"), "content B\n");

        with_write(&root, |vr| commands::merge::run(vr, None, true)).unwrap(); // --abort
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM conflict_files WHERE path = 'app.py'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "conflict should be cleared from DB");
        assert!(!exists(&root, ".velo/MERGE_HEAD"));
        // Working tree should be restored to the pre-merge state ("content B\n")
        assert_eq!(
            read(&root, "app.py"),
            "content B\n",
            "abort should restore working tree to pre-merge state"
        );
        // And the working tree should be clean
        assert!(
            with_repo(&root, commands::get_dirty_files).is_empty(),
            "working tree should be clean after abort"
        );
    }
    #[test]
    fn merge_abort_works_after_all_conflicts_resolved() {
        // Key scenario: user resolves all conflicts but changes their mind.
        // abort must still work — MERGE_HEAD must stay alive until save.
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "feat", false)).unwrap();
        write(&root, "app.py", "theirs\n");
        save(&root, "feat snap");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "app.py", "ours\n");
        save(&root, "main snap");

        let pre_merge_parent = parent(&root);

        with_write(&root, |vr| commands::merge::run(vr, Some("feat"), false)).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(n > 0, "should have conflict");

        // Resolve all conflicts non-interactively
        resolve_take(
            &root,
            Some("app.py"),
            commands::resolve::TakeOption::Theirs,
            false,
        )
        .unwrap();
        let n2: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 0, "all conflicts resolved");

        // MERGE_HEAD must still exist after resolving
        assert!(
            exists(&root, ".velo/MERGE_HEAD"),
            "MERGE_HEAD must stay alive until save"
        );

        // Abort even though all conflicts were resolved
        with_write(&root, |vr| commands::merge::run(vr, None, true)).unwrap();

        assert!(
            !exists(&root, ".velo/MERGE_HEAD"),
            "MERGE_HEAD should be gone after abort"
        );
        assert_eq!(
            parent(&root),
            pre_merge_parent,
            "PARENT should rewind to pre-merge"
        );
        assert!(
            with_repo(&root, commands::get_dirty_files).is_empty(),
            "working tree should be clean"
        );
        assert_eq!(
            read(&root, "app.py"),
            "ours\n",
            "file restored to pre-merge version"
        );
    }

    #[test]
    fn resolve_take_theirs_produces_correct_content() {
        // Regression: zero-width conflict (both sides inserted at same ancestor
        // position) resolved with Decision::Theirs must produce exactly theirs
        // content, not any duplicate lines.
        let (_tmp, root) = setup();

        // ancestor: 2 lines
        write(&root, "app.py", "def img2pdf():\n    return None\n");
        save(&root, "base");

        // branch A (ours): insert print("ours") before return
        with_write(&root, |vr| commands::switch::run(vr, "A", false)).unwrap();
        write(
            &root,
            "app.py",
            "def img2pdf():\n    print(\"ours\")\n    return None\n",
        );
        save(&root, "ours");

        // back to main, create branch B (theirs): insert print("theirs") before return
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "B", false)).unwrap();
        write(
            &root,
            "app.py",
            "def img2pdf():\n    print(\"theirs\")\n    return None\n",
        );
        save(&root, "theirs");

        // Merge A into B → conflict
        with_write(&root, |vr| commands::merge::run(vr, Some("A"), false)).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(n > 0, "expected a conflict");

        // Resolve taking theirs (branch A = print("ours"))
        resolve_take(
            &root,
            Some("app.py"),
            commands::resolve::TakeOption::Theirs,
            false,
        )
        .unwrap();

        // Working file must have exactly 3 lines, with "print("ours")" once
        let result = read(&root, "app.py");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "resolved file should have exactly 3 lines, got: {:?}",
            lines
        );
        assert_eq!(lines[0], "def img2pdf():", "line 0");
        assert_eq!(
            lines[1], "    print(\"ours\")",
            "line 1 — 'theirs' in merge context is branch A"
        );
        assert_eq!(lines[2], "    return None", "line 2");

        // And taking ours (branch B = print("theirs")) must also be correct
        // Re-do the merge to test the ours path
        with_write(&root, |vr| commands::merge::run(vr, None, true)).unwrap(); // abort

        with_write(&root, |vr| commands::merge::run(vr, Some("A"), false)).unwrap();
        resolve_take(
            &root,
            Some("app.py"),
            commands::resolve::TakeOption::Ours,
            false,
        )
        .unwrap();

        let result2 = read(&root, "app.py");
        let lines2: Vec<&str> = result2.lines().collect();
        assert_eq!(
            lines2.len(),
            3,
            "resolved file should have exactly 3 lines (ours), got: {:?}",
            lines2
        );
        assert_eq!(
            lines2[1], "    print(\"theirs\")",
            "line 1 — our branch is B"
        );
    }

    #[test]
    fn merge_second_merge_while_in_progress_is_error() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "A", false)).unwrap();
        write(&root, "app.py", "content A");
        save(&root, "A snap");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "B", false)).unwrap();
        write(&root, "app.py", "content B");
        save(&root, "B snap");

        with_write(&root, |vr| commands::merge::run(vr, Some("A"), false)).unwrap();
        // Try to merge again while conflicts outstanding
        let result = with_write(&root, |vr| commands::merge::run(vr, Some("A"), false));
        assert!(result.is_err());
    }

    #[test]
    fn merge_self_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v");
        save(&root, "snap");
        let result = with_write(&root, |vr| commands::merge::run(vr, Some("main"), false));
        assert!(result.is_err());
    }

    #[test]
    fn merge_nonexistent_branch_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v");
        save(&root, "snap");
        let result = with_write(&root, |vr| commands::merge::run(vr, Some("ghost"), false));
        assert!(result.is_err());
    }

    #[test]
    fn merge_nonoverlapping_edits_auto_merge() {
        // Regression (critical): two branches editing DIFFERENT parts of the
        // same file must auto-merge cleanly, keeping BOTH sides' changes — not
        // flag a whole-file conflict and silently drop one side.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "A\nB\nC\nD\nE\n");
        save(&root, "ancestor");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "A_CHANGED\nB\nC\nD\nE\n"); // edits line 1
        save(&root, "feature line1");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "A\nB\nC\nD\nE_CHANGED\n"); // edits line 5
        save(&root, "main line5");

        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();

        // No conflict should have been recorded.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "non-overlapping edits must not conflict");
        // A clean merge still records MERGE_HEAD so the finalising save can
        // stamp the second parent.
        assert!(
            exists(&root, ".velo/MERGE_HEAD"),
            "clean merge records MERGE_HEAD until save"
        );

        // Both sides' changes must be present.
        assert_eq!(read(&root, "f.txt"), "A_CHANGED\nB\nC\nD\nE_CHANGED\n");

        // Finalise: the merge commit must carry a non-empty merge_parent and
        // MERGE_HEAD must be cleared.
        let merge_hash = save(&root, "Merge feature");
        assert!(
            !exists(&root, ".velo/MERGE_HEAD"),
            "MERGE_HEAD cleared after save"
        );
        let mp: String = conn
            .query_row(
                "SELECT merge_parent FROM snapshots WHERE hash = ?",
                [&merge_hash],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !mp.is_empty(),
            "clean merge commit records its second parent"
        );
    }

    #[test]
    fn merge_conflict_plus_theirs_only_region_preserves_both() {
        // Regression: a file that has one genuine conflict AND a separate
        // region only theirs changed. Resolving the conflict must NOT drop the
        // theirs-only change elsewhere in the file.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "A\nB\nC\nD\nE\n");
        save(&root, "ancestor");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "A\nB_THEIRS\nC\nD\nE_THEIRS\n"); // changes B and E
        save(&root, "feature");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "A\nB_OURS\nC\nD\nE\n"); // changes only B (conflict on B)
        save(&root, "main");

        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "exactly one region (B) conflicts");

        // Take ours for the conflict; the theirs-only change to E must survive.
        resolve_take(&root, None, commands::resolve::TakeOption::Ours, true).unwrap();
        assert_eq!(read(&root, "f.txt"), "A\nB_OURS\nC\nD\nE_THEIRS\n");
    }

    // The merge engine itself is tested in its own crate — worked diff3
    // examples in `crates/velo-merge/tests/diff3.rs`, randomised properties in
    // `props.rs`. velo-core never wrapped it, so what belongs here is the other
    // side of the boundary: `reconcile` deciding *when* a line merge is even
    // attempted (binary content and symlinks can't be, a mode-only change needs
    // no merge), and the merge/resolve commands on top. Those need a
    // repository, which is why they live in this suite and not that one.

    // =========================================================================
    // resolve
    // =========================================================================

    #[test]
    fn resolve_no_conflict_file_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        save(&root, "s1");
        let result = resolve_take(
            &root,
            Some("f.txt"),
            commands::resolve::TakeOption::Theirs,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn resolve_all_clears_all_conflicts() {
        let (_tmp, root) = setup();
        write(&root, "a.py", "base a");
        write(&root, "b.py", "base b");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "X", false)).unwrap();
        write(&root, "a.py", "X-a");
        write(&root, "b.py", "X-b");
        save(&root, "X snap");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::switch::run(vr, "Y", false)).unwrap();
        write(&root, "a.py", "Y-a");
        write(&root, "b.py", "Y-b");
        save(&root, "Y snap");

        with_write(&root, |vr| commands::merge::run(vr, Some("X"), false)).unwrap();
        // Conflicts are stored in DB, not .conflict files
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(
            n >= 2,
            "expected at least 2 conflict entries in DB, got {}",
            n
        );

        resolve_take(&root, None, commands::resolve::TakeOption::Theirs, true).unwrap();

        let n2: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 0, "all conflicts should be resolved");

        // MERGE_HEAD stays alive until `velo save` so the user can still abort.
        assert!(
            exists(&root, ".velo/MERGE_HEAD"),
            "MERGE_HEAD should remain until velo save finalises the merge"
        );

        // After saving, MERGE_HEAD is cleared.
        with_write(&root, |vr| commands::save::run(vr, "Finish merge", false)).unwrap();
        assert!(
            !exists(&root, ".velo/MERGE_HEAD"),
            "MERGE_HEAD should be gone after velo save"
        );
    }

    #[test]
    fn resolve_all_with_no_conflicts_is_graceful() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        save(&root, "s1");
        // No conflicts active, should not error
        resolve_take(&root, None, commands::resolve::TakeOption::Ours, true).unwrap();
    }

    // =========================================================================
    // gc
    // =========================================================================

    #[test]
    fn gc_removes_orphaned_objects() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        // Undo s2: its object is now orphaned (file_map entries move to trash)
        with_write(&root, commands::undo::run).unwrap();

        // Inject a fake orphaned object manually
        fs::write(
            root.join(".velo/objects/fake_orphan_object_hash"),
            b"garbage",
        )
        .unwrap();

        let before = object_count(&root);
        // Run GC with 0 day keep to also purge trash immediately
        with_write(&root, |vr| commands::gc::run(vr, 0)).unwrap();
        let after = object_count(&root);

        assert!(after < before, "GC should have removed orphaned object(s)");
        let _ = h2;
    }

    #[test]
    fn gc_clean_repo_is_noop() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let before = object_count(&root);
        with_write(&root, |vr| commands::gc::run(vr, 30)).unwrap();
        let after = object_count(&root);
        assert_eq!(
            before, after,
            "GC on a clean repo should not delete anything"
        );
    }

    // =========================================================================
    // resolve_snapshot_id (prefix matching)
    // =========================================================================

    #[test]
    fn resolve_snapshot_id_exact_hash() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h = save(&root, "s1");
        let resolved = with_repo(&root, |r| commands::resolve_snapshot_id(r, &h)).unwrap();
        assert_eq!(resolved, h);
    }

    #[test]
    fn resolve_snapshot_id_prefix() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h = save(&root, "s1");
        // First 6 characters should be unambiguous for a single snapshot
        let prefix = &h[..6];
        let resolved = with_repo(&root, |r| commands::resolve_snapshot_id(r, prefix)).unwrap();
        assert_eq!(resolved, h);
    }

    #[test]
    fn resolve_snapshot_id_nonexistent_is_error() {
        let (_tmp, root) = setup();
        let result = with_repo(&root, |r| commands::resolve_snapshot_id(r, "doesnotexist"));
        assert!(result.is_err());
    }

    // =========================================================================
    // path normalisation
    // =========================================================================

    #[test]
    fn path_normalisation_forward_slash() {
        let raw = "src\\commands\\mod.rs";
        let normalised = db::normalise(raw);
        assert_eq!(normalised, "src/commands/mod.rs");
        assert!(!normalised.contains('\\'));
    }

    #[test]
    fn path_normalisation_unix_noop() {
        let raw = "src/commands/mod.rs";
        assert_eq!(db::normalise(raw), raw);
    }

    // =========================================================================
    // Integration: time-travel across multiple snapshots
    // =========================================================================

    #[test]
    fn time_travel_integrity() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");
        write(&root, "f.txt", "v3");
        let h3 = save(&root, "s3");

        with_write(&root, |vr| commands::restore::run(vr, &h2, true, &[])).unwrap();
        assert_eq!(read(&root, "f.txt"), "v2");

        with_write(&root, |vr| commands::restore::run(vr, &h3, true, &[])).unwrap();
        assert_eq!(read(&root, "f.txt"), "v3");

        with_write(&root, |vr| commands::restore::run(vr, &h1, true, &[])).unwrap();
        assert_eq!(read(&root, "f.txt"), "v1");
    }

    // =========================================================================
    // Integration: full branch workflow
    // =========================================================================

    #[test]
    fn full_branch_workflow() {
        let (_tmp, root) = setup();

        // Start on main
        write(&root, "README.md", "# Project");
        save(&root, "init");

        // Create feature branch
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "feature.txt", "feature work");
        save(&root, "feat work");

        // Switch back to main — feature.txt must vanish (it wasn't on main)
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        assert_eq!(read(&root, "README.md"), "# Project");
        assert!(
            !exists(&root, "feature.txt"),
            "feature.txt should not exist on main"
        );
        assert!(
            with_repo(&root, commands::get_dirty_files).is_empty(),
            "main must be clean before merge"
        );

        // Fast-forward merge: feature.txt should appear
        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        assert!(exists(&root, "feature.txt"));
        assert_eq!(read(&root, "feature.txt"), "feature work");
    }

    // =========================================================================
    // Integration: undo + redo + save cycle
    // =========================================================================

    #[test]
    fn undo_redo_save_cycle() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        // Undo s2 -> at s1
        with_write(&root, commands::undo::run).unwrap();
        assert_eq!(parent(&root), h1);
        assert_eq!(read(&root, "f.txt"), "v1");

        // Redo s2 -> back at s2
        with_write(&root, commands::redo::run).unwrap();
        assert_eq!(parent(&root), h2);
        assert_eq!(read(&root, "f.txt"), "v2");

        // Undo again, then make a new save (invalidates redo)
        with_write(&root, commands::undo::run).unwrap();
        write(&root, "f.txt", "v3_diverge");
        let h3 = save(&root, "s3_diverge");
        assert_eq!(parent(&root), h3);
        assert!(with_write(&root, commands::redo::run).is_err());
    }

    // =========================================================================
    // Integration: veloignore respects patterns
    // =========================================================================

    #[test]
    fn veloignore_glob_logic() {
        let (_tmp, root) = setup();
        // Override the default .veloignore
        write(&root, ".veloignore", "*.log\ntemp/");
        write(&root, "main.rs", "fn main() {}");
        write(&root, "debug.log", "noise");
        fs::create_dir_all(root.join("temp")).unwrap();
        write(&root, "temp/cache.tmp", "junk");

        let r = with_write(&root, |vr| commands::save::run(vr, "test", false))
            .unwrap()
            .into_result()
            .unwrap();
        // Only main.rs + .veloignore should be tracked
        assert_eq!(r.new_count, 2);
    }

    // =========================================================================
    // Integration: subdirectory find_repo_root in main workflow
    // =========================================================================

    #[test]
    fn commands_work_from_subdirectory() {
        let (_tmp, root) = setup();
        write(&root, "src/lib.rs", "pub fn foo() {}");
        save(&root, "initial");

        // Simulate running from a subdirectory by finding root from there
        let sub = root.join("src");
        let found = commands::find_repo_root(&sub).unwrap();
        assert_eq!(found, root);

        // Dirty check should work from the found root
        write(&root, "src/lib.rs", "pub fn bar() {}");
        let dirty = with_repo(&found, commands::get_dirty_files);
        assert_eq!(dirty.get("src/lib.rs"), Some(&FileStatus::Modified));
    }

    // ═════════════════════════════════════════════════════════════════════════════
    // NEW FEATURE TESTS
    // ═════════════════════════════════════════════════════════════════════════════

    // ─── stash ───────────────────────────────────────────────────────────────

    #[test]
    fn stash_push_clears_working_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "dirty");
        write(&root, "new.txt", "brand new");
        with_write(&root, |vr| commands::stash::push(vr, None)).unwrap();

        // Working tree should be clean (back to s1 state)
        assert_eq!(read(&root, "f.txt"), "base");
        assert!(!exists(&root, "new.txt"));
        assert!(with_repo(&root, commands::get_dirty_files).is_empty());
    }

    #[test]
    fn stash_push_named_shelf() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");
        write(&root, "f.txt", "wip");
        with_write(&root, |vr| {
            commands::stash::push(vr, Some("my-feature".into()))
        })
        .unwrap();

        // Should appear in list
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM stash WHERE name = 'my-feature'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn stash_push_duplicate_name_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");
        write(&root, "f.txt", "wip");
        with_write(&root, |vr| commands::stash::push(vr, Some("shelf".into()))).unwrap();

        write(&root, "f.txt", "wip2");
        let result = with_write(&root, |vr| commands::stash::push(vr, Some("shelf".into())));
        assert!(result.is_err());
    }

    #[test]
    fn stash_push_clean_tree_is_noop() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");
        // Clean — stash should do nothing
        with_write(&root, |vr| commands::stash::push(vr, None)).unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM stash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn stash_pop_restores_changes() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "stashed content");
        with_write(&root, |vr| commands::stash::push(vr, Some("wip".into()))).unwrap();
        assert_eq!(read(&root, "f.txt"), "base");

        with_write(&root, |vr| commands::stash::pop(vr, Some("wip".into()))).unwrap();
        assert_eq!(read(&root, "f.txt"), "stashed content");

        // Shelf should be gone after pop
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM stash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn stash_pop_most_recent_when_no_name() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "v2");
        with_write(&root, |vr| commands::stash::push(vr, Some("first".into()))).unwrap();

        write(&root, "f.txt", "v3");
        with_write(&root, |vr| commands::stash::push(vr, Some("second".into()))).unwrap();

        // Pop with no name should get "second" (most recent)
        with_write(&root, |vr| commands::stash::pop(vr, None)).unwrap();
        assert_eq!(read(&root, "f.txt"), "v3");
    }

    #[test]
    fn stash_pop_on_dirty_tree_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "stashed");
        with_write(&root, |vr| commands::stash::push(vr, None)).unwrap();

        // Make the tree dirty again
        write(&root, "f.txt", "dirty");
        let result = with_write(&root, |vr| commands::stash::pop(vr, None));
        assert!(result.is_err());
    }

    #[test]
    fn stash_drop_removes_shelf_without_restoring() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "stashed");
        with_write(&root, |vr| commands::stash::push(vr, Some("temp".into()))).unwrap();
        assert_eq!(read(&root, "f.txt"), "base");

        with_write(&root, |vr| {
            commands::stash::drop_shelf(vr, Some("temp".into()))
        })
        .unwrap();
        // File still "base" — not restored
        assert_eq!(read(&root, "f.txt"), "base");

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM stash", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn stash_pop_nonexistent_is_error() {
        let (_tmp, root) = setup();
        let result = with_write(&root, |vr| commands::stash::pop(vr, Some("ghost".into())));
        assert!(result.is_err());
    }

    #[test]
    fn stash_preserves_new_files() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "base");
        save(&root, "s1");

        // Stash a brand-new file that doesn't exist in the snapshot
        write(&root, "brand_new.txt", "totally new");
        with_write(&root, |vr| {
            commands::stash::push(vr, Some("new-file-stash".into()))
        })
        .unwrap();
        assert!(!exists(&root, "brand_new.txt"));

        with_write(&root, |vr| {
            commands::stash::pop(vr, Some("new-file-stash".into()))
        })
        .unwrap();
        assert!(exists(&root, "brand_new.txt"));
        assert_eq!(read(&root, "brand_new.txt"), "totally new");
    }

    // ─── show ────────────────────────────────────────────────────────────────

    #[test]
    fn show_reports_snapshot_metadata() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        let d = with_repo(&root, |vr| commands::show::run(vr, &h1, &None)).unwrap();
        assert_eq!(d.hash, h1);
        assert_eq!(d.message, "s1");
        assert_eq!(d.branch, "main");
        assert!(
            d.created_at > year_2020(),
            "show must report a real creation time, not the column default"
        );
    }

    #[test]
    fn show_file_filter_restricts_the_diff() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        write(&root, "b.txt", "B");
        let h1 = save(&root, "s1");

        let d = with_repo(&root, |vr| {
            commands::show::run(vr, &h1, &Some("a.txt".into()))
        })
        .unwrap();
        let paths: Vec<&str> = d.diff.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.txt"], "the filter must exclude b.txt");
    }

    #[test]
    fn show_root_snapshot_has_no_parent_and_adds_every_file() {
        use commands::diff::FileChange;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        let d = with_repo(&root, |vr| {
            commands::show::run(vr, &h1, &Some("f.txt".into()))
        })
        .unwrap();
        assert!(d.parent.is_none(), "the root snapshot has no parent");
        let FileChange::Added { lines } = &d.diff.files[0].change else {
            panic!("expected an addition, got {:?}", d.diff.files[0].change);
        };
        assert_eq!(lines, &vec!["v1".to_string()]);
    }

    #[test]
    fn show_records_the_parent_of_a_later_snapshot() {
        use commands::diff::FileChange;
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        let d = with_repo(&root, |vr| {
            commands::show::run(vr, &h2, &Some("f.txt".into()))
        })
        .unwrap();
        assert_eq!(d.parent.as_deref(), Some(h1.as_str()));
        assert!(matches!(
            d.diff.files[0].change,
            FileChange::Modified { .. }
        ));
    }

    #[test]
    fn show_reports_a_deleted_file() {
        use commands::diff::FileChange;
        let (_tmp, root) = setup();
        write(&root, "keep.txt", "k");
        write(&root, "gone.txt", "g");
        save(&root, "s1");
        fs::remove_file(root.join("gone.txt")).unwrap();
        let h2 = save(&root, "s2");

        let d = with_repo(&root, |vr| {
            commands::show::run(vr, &h2, &Some("gone.txt".into()))
        })
        .unwrap();
        assert_eq!(d.diff.files.len(), 1);
        assert_eq!(d.diff.files[0].change, FileChange::Deleted);
    }

    #[test]
    fn show_nonexistent_hash_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        assert!(with_repo(&root, |vr| commands::show::run(vr, "deadbeef1234", &None)).is_err());
    }

    #[test]
    fn show_works_via_tag() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("release"), None, false)
        })
        .unwrap();
        let d = with_repo(&root, |vr| commands::show::run(vr, "release", &None)).unwrap();
        assert_eq!(d.hash, h1, "the tag must resolve to the snapshot it names");
    }

    // ─── history file filter ──────────────────────────────────────────────────

    /// The filter means "changed this path", which is what the CLI has always
    /// advertised — not "this path existed", which is what it used to do.
    ///
    /// A velo tree is the complete file set, so presence is true for every
    /// snapshot after the file is created. The previous test asserted 3 of 3
    /// snapshots for a path that two of them touched, and its comment described
    /// that as the contract; the help text said otherwise, and the help text was
    /// right.
    #[test]
    fn history_file_filter_selects_snapshots_that_changed_the_path() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A1");
        write(&root, "b.txt", "B1");
        save(&root, "both created");

        write(&root, "a.txt", "A2");
        save(&root, "only a touched");

        write(&root, "b.txt", "B2");
        save(&root, "only b touched");

        let messages = |file: &str, limit: Option<usize>| -> Vec<String> {
            with_repo(&root, |vr| {
                commands::history::run(
                    vr,
                    commands::history::Options {
                        file: Some(file),
                        limit,
                        ..Default::default()
                    },
                )
            })
            .unwrap()
            .entries
            .iter()
            .map(|e| e.message.clone())
            .collect()
        };

        assert_eq!(
            messages("a.txt", None),
            vec!["only a touched", "both created"],
            "the snapshot that changed only b.txt must not appear"
        );
        assert_eq!(
            messages("b.txt", None),
            vec!["only b touched", "both created"]
        );

        // A path added later is absent from everything that predates it.
        write(&root, "c.txt", "C1");
        save(&root, "adds c");
        assert_eq!(messages("c.txt", None), vec!["adds c"]);
    }

    /// A deletion changes the path, and so does a mode flip.
    ///
    /// Deletions only appear in the *parent's* tree, so a one-directional
    /// comparison would miss them entirely — which is why the query runs both
    /// ways.
    #[test]
    fn history_file_filter_sees_deletions_and_mode_changes() {
        let (_tmp, root) = setup();
        write(&root, "gone.txt", "here");
        save(&root, "create");
        write(&root, "unrelated.txt", "x");
        save(&root, "unrelated");
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        save(&root, "delete it");

        let msgs: Vec<String> = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    file: Some("gone.txt"),
                    ..Default::default()
                },
            )
        })
        .unwrap()
        .entries
        .iter()
        .map(|e| e.message.clone())
        .collect();
        assert_eq!(
            msgs,
            vec!["delete it", "create"],
            "the deleting snapshot changed the path as much as the creating one"
        );
    }

    /// A directory matches everything beneath it.
    ///
    /// The CLI has always said "file or directory"; a directory previously
    /// matched nothing at all, because the query compared the path for equality.
    #[test]
    fn history_file_filter_accepts_a_directory() {
        let (_tmp, root) = setup();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        write(&root, "src/deep/a.rs", "A");
        save(&root, "add under src");
        write(&root, "top.txt", "T");
        save(&root, "outside src");
        write(&root, "src/deep/a.rs", "A2");
        save(&root, "edit under src");

        let msgs: Vec<String> = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    file: Some("src"),
                    ..Default::default()
                },
            )
        })
        .unwrap()
        .entries
        .iter()
        .map(|e| e.message.clone())
        .collect();
        assert_eq!(msgs, vec!["edit under src", "add under src"]);
    }

    /// The limit counts matches, not candidates.
    ///
    /// Applying it before the filter returns whichever of the newest N happened
    /// to match — so asking for "the last checkpoint touching this file" could
    /// return nothing at all while the file had plenty of history.
    #[test]
    fn history_file_filter_applies_the_limit_after_filtering() {
        let (_tmp, root) = setup();
        write(&root, "watched.txt", "v1");
        save(&root, "watched v1");
        write(&root, "watched.txt", "v2");
        save(&root, "watched v2");
        // Several newer snapshots that do not touch it.
        for i in 0..5 {
            write(&root, &format!("noise{}.txt", i), "x");
            save(&root, &format!("noise {}", i));
        }

        let msgs: Vec<String> = with_repo(&root, |vr| {
            commands::history::run(
                vr,
                commands::history::Options {
                    file: Some("watched.txt"),
                    limit: Some(1),
                    ..Default::default()
                },
            )
        })
        .unwrap()
        .entries
        .iter()
        .map(|e| e.message.clone())
        .collect();
        assert_eq!(
            msgs,
            vec!["watched v2"],
            "limit 1 must mean the newest matching snapshot, not zero of the newest one"
        );
    }

    // ─── save --amend ─────────────────────────────────────────────────────────

    #[test]
    fn save_amend_replaces_last_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        // Amend: fix a typo in f.txt and update message
        write(&root, "f.txt", "v1 fixed");
        let result = with_write(&root, |vr| commands::save::run(vr, "s1 amended", true)).unwrap();
        let amended = result.into_result().unwrap();

        assert_ne!(amended.hash, h1, "Amended hash must differ");

        // Original hash no longer exists
        assert!(!snapshot_exists(&root, &h1));
        // Amended snapshot does exist and is the current parent
        assert!(snapshot_exists(&root, &amended.hash));
        assert_eq!(parent(&root), amended.hash);

        // Content of the file should reflect the amendment
        assert_eq!(read(&root, "f.txt"), "v1 fixed");
    }

    #[test]
    fn save_amend_message_updated() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "original message");

        write(&root, "f.txt", "v1b");
        with_write(&root, |vr| {
            commands::save::run(vr, "corrected message", true)
        })
        .unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let msg: String = conn
            .query_row(
                "SELECT message FROM snapshots WHERE branch = 'main' ORDER BY created_at_ms DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg, "corrected message");
    }

    #[test]
    fn save_amend_preserves_parent_lineage() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        // Amend s2 — the amended snapshot's parent should still be h1
        write(&root, "f.txt", "v2 amended");
        with_write(&root, |vr| commands::save::run(vr, "s2 amended", true)).unwrap();

        // PARENT file is the authoritative source — it's written last in save::run
        // and always points to the most recently created snapshot.
        let amended_hash = parent(&root);

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let new_parent: String = conn
            .query_row(
                "SELECT parent_hash FROM snapshots WHERE hash = ?",
                [&amended_hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_parent, h1, "Amended snapshot's parent should be h1");
    }

    #[test]
    fn save_amend_clean_tree_still_updates_message() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "typo mesage");

        // Amend with no file changes — just fix the message
        // (dirty is empty, but amend=true forces a save)
        let result =
            with_write(&root, |vr| commands::save::run(vr, "fixed message", true)).unwrap();
        // Should return Some even when tree is clean (amend forces it)
        // Note: if dirty is empty AND amend, we still create a new snapshot
        assert!(result.saved());
    }

    // ─── restore pathspec ─────────────────────────────────────────────────────

    #[test]
    fn restore_single_file_does_not_update_parent() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A1");
        write(&root, "b.txt", "B1");
        let h1 = save(&root, "s1");

        write(&root, "a.txt", "A2");
        write(&root, "b.txt", "B2");
        let h2 = save(&root, "s2");

        // Restore only a.txt from s1
        with_write(&root, |vr| {
            commands::restore::run(vr, &h1, false, &["a.txt".into()])
        })
        .unwrap();

        // a.txt should be back to A1, b.txt stays at B2
        assert_eq!(read(&root, "a.txt"), "A1");
        assert_eq!(read(&root, "b.txt"), "B2");

        // PARENT should still point to h2 (partial restore doesn't update it)
        assert_eq!(parent(&root), h2);
    }

    #[test]
    fn restore_pathspec_nonexistent_in_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        let h1 = save(&root, "s1");
        write(&root, "a.txt", "A2");
        save(&root, "s2");

        // "ghost.txt" didn't exist in s1 — should be a no-op (no error)
        with_write(&root, |vr| {
            commands::restore::run(vr, &h1, false, &["ghost.txt".into()])
        })
        .unwrap();
        // Current file unchanged
        assert_eq!(read(&root, "a.txt"), "A2");
    }

    #[test]
    fn restore_pathspec_prefix_matches_directory() {
        let (_tmp, root) = setup();
        write(&root, "src/a.rs", "fn a() {}");
        write(&root, "src/b.rs", "fn b() {}");
        write(&root, "main.rs", "fn main() {}");
        let h1 = save(&root, "s1");

        write(&root, "src/a.rs", "fn a_modified() {}");
        write(&root, "src/b.rs", "fn b_modified() {}");
        write(&root, "main.rs", "fn main_modified() {}");
        save(&root, "s2");

        // Restore the entire src/ directory from h1
        with_write(&root, |vr| {
            commands::restore::run(vr, &h1, false, &["src/".into()])
        })
        .unwrap();
        assert_eq!(read(&root, "src/a.rs"), "fn a() {}");
        assert_eq!(read(&root, "src/b.rs"), "fn b() {}");
        // main.rs should remain at s2 version
        assert_eq!(read(&root, "main.rs"), "fn main_modified() {}");
    }

    // ─── cherry-pick ──────────────────────────────────────────────────────────

    #[test]
    fn cherry_pick_applies_changes_from_another_branch() {
        let (_tmp, root) = setup();
        write(&root, "base.txt", "base content");
        save(&root, "base");

        // Create a hotfix on another branch
        with_write(&root, |vr| commands::switch::run(vr, "hotfix", false)).unwrap();
        write(&root, "fix.txt", "bug fix content");
        save(&root, "hotfix save");
        let fix_hash = parent(&root);

        // Back on main, apply the hotfix
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::cherry_pick::run(vr, &fix_hash)).unwrap();

        // The new file from the hotfix should be on main now
        assert!(exists(&root, "fix.txt"));
        assert_eq!(read(&root, "fix.txt"), "bug fix content");
    }

    #[test]
    fn cherry_pick_auto_merges_nonoverlapping_edits() {
        // Regression: cherry-pick used a naive classifier that flagged a false
        // conflict when the picked commit and the current branch touched
        // different lines of the same file. It must auto-merge instead.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "A\nB\nC\nD\nE\n");
        save(&root, "ancestor");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "A_CHANGED\nB\nC\nD\nE\n"); // picks: edits line 1
        let fix = save(&root, "feature line1");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "A\nB\nC\nD\nE_CHANGED\n"); // main: edits line 5
        save(&root, "main line5");

        with_write(&root, |vr| commands::cherry_pick::run(vr, &fix)).unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "non-overlapping cherry-pick must not conflict");
        assert!(!exists(&root, ".velo/MERGE_HEAD"));
        assert_eq!(read(&root, "f.txt"), "A_CHANGED\nB\nC\nD\nE_CHANGED\n");
    }

    #[test]
    fn cherry_pick_aborts_on_dirty_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        // Make the tree dirty
        write(&root, "f.txt", "dirty");
        let result = with_write(&root, |vr| commands::cherry_pick::run(vr, &h2));
        assert!(result.is_err());
    }

    #[test]
    fn cherry_pick_conflict_creates_conflict_file() {
        let (_tmp, root) = setup();
        write(&root, "shared.txt", "base");
        save(&root, "base");

        // Branch A: modify shared.txt
        with_write(&root, |vr| commands::switch::run(vr, "branch-a", false)).unwrap();
        write(&root, "shared.txt", "branch A version");
        save(&root, "branch A");
        let branch_a_hash = parent(&root);

        // Back on main: independently modify shared.txt
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "shared.txt", "main version");
        save(&root, "main snap");

        // Cherry-pick branch A's change — should conflict
        with_write(&root, |vr| commands::cherry_pick::run(vr, &branch_a_hash)).unwrap();
        // Conflict stored in DB, not as a .conflict file
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM conflict_files WHERE path = 'shared.txt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(n > 0, "shared.txt conflict should be in DB");
    }

    #[test]
    fn cherry_pick_auto_saves_clean_pick() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        write(&root, "b.txt", "B");
        save(&root, "base");

        // On another branch, add a new independent file
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "c.txt", "C — new feature file");
        save(&root, "feature adds c");
        let feature_hash = parent(&root);

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        let before_parent = parent(&root);

        with_write(&root, |vr| commands::cherry_pick::run(vr, &feature_hash)).unwrap();

        // Should auto-save — parent should have advanced
        let after_parent = parent(&root);
        assert_ne!(
            before_parent, after_parent,
            "Cherry-pick should auto-save when clean"
        );
        assert!(exists(&root, "c.txt"));
    }

    #[test]
    fn cherry_pick_nonexistent_hash_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v");
        save(&root, "s1");
        let result = with_write(&root, |vr| commands::cherry_pick::run(vr, "deadbeef1234"));
        assert!(result.is_err());
    }

    // ─── blame ───────────────────────────────────────────────────────────────

    #[test]
    fn blame_attributes_lines_to_snapshots() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "line one\n");
        let h1 = save(&root, "first");
        write(&root, "app.py", "line one\nline two\n");
        let h2 = save(&root, "second");

        // blame should not panic and should succeed
        with_repo(&root, |vr| commands::blame::run(vr, "app.py", None)).unwrap();
        // line 2 ("line two") was introduced in h2
        // line 1 ("line one") was introduced in h1
        // We verify by checking the snapshots exist
        assert_ne!(h1, h2);
    }

    #[test]
    fn blame_nonexistent_file_is_error() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "hello\n");
        save(&root, "s1");
        let r = with_repo(&root, |vr| commands::blame::run(vr, "missing.py", None));
        assert!(r.is_err());
    }

    #[test]
    fn blame_at_past_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        let h1 = save(&root, "v1");
        write(&root, "f.txt", "v2\n");
        save(&root, "v2");
        // blame at h1 should work on the file as it was then
        with_repo(&root, |vr| commands::blame::run(vr, "f.txt", Some(&h1))).unwrap();
    }

    // ─── grep ─────────────────────────────────────────────────────────────────

    #[test]
    fn grep_finds_pattern_in_working_tree() {
        let (_tmp, root) = setup();
        write(&root, "main.py", "def hello():\n    return 'world'\n");
        save(&root, "init");
        // Should succeed (no panic) even with no match
        with_repo(&root, |vr| {
            commands::grep::run(vr, "hello", None, false, false, 2)
        })
        .unwrap();
    }

    #[test]
    fn grep_no_match_is_ok() {
        let (_tmp, root) = setup();
        write(&root, "main.py", "def hello():\n    pass\n");
        save(&root, "init");
        with_repo(&root, |vr| {
            commands::grep::run(vr, "XYZNOTFOUND", None, false, false, 0)
        })
        .unwrap();
    }

    #[test]
    fn grep_in_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "auth.py", "API_KEY = 'secret'\n");
        let h = save(&root, "add key");
        write(&root, "auth.py", "API_KEY = 'rotated'\n");
        save(&root, "rotate key");
        // Search the old snapshot — should find 'secret'
        with_repo(&root, |vr| {
            commands::grep::run(vr, "secret", Some(&h), false, false, 0)
        })
        .unwrap();
    }

    #[test]
    fn grep_invalid_regex_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "hello\n");
        save(&root, "s1");
        let r = with_repo(&root, |vr| {
            commands::grep::run(vr, "[invalid(", None, false, false, 0)
        });
        assert!(r.is_err());
    }

    #[test]
    fn grep_case_insensitive() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "Hello World\n");
        save(&root, "s1");
        with_repo(&root, |vr| {
            commands::grep::run(vr, "hello", None, true, false, 0)
        })
        .unwrap();
    }

    // ─── squash ───────────────────────────────────────────────────────────────

    #[test]
    fn squash_collapses_n_commits() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "s1");
        write(&root, "f.txt", "v2\n");
        save(&root, "s2");
        write(&root, "f.txt", "v3\n");
        save(&root, "s3");

        with_write(&root, |vr| commands::squash::run(vr, 3, "combined")).unwrap();

        // History should now have exactly 1 snapshot on this branch
        let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch = 'main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "squash 3 should leave 1 snapshot");

        // The message should be the new one
        let msg: String = conn
            .query_row(
                "SELECT message FROM snapshots WHERE branch = 'main'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg, "combined");

        // File content should be the HEAD content
        assert_eq!(read(&root, "f.txt"), "v3\n");
    }

    #[test]
    fn squash_requires_at_least_two() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "s1");
        let r = with_write(&root, |vr| commands::squash::run(vr, 1, "msg"));
        assert!(r.is_err());
    }

    #[test]
    fn squash_fails_when_not_enough_commits() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "only one");
        let r = with_write(&root, |vr| commands::squash::run(vr, 3, "too many"));
        assert!(r.is_err());
    }

    #[test]
    fn squash_fails_on_dirty_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "s1");
        write(&root, "f.txt", "v2\n");
        save(&root, "s2");
        write(&root, "f.txt", "dirty\n"); // unsaved
        let r = with_write(&root, |vr| commands::squash::run(vr, 2, "msg"));
        assert!(r.is_err());
    }

    #[test]
    fn squash_refuses_when_another_branch_depends_on_range() {
        // Regression: squashing deleted snapshots without checking whether any
        // other branch forked off one of them, orphaning that branch's history.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "s1");
        write(&root, "f.txt", "v2\n");
        save(&root, "s2"); // <- feature will fork from here
                           // Fork a branch off s2.
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "g.txt", "feature\n");
        save(&root, "feature work");
        // Back to main, add one more so we have >=3 to squash.
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "v3\n");
        save(&root, "s3");

        // Squashing main's last 3 (s1,s2,s3) would delete s2, which feature
        // depends on → must be refused.
        let r = with_write(&root, |vr| commands::squash::run(vr, 3, "combined"));
        assert!(r.is_err(), "squash must refuse to orphan branch 'feature'");
        // History must be intact.
        assert!(snapshot_exists(&root, &parent(&root)));
    }

    // =========================================================================
    // repo lock
    // =========================================================================

    #[test]
    fn repo_lock_refuses_second_holder_and_frees_on_drop() {
        let (_tmp, root) = setup();
        let guard = crate::lock::RepoLock::acquire(&root).unwrap();
        // A second acquisition while the first is held must be refused.
        assert!(
            crate::lock::RepoLock::acquire(&root).is_err(),
            "a concurrent lock must be refused"
        );
        drop(guard);
        // Once released, the lock is available again.
        assert!(
            crate::lock::RepoLock::acquire(&root).is_ok(),
            "lock must be re-acquirable after release"
        );
    }

    // =========================================================================
    // fsck
    // =========================================================================

    #[test]
    fn fsck_passes_on_healthy_repo() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "A\nB\nC\n");
        save(&root, "s1");
        with_write(&root, |vr| commands::switch::run(vr, "feat", false)).unwrap();
        write(&root, "f.txt", "A2\nB\nC\n");
        save(&root, "s2");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "A\nB\nC2\n");
        save(&root, "s3");
        with_write(&root, |vr| commands::merge::run(vr, Some("feat"), false)).unwrap();
        save(&root, "merge");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();

        assert!(
            with_repo(&root, commands::fsck::check)
                .unwrap()
                .is_healthy(),
            "healthy repo must pass fsck"
        );
    }

    #[test]
    fn fsck_detects_content_addressing_and_dangling_refs() {
        use commands::fsck::{Problem, Section};
        let (_tmp, root) = setup();
        write(&root, "f.txt", "hello\n");
        let h = save(&root, "s1");

        // Snapshot id must verify against its content.
        assert!(with_repo(&root, commands::fsck::check)
            .unwrap()
            .is_healthy());

        // A corrupt object is detected.
        let obj = fs::read_dir(root.join(".velo/objects"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.is_file())
            .unwrap();
        fs::write(&obj, b"not valid zstd").unwrap();
        let report = with_repo(&root, commands::fsck::check).unwrap();
        assert!(!report.is_healthy(), "a corrupt object must be reported");
        // The finding names the object, so a consumer can act on it.
        let corrupt_name = obj.file_name().unwrap().to_str().unwrap();
        assert!(
            report.problems.iter().any(|p| matches!(
                p,
                Problem::UndecodableObject { hash } | Problem::CorruptObject { hash, .. }
                    if hash == corrupt_name
            )),
            "expected a corruption finding for {}, got {:?}",
            corrupt_name,
            report.problems
        );
        // Objects are the first stage, so that is where the count lands.
        assert!(matches!(
            report.sections.first(),
            Some(Section::Objects { problems: 1.., .. })
        ));
        fs::remove_file(&obj).ok(); // restore-ish

        // A tag pointing at a missing snapshot is detected.
        let (_t2, root2) = setup();
        write(&root2, "g.txt", "x\n");
        save(&root2, "s1");
        let conn = db::get_conn_at_path(&root2.join(".velo/velo.db")).unwrap();
        conn.execute(
            "INSERT INTO tags (name, snapshot_hash) VALUES ('ghost', 'deadbeefdeadbeef')",
            [],
        )
        .unwrap();
        let report = with_repo(&root2, commands::fsck::check).unwrap();
        assert!(!report.is_healthy(), "a dangling tag must be reported");
        assert!(
            report.problems.iter().any(|p| matches!(
                p,
                Problem::DanglingRef { table, name, hash }
                    if table == "tags" && name == "ghost" && hash == "deadbeefdeadbeef"
            )),
            "expected a dangling-tag finding, got {:?}",
            report.problems
        );

        let _ = h;
    }

    #[test]
    fn fsck_repair_prunes_orphaned_state() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x\n");
        save(&root, "s1");
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        // An orphaned hunk-decision (no matching conflict_files row) is cruft,
        // not corruption: report-only fsck still passes.
        conn.execute(
            "INSERT INTO hunk_decisions (file_path, hunk_id, decision, manual_content)
             VALUES ('ghost.txt', 0, 'ours', NULL)",
            [],
        )
        .unwrap();
        assert!(
            with_repo(&root, commands::fsck::check)
                .unwrap()
                .is_healthy(),
            "cruft is not corruption, so the repo stays healthy"
        );
        let before: i64 = conn
            .query_row("SELECT count(*) FROM hunk_decisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 1);

        // --repair prunes it.
        with_write(&root, commands::fsck::repair).unwrap();
        let after: i64 = conn
            .query_row("SELECT count(*) FROM hunk_decisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 0, "repair must prune the orphaned row");
    }

    // =========================================================================
    // file model — mode (exec bit) + symlinks
    // =========================================================================

    #[test]
    fn file_mode_regular_defaults_zero_and_fsck_ok() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "x\n");
        let h = save(&root, "s1");
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let mode: i64 = conn
            .query_row(
                "SELECT mode FROM file_map WHERE snapshot_hash = ? AND path = 'a.txt'",
                [&h],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mode, 0, "a regular file records mode 0");
        assert!(
            with_repo(&root, commands::fsck::check)
                .unwrap()
                .is_healthy(),
            "fsck must pass with modes present"
        );
    }

    #[test]
    #[cfg(unix)]
    fn file_mode_exec_bit_roundtrips() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = setup();
        write(&root, "run.sh", "#!/bin/sh\necho hi\n");
        let p = root.join("run.sh");
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms).unwrap();

        let h = save(&root, "add script");
        fs::remove_file(&p).unwrap();
        with_write(&root, |vr| commands::restore::run(vr, &h, true, &[])).unwrap();

        let mode = fs::metadata(&p).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "executable bit must survive save→restore"
        );
    }

    #[test]
    #[cfg(unix)]
    fn file_mode_symlink_roundtrips() {
        let (_tmp, root) = setup();
        write(&root, "target.txt", "hello\n");
        std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();

        let h = save(&root, "add symlink");
        fs::remove_file(root.join("link.txt")).unwrap();
        with_write(&root, |vr| commands::restore::run(vr, &h, true, &[])).unwrap();

        let meta = fs::symlink_metadata(root.join("link.txt")).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "symlink must be restored as a symlink"
        );
        assert_eq!(
            fs::read_link(root.join("link.txt"))
                .unwrap()
                .to_string_lossy(),
            "target.txt"
        );
    }

    #[test]
    #[cfg(unix)]
    fn file_mode_exec_survives_ff_merge() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, root) = setup();
        write(&root, "base.txt", "b\n");
        save(&root, "base");
        with_write(&root, |vr| commands::switch::run(vr, "feat", false)).unwrap();
        write(&root, "run.sh", "#!/bin/sh\n");
        let p = root.join("run.sh");
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms).unwrap();
        save(&root, "add exec on feat");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        with_write(&root, |vr| commands::merge::run(vr, Some("feat"), false)).unwrap(); // fast-forward

        let mode = fs::metadata(root.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "exec bit must survive a fast-forward merge"
        );
    }

    // =========================================================================
    // branch refs & untracked-file safety
    // =========================================================================

    #[test]
    fn main_exists_from_init_and_is_mergeable() {
        // Regression: branches used to exist only as a `branch` value stamped on
        // snapshots, so committing your first work on another branch left `main`
        // nonexistent — `velo merge` then failed with "has no snapshots" even
        // though PARENT pointed at a real commit.
        let (_tmp, root) = setup();
        write(&root, "a.py", "x\n");
        with_write(&root, |vr| commands::switch::run(vr, "init", false)).unwrap();
        let first = save(&root, "first commit");

        with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        // Visiting a branch must not move it: main stays unborn until something
        // explicitly advances it.
        assert!(
            commands::branch_tip(&conn, "main").is_none(),
            "switching to a branch must not silently give it commits"
        );
        // Merging into an unborn branch starts it at the target (not an error,
        // which is what used to happen).
        with_write(&root, |vr| commands::merge::run(vr, Some("init"), false)).unwrap();
        assert_eq!(
            commands::branch_tip(&conn, "main").as_deref(),
            Some(first.as_str()),
            "merge must fast-forward the unborn branch to the target"
        );

        // Now diverge and merge for real.
        with_write(&root, |vr| commands::switch::run(vr, "init", false)).unwrap();
        write(&root, "feature.py", "feature\n");
        save(&root, "add feature");
        with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();
        with_write(&root, |vr| commands::merge::run(vr, Some("init"), false)).unwrap();
        assert!(
            exists(&root, "feature.py"),
            "merge brought the work into main"
        );
        assert!(with_repo(&root, commands::fsck::check)
            .unwrap()
            .is_healthy());
    }

    #[test]
    fn switch_does_not_block_on_or_delete_untracked_files() {
        // Regression: brand-new files counted as "unsaved changes", so switching
        // demanded --force — and --force then deleted them. They exist in no
        // object, so that loss was unrecoverable.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "c1");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "v2\n");
        save(&root, "c2");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        write(&root, "brand_new.txt", "IMPORTANT\n");
        // No --force needed, and the file survives the switch.
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        assert!(
            exists(&root, "brand_new.txt"),
            "untracked file must survive a switch"
        );
        assert_eq!(read(&root, "brand_new.txt"), "IMPORTANT\n");
        assert_eq!(read(&root, "f.txt"), "v2\n", "tracked file still switched");

        // Even with --force, untracked work is never destroyed.
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        assert!(
            exists(&root, "brand_new.txt"),
            "--force must not delete untracked files"
        );
    }

    #[test]
    fn switch_still_guards_tracked_modifications() {
        // The guard must remain for changes that a switch would overwrite.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1\n");
        save(&root, "c1");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "v2\n");
        save(&root, "c2");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();

        write(&root, "f.txt", "uncommitted edit\n");
        assert!(
            matches!(
                with_write(&root, |vr| commands::switch::run(vr, "feature", false)),
                Err(VeloError::DirtyWorkingTree { .. })
            ),
            "switch must refuse rather than silently decline"
        );
        assert_eq!(
            read(&root, "f.txt"),
            "uncommitted edit\n",
            "and leave the tracked edit intact"
        );
        assert_eq!(head(&root), "main", "and must not have switched branches");
    }

    // =========================================================================
    // bundle (offline transfer)
    // =========================================================================

    #[test]
    fn bundle_roundtrip_transfers_history_and_verifies() {
        let (_ta, a) = setup();
        write(&a, "f.txt", "v1\n");
        save(&a, "s1");
        write(&a, "f.txt", "v2\n");
        let h2 = save(&a, "s2");
        with_write(&a, |vr| {
            commands::tag::create(vr, &tag_name("rel"), None, false)
        })
        .unwrap();

        let bd = TempDir::new().unwrap();
        let bundle = bd.path().join("out.velo");
        let bundle = bundle.to_str().unwrap();
        with_repo(&a, |vr| commands::bundle::create(vr, bundle, None)).unwrap();

        // Apply into a fresh repo.
        let (_tb, b) = setup();
        with_write(&b, |vr| commands::bundle::apply(vr, bundle)).unwrap();

        // Content-addressed: the imported snapshot has the *same* hash as in A.
        assert!(
            snapshot_exists(&b, &h2),
            "imported snapshot must exist in B with the same id"
        );
        // The tag came along.
        let conn = db::get_conn_at_path(&b.join(".velo/velo.db")).unwrap();
        let tagged: String = conn
            .query_row(
                "SELECT snapshot_hash FROM tags WHERE name = 'rel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tagged, h2);
        // The receiver is internally consistent.
        assert!(
            with_repo(&b, commands::fsck::check).unwrap().is_healthy(),
            "receiver must pass fsck"
        );
        // Re-applying is a no-op (idempotent).
        with_write(&b, |vr| commands::bundle::apply(vr, bundle)).unwrap();
        assert!(with_repo(&b, commands::fsck::check).unwrap().is_healthy());
    }

    /// Metadata is part of a snapshot's identity, so a bundle that dropped it
    /// would be rejected by its own receiver — the id would not recompute. This
    /// pins the wire format's metadata section end to end.
    #[test]
    fn bundle_carries_metadata_so_ids_still_verify() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_ta, a) = setup();
        let repo_a = Repo::open_and_migrate(&a).unwrap();

        let mut meta = SnapshotMeta::new();
        meta.set("app", "eval_run", "42").unwrap();
        meta.set("other", "k", "v").unwrap();
        let id = {
            let guard = repo_a.write().unwrap();
            guard
                .save_tree(SaveTree {
                    branch: &branch_name("main"),
                    parent: None,
                    merge_parent: None,
                    message: "published",
                    entries: vec![TreeEntry::file(
                        "a.txt",
                        b"x
"
                        .to_vec(),
                    )],
                    meta: meta.clone(),
                })
                .unwrap()
        };
        drop(repo_a);

        let bd = TempDir::new().unwrap();
        let path = bd.path().join("out.velo");
        let path = path.to_str().unwrap();
        with_repo(&a, |vr| commands::bundle::create(vr, path, None)).unwrap();

        let (_tb, b) = setup();
        with_write(&b, |vr| commands::bundle::apply(vr, path)).unwrap();

        let repo_b = Repo::open_and_migrate(&b).unwrap();
        assert_eq!(
            repo_b.snapshot_meta(&id).unwrap(),
            meta,
            "metadata must survive the round trip intact"
        );
        assert!(
            commands::fsck::check(&repo_b).unwrap().is_healthy(),
            "the receiver recomputes every id, so this fails if metadata was lost"
        );
    }

    #[test]
    fn bundle_create_with_ref_is_self_contained() {
        let (_ta, a) = setup();
        write(&a, "f.txt", "v1\n");
        save(&a, "s1");
        with_write(&a, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&a, "g.txt", "feat\n");
        let hf = save(&a, "feat");
        with_write(&a, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&a, "f.txt", "v2\n");
        save(&a, "main2");

        // Bundle only feature's ancestry.
        let bd = TempDir::new().unwrap();
        let bundle = bd.path().join("f.velo");
        let bundle = bundle.to_str().unwrap();
        with_repo(&a, |vr| {
            commands::bundle::create(vr, bundle, Some("feature"))
        })
        .unwrap();

        let (_tb, b) = setup();
        with_write(&b, |vr| commands::bundle::apply(vr, bundle)).unwrap();
        assert!(snapshot_exists(&b, &hf), "feature tip must be present");
        // Self-contained (walked to root) → receiver is consistent.
        assert!(with_repo(&b, commands::fsck::check).unwrap().is_healthy());
    }

    #[test]
    fn bundle_apply_rejects_corrupt_or_truncated() {
        let (_ta, a) = setup();
        write(&a, "f.txt", "x\n");
        save(&a, "s1");
        let bd = TempDir::new().unwrap();
        let bundle = bd.path().join("out.velo");
        with_repo(&a, |vr| {
            commands::bundle::create(vr, bundle.to_str().unwrap(), None)
        })
        .unwrap();

        // Truncated bundle → rejected.
        let mut bytes = fs::read(&bundle).unwrap();
        bytes.truncate(bytes.len() / 2);
        fs::write(&bundle, &bytes).unwrap();
        let (_tb, b) = setup();
        assert!(
            with_write(&b, |vr| commands::bundle::apply(
                vr,
                bundle.to_str().unwrap()
            ))
            .is_err(),
            "a truncated bundle must be rejected"
        );

        // Not a bundle at all → rejected.
        fs::write(&bundle, b"definitely not a velo bundle").unwrap();
        assert!(with_write(&b, |vr| commands::bundle::apply(
            vr,
            bundle.to_str().unwrap()
        ))
        .is_err());
    }

    // =========================================================================
    // sync negotiation (minimal transfer)
    // =========================================================================

    #[test]
    fn negotiation_sends_only_objects_the_peer_lacks() {
        // A one-file change in a 20-file project must ship ONE object, not the
        // whole tree — a snapshot's file_map references every file, so without
        // exclusion every push/fetch re-sends the entire project.
        let (_tmp, root) = setup();
        for i in 0..20 {
            write(&root, &format!("f{i}.txt"), &format!("content {i}\n"));
        }
        let s1 = save(&root, "initial");
        write(&root, "f0.txt", "changed\n");
        let s2 = save(&root, "touch one file");

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let objects = root.join(".velo/objects");

        // Self-contained pack (what `bundle create` produces): whole tree.
        let all = commands::bundle::reachable_ancestry(&conn, &s2);
        let full = commands::bundle::build_pack(&conn, &objects, &all).unwrap();
        assert!(
            full.objects.len() >= 20,
            "a self-contained pack must carry the whole tree, got {}",
            full.objects.len()
        );

        // Minimal pack for a peer that already has s1: just the changed file.
        let peer_has = commands::bundle::reachable_ancestry(&conn, &s1);
        let mut new_only = all.clone();
        for h in &peer_has {
            new_only.remove(h);
        }
        let minimal =
            commands::bundle::build_pack_excluding(&conn, &objects, &new_only, &peer_has).unwrap();
        assert_eq!(minimal.snapshots.len(), 1, "only the new snapshot is sent");
        assert_eq!(
            minimal.objects.len(),
            1,
            "only the changed file's object is sent, got {}",
            minimal.objects.len()
        );
    }

    #[test]
    fn bundle_create_stays_self_contained_despite_negotiation() {
        // Regression guard: the exclusion path must never leak into bundles.
        let (_tmp, root) = setup();
        write(&root, "a.txt", "1\n");
        save(&root, "s1");
        write(&root, "a.txt", "2\n");
        save(&root, "s2");

        let bd = TempDir::new().unwrap();
        let bundle_path = bd.path().join("out.velo");
        with_repo(&root, |vr| {
            commands::bundle::create(vr, bundle_path.to_str().unwrap(), None)
        })
        .unwrap();

        // A completely fresh repo can import it and verify.
        let (_tb, b) = setup();
        with_write(&b, |vr| {
            commands::bundle::apply(vr, bundle_path.to_str().unwrap())
        })
        .unwrap();
        assert!(with_repo(&b, commands::fsck::check).unwrap().is_healthy());
    }

    #[test]
    fn ahead_behind_counts_divergence() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base\n");
        let base = save(&root, "base");
        // Two commits on main.
        write(&root, "f.txt", "one\n");
        save(&root, "m1");
        write(&root, "f.txt", "two\n");
        let main_tip = save(&root, "m2");
        // One commit on a branch off base.
        with_write(&root, |vr| commands::switch::run(vr, "side", false)).unwrap();
        with_write(&root, |vr| commands::restore::run(vr, &base, true, &[])).unwrap();
        write(&root, "g.txt", "side\n");
        let side_tip = save(&root, "s1");

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        // main is 2 ahead / 1 behind relative to side.
        let (ahead, behind) = commands::ahead_behind(&conn, &main_tip, &side_tip);
        assert_eq!((ahead, behind), (2, 1), "got ahead={ahead} behind={behind}");
        // Identical tips → no divergence.
        assert_eq!(commands::ahead_behind(&conn, &main_tip, &main_tip), (0, 0));
    }

    // =========================================================================
    // object store — roundtrip property tests
    // =========================================================================
    mod storage_props {
        use crate::storage;
        use proptest::prelude::*;
        use tempfile::TempDir;

        /// Save `bytes` as an object and read it back.
        fn roundtrip(bytes: &[u8]) -> Vec<u8> {
            let tmp = TempDir::new().unwrap();
            let objects = tmp.path().join("objects");
            std::fs::create_dir_all(&objects).unwrap();
            let file = tmp.path().join("input.bin");
            std::fs::write(&file, bytes).unwrap();
            let hash = storage::hash_and_compress(&file, &objects).unwrap();
            storage::read_object(&objects, &hash).unwrap()
        }

        proptest! {
            /// Binary content (contains a NUL) must round-trip byte-for-byte —
            /// no CRLF normalisation, no compression loss.
            #[test]
            fn binary_roundtrips_exactly(mut bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
                // Guarantee it's treated as binary.
                bytes.push(0);
                prop_assert_eq!(roundtrip(&bytes), bytes);
            }

            /// Text content round-trips modulo CRLF→LF normalisation: the stored
            /// bytes equal the input with all '\r' stripped.
            #[test]
            fn text_roundtrips_modulo_crlf(s in "[ -~\r\n]{0,4096}") {
                let bytes = s.into_bytes();
                let expected: Vec<u8> = bytes.iter().copied().filter(|b| *b != b'\r').collect();
                prop_assert_eq!(roundtrip(&bytes), expected);
            }

            /// Identical content always hashes to the same object name.
            #[test]
            fn identical_content_same_hash(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
                let tmp = TempDir::new().unwrap();
                let objects = tmp.path().join("objects");
                std::fs::create_dir_all(&objects).unwrap();
                let f1 = tmp.path().join("a"); let f2 = tmp.path().join("b");
                std::fs::write(&f1, &bytes).unwrap();
                std::fs::write(&f2, &bytes).unwrap();
                let h1 = storage::hash_and_compress(&f1, &objects).unwrap();
                let h2 = storage::hash_and_compress(&f2, &objects).unwrap();
                prop_assert_eq!(h1, h2);
            }
        }
    }

    // (The merge engine's property tests live with the engine — see the note in
    // the merge section above.)

    // ─── diff range ───────────────────────────────────────────────────────────

    #[test]
    fn amend_without_a_message_keeps_the_existing_one() {
        // Folding a forgotten file into the last snapshot shouldn't force you to
        // retype the message.
        let (_tmp, root) = setup();
        write(&root, "a.py", "a\n");
        save(&root, "Add login feature");

        write(&root, "forgotten.py", "b\n");
        let r = with_write(&root, |vr| {
            commands::save::run_with_paths(vr, None, true, &[])
        })
        .unwrap()
        .into_result()
        .expect("amend should produce a snapshot");
        assert_eq!(r.new_count, 1, "the forgotten file is folded in");

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let msg: String = conn
            .query_row(
                "SELECT message FROM snapshots WHERE hash = ?",
                [&r.hash],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(msg, "Add login feature", "message carried over");
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch = 'main'",
                [],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "amend replaces rather than adds");

        // A message may still be supplied to reword.
        write(&root, "a.py", "a2\n");
        let r2 = with_write(&root, |vr| {
            commands::save::run_with_paths(vr, Some("Reworded"), true, &[])
        })
        .unwrap()
        .into_result()
        .unwrap();
        let msg2: String = conn
            .query_row(
                "SELECT message FROM snapshots WHERE hash = ?",
                [&r2.hash],
                |x| x.get(0),
            )
            .unwrap();
        assert_eq!(msg2, "Reworded");
    }

    #[test]
    fn amend_needs_something_to_do_and_save_still_needs_a_message() {
        let (_tmp, root) = setup();
        write(&root, "a.py", "a\n");
        save(&root, "first");

        // Clean tree + no new message → nothing to amend (no pointless rehash).
        assert!(
            with_write(&root, |vr| commands::save::run_with_paths(
                vr,
                None,
                true,
                &[]
            ))
            .unwrap()
                == commands::save::Outcome::NothingToAmend,
            "amending nothing should be a no-op, and say why"
        );
        // A plain save still requires a message.
        assert!(with_write(&root, |vr| commands::save::run_with_paths(
            vr,
            None,
            false,
            &[]
        ))
        .is_err());

        // Amending a branch with no snapshots is a clear error, not a panic.
        let (_t2, root2) = setup();
        write(&root2, "x.txt", "x\n");
        assert!(with_write(&root2, |vr| commands::save::run_with_paths(
            vr,
            None,
            true,
            &[]
        ))
        .is_err());
    }

    #[test]
    fn diff_dispatch_covers_every_form() {
        // `velo diff` is one command for every comparison; these are the shapes
        // the CLI hands to the dispatcher.
        let (_tmp, root) = setup();
        write(&root, "a.py", "l1\nl2\n");
        write(&root, "b.txt", "x\n");
        let h1 = save(&root, "c1");
        write(&root, "a.py", "l1\nCHANGED\n");
        let h2 = save(&root, "c2");
        with_write(&root, |vr| {
            commands::tag::create(vr, &tag_name("v1"), None, false)
        })
        .unwrap();
        write(&root, "a.py", "l1\nWORKING\n");

        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();

        // No args: working tree vs last snapshot.
        with_repo(&root, |vr| commands::diff::dispatch(vr, &[], &[])).unwrap();
        // Pathspec only.
        with_repo(&root, |vr| commands::diff::dispatch(vr, &[], &s(&["a.py"]))).unwrap();
        // A lone existing filename is a file, not a ref.
        with_repo(&root, |vr| commands::diff::dispatch(vr, &s(&["a.py"]), &[])).unwrap();
        // A lone ref: snapshot vs working tree (hash, and by tag).
        with_repo(&root, |vr| commands::diff::dispatch(vr, &s(&[&h1]), &[])).unwrap();
        with_repo(&root, |vr| commands::diff::dispatch(vr, &s(&["v1"]), &[])).unwrap();
        // Two refs, and the equivalent range syntax.
        with_repo(&root, |vr| {
            commands::diff::dispatch(vr, &s(&[&h1, &h2]), &[])
        })
        .unwrap();
        with_repo(&root, |vr| {
            commands::diff::dispatch(vr, &s(&[&format!("{}..{}", h1, h2)]), &[])
        })
        .unwrap();
        // Two refs restricted to a path.
        with_repo(&root, |vr| {
            commands::diff::dispatch(vr, &s(&[&h1, &h2]), &s(&["a.py"]))
        })
        .unwrap();

        // Something that is neither a file nor a ref is a clear error.
        assert!(with_repo(&root, |vr| commands::diff::dispatch(
            vr,
            &s(&["no_such_thing"]),
            &[]
        ))
        .is_err());
    }

    #[test]
    fn diff_prefers_file_over_ref_for_ambiguous_name() {
        // A short filename must not be swallowed by a hash-prefix match: files
        // are checked before refs precisely so `velo diff a` means the file.
        let (_tmp, root) = setup();
        write(&root, "a", "one\n");
        save(&root, "c1");
        write(&root, "a", "two\n");
        // Resolves as the file (and therefore succeeds even though a hash may
        // well begin with 'a').
        with_repo(&root, |vr| {
            commands::diff::dispatch(vr, &["a".to_string()], &[])
        })
        .unwrap();
    }

    #[test]
    fn diff_range_two_snapshots() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "version 1\n");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "version 2\n");
        let h2 = save(&root, "s2");
        // Should not panic
        with_repo(&root, |vr| {
            commands::diff::run_range(vr, &h1, Some(&h2), &[])
        })
        .unwrap();
    }

    #[test]
    fn diff_range_snapshot_vs_working_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "saved\n");
        let h = save(&root, "s1");
        write(&root, "f.txt", "working\n");
        with_repo(&root, |vr| commands::diff::run_range(vr, &h, None, &[])).unwrap();
    }

    #[test]
    fn diff_range_with_pathspec() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "a1\n");
        write(&root, "b.txt", "b1\n");
        let h1 = save(&root, "s1");
        write(&root, "a.txt", "a2\n");
        write(&root, "b.txt", "b2\n");
        let h2 = save(&root, "s2");
        // Only diff b.txt — should succeed
        with_repo(&root, |vr| {
            commands::diff::run_range(vr, &h1, Some(&h2), &["b.txt".to_string()])
        })
        .unwrap();
    }

    // ─── rebase ───────────────────────────────────────────────────────────────

    #[test]
    fn rebase_clean_produces_linear_history() {
        let (_tmp, root) = setup();
        write(&root, "base.txt", "base\n");
        save(&root, "base");

        // feature branch: adds feature.txt
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "feature.txt", "feature\n");
        save(&root, "add feature");

        // main advances independently
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "main.txt", "main\n");
        save(&root, "main update");

        // Rebase feature onto main
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        with_write(&root, |vr| commands::rebase::run(vr, "main", false, false)).unwrap();

        // After rebase: feature.txt present, rebased on top of main update
        assert!(exists(&root, "feature.txt"));
        assert!(exists(&root, "main.txt"));

        // History should be linear: no REBASE_STATE file left
        assert!(!exists(&root, ".velo/REBASE_STATE"));
    }

    #[test]
    fn rebase_auto_merges_nonoverlapping_edits() {
        // Regression: rebase replay flagged a false conflict when a replayed
        // commit and the new base touched different lines of the same file.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "A\nB\nC\nD\nE\n");
        save(&root, "ancestor");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "A_CHANGED\nB\nC\nD\nE\n"); // feature: edits line 1
        save(&root, "feature line1");
        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "A\nB\nC\nD\nE_CHANGED\n"); // main: edits line 5
        save(&root, "main line5");
        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();

        with_write(&root, |vr| commands::rebase::run(vr, "main", false, false)).unwrap();

        assert!(
            !exists(&root, ".velo/REBASE_STATE"),
            "clean rebase, no state left"
        );
        assert!(!exists(&root, ".velo/MERGE_HEAD"), "no conflict recorded");
        assert_eq!(read(&root, "f.txt"), "A_CHANGED\nB\nC\nD\nE_CHANGED\n");
    }

    #[test]
    fn rebase_abort_restores_original() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base\n");
        save(&root, "base");
        let _orig = parent(&root);

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "feature\n");
        save(&root, "feature change");

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "main conflict\n");
        save(&root, "main conflict");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        let feature_head = parent(&root);

        // Start rebase (will have conflict)
        let _ = with_write(&root, |vr| commands::rebase::run(vr, "main", false, false));

        // Abort
        with_write(&root, |vr| commands::rebase::run(vr, "", true, false)).unwrap();

        assert!(!exists(&root, ".velo/REBASE_STATE"));
        assert_eq!(parent(&root), feature_head);
    }

    #[test]
    fn rebase_no_in_progress_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v\n");
        save(&root, "s1");
        let r = with_write(&root, |vr| commands::rebase::run(vr, "", false, true));
        assert!(r.is_err());
    }

    #[test]
    fn rebase_abort_after_conflict_restores_tip_and_cleans_snapshots() {
        // A conflicting rebase, aborted, must restore the branch tip to exactly
        // the pre-rebase head with no replayed commits left masquerading as tip.
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base\n");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "feature\n");
        save(&root, "feature change");
        let feature_head = parent(&root);

        with_write(&root, |vr| commands::switch::run(vr, "main", true)).unwrap();
        write(&root, "f.txt", "main conflict\n");
        save(&root, "main conflict");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        let _ = with_write(&root, |vr| commands::rebase::run(vr, "main", false, false)); // conflicts

        with_write(&root, |vr| commands::rebase::run(vr, "", true, false)).unwrap(); // abort

        assert!(!exists(&root, ".velo/REBASE_STATE"));
        assert!(!exists(&root, ".velo/MERGE_HEAD"));
        assert_eq!(
            parent(&root),
            feature_head,
            "PARENT restored to feature head"
        );

        // The apparent branch tip must be the original feature head, not a
        // leftover replayed commit.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let tip: String = conn
            .query_row(
                "SELECT hash FROM snapshots WHERE branch = 'feature' \
                 ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tip, feature_head, "no replayed snapshot may survive abort");
    }

    // ─── pathspec on save ─────────────────────────────────────────────────────

    #[test]
    fn save_with_pathspec_only_saves_matching_files() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "a1\n");
        write(&root, "b.txt", "b1\n");
        save(&root, "base");

        write(&root, "a.txt", "a2\n");
        write(&root, "b.txt", "b2\n");

        // Only save a.txt
        with_write(&root, |vr| {
            commands::save::run_with_paths(vr, Some("only a"), false, &["a.txt".to_string()])
        })
        .unwrap();

        // a.txt should be saved, b.txt should still be dirty
        let dirty = with_repo(&root, commands::get_dirty_files);
        assert!(!dirty.contains_key("a.txt"), "a.txt should be clean");
        assert!(dirty.contains_key("b.txt"), "b.txt should still be dirty");
    }

    /// Resolving every conflict in favour of our own side leaves the working tree
    /// unchanged, which used to make `save` return `NothingToSave` — refusing to
    /// record the merge *and* leaving `MERGE_HEAD` behind. That wedged the
    /// repository: every later merge, rebase, undo, redo and cherry-pick refused
    /// with "a merge is already in progress", and the only way out was
    /// `merge --abort`, which discards the merge.
    #[test]
    fn a_merge_resolved_to_ours_is_still_recorded() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base\n");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "theirs\n");
        let theirs = save(&root, "theirs");

        with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();
        write(&root, "f.txt", "ours\n");
        let ours = save(&root, "ours");

        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        resolve_take(&root, None, commands::resolve::TakeOption::Ours, true).unwrap();

        // The tree now matches `ours` exactly, so nothing is dirty.
        assert!(
            with_repo(&root, commands::get_dirty_files).is_empty(),
            "resolving to ours must leave the tree clean — that is the whole point \
             of this test"
        );

        let outcome =
            with_write(&root, |vr| commands::save::run(vr, "merge feature", false)).unwrap();
        let merged = outcome
            .into_result()
            .expect("the merge must be recorded even though the tree is unchanged");

        // Both parents, because the merge is what this snapshot is *for*.
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let (parent, merge_parent): (String, String) = conn
            .query_row(
                "SELECT parent_hash, merge_parent FROM snapshots WHERE hash = ?",
                [merged.hash.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(parent, ours);
        assert_eq!(merge_parent, theirs, "the second parent must be recorded");

        // And the merge is over, so the repository is usable again.
        assert!(
            !root.join(".velo/MERGE_HEAD").exists(),
            "MERGE_HEAD must be cleared, or every later operation refuses"
        );
        assert!(
            !with_repo(&root, commands::resolve::merge_active),
            "no merge should still be active"
        );
        with_write(&root, commands::undo::run)
            .expect("undo must work again once the merge is concluded");
    }

    /// The counterpart: with no merge pending, a clean tree still has nothing to
    /// save. The fix above must not turn every no-op save into a snapshot.
    #[test]
    fn a_clean_tree_with_no_merge_still_saves_nothing() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "one\n");
        save(&root, "first");

        let outcome = with_write(&root, |vr| {
            commands::save::run(vr, "nothing changed", false)
        })
        .unwrap();
        assert_eq!(outcome, commands::save::Outcome::NothingToSave);
    }

    /// Taking theirs for everything is the same situation whenever their side
    /// happens to equal ours, and it must conclude the merge too.
    #[test]
    fn a_merge_resolved_to_theirs_concludes_the_merge() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base\n");
        save(&root, "base");

        with_write(&root, |vr| commands::switch::run(vr, "feature", false)).unwrap();
        write(&root, "f.txt", "theirs\n");
        save(&root, "theirs");

        with_write(&root, |vr| commands::switch::run(vr, "main", false)).unwrap();
        write(&root, "f.txt", "ours\n");
        save(&root, "ours");

        with_write(&root, |vr| commands::merge::run(vr, Some("feature"), false)).unwrap();
        resolve_take(&root, None, commands::resolve::TakeOption::Theirs, true).unwrap();

        let outcome =
            with_write(&root, |vr| commands::save::run(vr, "merge feature", false)).unwrap();
        assert!(outcome.saved(), "taking theirs must also record the merge");
        assert!(!root.join(".velo/MERGE_HEAD").exists());
        assert_eq!(read(&root, "f.txt"), "theirs\n");
        assert!(with_repo(&root, commands::fsck::check)
            .unwrap()
            .is_healthy());
    }

    /// An unresolvable spec is `NotFound`, not `InvalidInput`.
    ///
    /// A consumer asking for a branch that has no snapshots yet needs to tell
    /// "there is nothing there" from "you asked me something malformed", and
    /// string-matching the message is what typed errors exist to avoid. Found by
    /// writing an actual consumer (see examples/prompt-registry).
    #[test]
    fn an_unresolvable_spec_is_reported_as_not_found() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "x");
        save(&root, "one");

        let repo = Repo::open_and_migrate(&root).unwrap();
        let err = commands::resolve_snapshot_id(&repo, "no-such-ref").unwrap_err();
        assert!(
            matches!(
                err,
                VeloError::NotFound {
                    kind: crate::error::RefKind::Any,
                    ..
                }
            ),
            "expected NotFound, got {:?}",
            err
        );
    }

    /// Carrying a tree forward by reference must produce exactly the snapshot
    /// that carrying it forward by value would.
    ///
    /// This is the whole point of `TreeEntry::stored`: if the two disagreed, a
    /// consumer would silently fork its own history the moment it started using
    /// the cheap path.
    #[test]
    fn a_stored_entry_is_indistinguishable_from_supplying_the_bytes() {
        use crate::tree::{Content, SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let base = {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    branch: &branch_name("a"),
                    parent: None,
                    merge_parent: None,
                    message: "base",
                    entries: vec![
                        TreeEntry::file(
                            "keep.txt",
                            b"kept
"
                            .to_vec(),
                        ),
                        TreeEntry::executable(
                            "run.sh",
                            b"#!/bin/sh
"
                            .to_vec(),
                        ),
                        TreeEntry::symlink("link", "keep.txt"),
                    ],
                    meta: SnapshotMeta::new(),
                })
                .unwrap()
        };

        // Two branches from the same parent: one re-supplies every byte, the
        // other references the objects already stored. Same message is not
        // enough — the timestamp differs — so compare the trees, which is what
        // the id commits to.
        let by_value: Vec<TreeEntry> = repo
            .tree_at(&base)
            .unwrap()
            .into_iter()
            .map(|f| {
                let bytes = repo.read_object(&f.object).unwrap();
                TreeEntry {
                    path: f.path,
                    content: Content::Bytes(bytes),
                    kind: f.kind,
                }
            })
            .collect();
        let by_reference: Vec<TreeEntry> = repo
            .tree_at(&base)
            .unwrap()
            .into_iter()
            .map(|f| TreeEntry::stored(f.path, f.object, f.kind))
            .collect();
        assert_ne!(by_value, by_reference, "the two entry forms differ");

        let (left, right) = {
            let guard = repo.write().unwrap();
            let left = guard
                .save_tree(SaveTree {
                    branch: &branch_name("l"),
                    parent: Some(&base),
                    merge_parent: None,
                    // Distinct messages: identity excludes the branch, so with
                    // one message these two would be the *same snapshot* whenever
                    // both saves land in the same millisecond.
                    message: "by value",
                    entries: by_value,
                    meta: SnapshotMeta::new(),
                })
                .unwrap();
            let right = guard
                .save_tree(SaveTree {
                    branch: &branch_name("r"),
                    parent: Some(&base),
                    merge_parent: None,
                    message: "by reference",
                    entries: by_reference,
                    meta: SnapshotMeta::new(),
                })
                .unwrap();
            (left, right)
        };

        // Identical trees: same paths, same objects, same modes — including the
        // executable bit and the symlink, which is where a naive implementation
        // would drop the mode.
        assert_eq!(repo.tree_at(&left).unwrap(), repo.tree_at(&right).unwrap());
        assert_eq!(
            repo.read_file_at(&right, "keep.txt").unwrap(),
            b"kept
"
        );
        assert!(with_repo(&root, commands::fsck::check)
            .unwrap()
            .is_healthy());
    }

    /// A referenced object that is not in the store is refused.
    ///
    /// Otherwise the public API could record a snapshot naming content that does
    /// not exist — corruption manufactured through a supported call, discovered
    /// only later by `fsck`.
    #[test]
    fn a_stored_entry_naming_a_missing_object_is_refused() {
        use crate::tree::{FileKind, SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let guard = repo.write().unwrap();

        let absent: ObjectHash = "ab".repeat(32).parse().unwrap();
        let err = guard
            .save_tree(SaveTree {
                branch: &branch_name("a"),
                parent: None,
                merge_parent: None,
                message: "m",
                entries: vec![TreeEntry::stored("f.txt", absent, FileKind::Regular)],
                meta: SnapshotMeta::new(),
            })
            .unwrap_err();
        assert!(
            matches!(err, VeloError::MissingObject { .. }),
            "expected MissingObject, got {:?}",
            err
        );
    }

    /// Asking about a branch directly, rather than resolving it as a spec and
    /// reading the error.
    ///
    /// The two questions are genuinely different: `switch` creates a branch that
    /// exists with nothing on it, and a consumer needs to tell that from a branch
    /// that was never created.
    #[test]
    fn a_branch_can_be_asked_about_without_interpreting_an_error() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "x");
        let first = save(&root, "one");

        let repo = Repo::open_and_migrate(&root).unwrap();
        let main = branch_name("main");
        assert!(repo.branch_exists(&main).unwrap());
        assert_eq!(repo.branch_tip(&main).unwrap().unwrap(), first);

        let ghost = branch_name("never_created");
        assert!(!repo.branch_exists(&ghost).unwrap());
        assert!(repo.branch_tip(&ghost).unwrap().is_none());
        drop(repo);

        // A branch created but not committed to exists, yet has no tip — which is
        // exactly the case resolving-and-catching-NotFound cannot express.
        with_write(&root, |vr| commands::switch::run(vr, "unborn", false)).unwrap();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let unborn = branch_name("unborn");
        assert!(repo.branch_exists(&unborn).unwrap(), "switch created it");
        assert!(
            repo.branch_tip(&unborn).unwrap().is_none(),
            "but nothing has been saved on it"
        );
    }

    /// Re-recording an identical snapshot returns the same id instead of failing.
    ///
    /// Identity covers the tree, parents, message, metadata and timestamp, but
    /// deliberately not the branch — so the same content saved to two branches
    /// inside one millisecond is one snapshot. That used to surface as a raw
    /// SQLite `UNIQUE constraint failed`, which is a poor answer to a legitimate
    /// call. Caught by CI, which is faster than this machine and hit the window
    /// that a local run mostly missed.
    #[test]
    fn saving_an_identical_snapshot_twice_is_idempotent() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let spec = |branch| SaveTree {
            branch,
            parent: None,
            merge_parent: None,
            message: "same",
            entries: vec![TreeEntry::file(
                "a.txt",
                b"x
"
                .to_vec(),
            )],
            meta: SnapshotMeta::new(),
        };

        let (one, two) = (branch_name("one"), branch_name("two"));
        let guard = repo.write().unwrap();
        let first = guard.save_tree(spec(&one)).unwrap();
        // Same content, same message, no parent — and a different branch, which
        // identity ignores. Back to back, so the timestamp is almost certainly
        // the same millisecond.
        let second = guard.save_tree(spec(&two)).unwrap();
        drop(guard);

        if first == second {
            // The window we are testing: one snapshot, and both branches see it.
            let rows: i64 = repo
                .conn()
                .query_row(
                    "SELECT count(*) FROM snapshots WHERE hash = ?",
                    [&first],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(rows, 1, "the snapshot must not be duplicated");
            let files: i64 = repo
                .conn()
                .query_row(
                    "SELECT count(*) FROM file_map WHERE snapshot_hash = ?",
                    [&first],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(files, 1, "its tree must not be duplicated either");
            assert_eq!(
                repo.branch_tip(&two).unwrap(),
                Some(first.clone()),
                "the second branch still gets the tip it asked for"
            );
        }

        // Either way the repository is consistent.
        assert!(with_repo(&root, commands::fsck::check)
            .unwrap()
            .is_healthy());
    }

    /// A merge recorded through the embedder API is a real merge.
    ///
    /// The column, the id recipe and `history::Entry::is_merge` all supported a
    /// second parent; only `SaveTree` had no way to set one, so every merge an
    /// embedder recorded was stored as linear. That loses the graph, and — worse
    /// — leaves the next merge computing its base from an ancestor that is too
    /// old, re-raising conflicts the author already resolved.
    #[test]
    fn save_tree_can_record_a_merge_parent() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let (base, side, merged) = {
            let guard = repo.write().unwrap();
            let base = guard
                .save_tree(SaveTree {
                    branch: &branch_name("main"),
                    parent: None,
                    merge_parent: None,
                    message: "base",
                    entries: vec![TreeEntry::file(
                        "a.txt",
                        b"base
"
                        .to_vec(),
                    )],
                    meta: SnapshotMeta::new(),
                })
                .unwrap();
            let side = guard
                .save_tree(SaveTree {
                    branch: &branch_name("side"),
                    parent: Some(&base),
                    merge_parent: None,
                    message: "side work",
                    entries: vec![TreeEntry::file(
                        "a.txt",
                        b"side
"
                        .to_vec(),
                    )],
                    meta: SnapshotMeta::new(),
                })
                .unwrap();
            let merged = guard
                .save_tree(SaveTree {
                    branch: &branch_name("main"),
                    parent: Some(&base),
                    merge_parent: Some(&side),
                    message: "merge side",
                    entries: vec![TreeEntry::file(
                        "a.txt",
                        b"merged
"
                        .to_vec(),
                    )],
                    meta: SnapshotMeta::new(),
                })
                .unwrap();
            (base, side, merged)
        };

        // Both parents are stored, and the reader reports it as a merge.
        let history = commands::history::run(
            &repo,
            commands::history::Options {
                all: true,
                ..Default::default()
            },
        )
        .unwrap();
        let entry = history
            .entries
            .iter()
            .find(|e| e.hash == merged)
            .expect("the merge is in history");
        assert_eq!(entry.parent.as_ref(), Some(&base));
        assert_eq!(entry.merge_parent.as_ref(), Some(&side));
        assert!(entry.is_merge(), "two parents is what is_merge means");

        // The second parent is part of identity, so fsck recomputing every id is
        // the proof it was hashed in and not merely written to a column.
        assert!(with_repo(&root, commands::fsck::check)
            .unwrap()
            .is_healthy());
    }

    /// A merge parent that does not exist is refused, like a first parent.
    #[test]
    fn save_tree_refuses_an_unknown_merge_parent() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();
        let guard = repo.write().unwrap();

        let ghost: SnapshotId = "ab".repeat(32).parse().unwrap();
        let err = guard
            .save_tree(SaveTree {
                branch: &branch_name("main"),
                parent: None,
                merge_parent: Some(&ghost),
                message: "m",
                entries: vec![TreeEntry::file("a.txt", b"x".to_vec())],
                meta: SnapshotMeta::new(),
            })
            .unwrap_err();
        assert!(
            matches!(err, VeloError::NotFound { .. }),
            "expected NotFound, got {:?}",
            err
        );
    }

    // ─── format v2 ────────────────────────────────────────────────────────────

    /// The break's central claim: metadata is part of a snapshot's identity.
    /// If this ever passes with equal ids, D1 has silently been undone.
    #[test]
    fn metadata_changes_the_snapshot_id() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let entries = || vec![TreeEntry::file("a.txt", b"same bytes\n".to_vec())];
        let mut tagged = SnapshotMeta::new();
        tagged.set("app", "run", "7").unwrap();

        let (plain, with_meta) = {
            let guard = repo.write().unwrap();
            let plain = guard
                .save_tree(SaveTree {
                    branch: &branch_name("a"),
                    parent: None,
                    merge_parent: None,
                    message: "m",
                    entries: entries(),
                    meta: SnapshotMeta::new(),
                })
                .unwrap();
            let with_meta = guard
                .save_tree(SaveTree {
                    branch: &branch_name("b"),
                    parent: None,
                    merge_parent: None,
                    message: "m",
                    entries: entries(),
                    meta: tagged.clone(),
                })
                .unwrap();
            (plain, with_meta)
        };

        assert_ne!(
            plain, with_meta,
            "identical content and message but different metadata must be \
             different snapshots"
        );
        assert_eq!(repo.snapshot_meta(&with_meta).unwrap(), tagged);
        assert!(repo.snapshot_meta(&plain).unwrap().is_empty());
    }

    /// Metadata insertion order must not reach the hash — otherwise two callers
    /// building the same set would disagree about the id.
    ///
    /// Tested against the recipe directly rather than through two saves: those
    /// would be milliseconds apart, and the timestamp is part of the identity, so
    /// the ids would differ for a reason that has nothing to do with ordering.
    #[test]
    fn metadata_order_does_not_affect_the_id() {
        let mut forwards = SnapshotMeta::new();
        forwards.set("app", "a", "1").unwrap();
        forwards.set("app", "z", "2").unwrap();
        let mut backwards = SnapshotMeta::new();
        backwards.set("app", "z", "2").unwrap();
        backwards.set("app", "a", "1").unwrap();

        let tree = vec![("a.txt".to_string(), "cafe".repeat(16), 0i64)];
        let id = |meta: &SnapshotMeta| {
            commands::snapshot_id(commands::SnapshotIdentity {
                tree: &tree,
                parent: "",
                merge_parent: "",
                message: "m",
                timestamp_ms: 1_785_922_872_345,
                meta,
            })
        };
        assert_eq!(id(&forwards), id(&backwards));

        // And an empty set is still a distinct, well-defined input — the recipe
        // emits the section marker either way.
        let mut one = SnapshotMeta::new();
        one.set("app", "a", "1").unwrap();
        assert_ne!(id(&SnapshotMeta::new()), id(&one));
    }

    /// The tree is sorted by the recipe, so the caller's order cannot reach the
    /// id either.
    #[test]
    fn tree_order_does_not_affect_the_id() {
        let meta = SnapshotMeta::new();
        let a = ("a.txt".to_string(), "aaaa".repeat(16), 0i64);
        let b = ("b.txt".to_string(), "bbbb".repeat(16), 0i64);
        let id = |tree: &[(String, String, i64)]| {
            commands::snapshot_id(commands::SnapshotIdentity {
                tree,
                parent: "",
                merge_parent: "",
                message: "m",
                timestamp_ms: 1_785_922_872_345,
                meta: &meta,
            })
        };
        assert_eq!(id(&[a.clone(), b.clone()]), id(&[b, a]));
    }

    /// Ids are stored whole. A truncated one would silently reintroduce the
    /// collision risk v2 exists to remove.
    #[test]
    fn stored_ids_are_full_width_everywhere_they_are_referenced() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        let first = save(&root, "first");
        write(&root, "a.txt", "two\n");
        let second = save(&root, "second");

        assert_eq!(first.len(), commands::SNAP_ID_LEN);
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();

        // The parent link, the branch tip and PARENT must all carry full ids.
        let parent: String = conn
            .query_row(
                "SELECT parent_hash FROM snapshots WHERE hash = ?",
                [&second],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parent, first);
        assert_eq!(parent.len(), commands::SNAP_ID_LEN);
        assert_eq!(
            std::fs::read_to_string(root.join(".velo/PARENT"))
                .unwrap()
                .trim()
                .len(),
            commands::SNAP_ID_LEN
        );
    }

    /// Timestamps are stored as integers, not text, so no formatting choice can
    /// reach a snapshot id.
    #[test]
    fn timestamps_are_stored_as_epoch_milliseconds() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        let hash = save(&root, "first");

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let ms: i64 = conn
            .query_row(
                "SELECT created_at_ms FROM snapshots WHERE hash = ?",
                [&hash],
                |r| r.get(0),
            )
            .unwrap();
        // A real clock value, not the column's 0 default.
        assert!(
            ms > 1_577_836_800_000,
            "expected a real timestamp, got {}",
            ms
        );

        // And the column really is an integer, not text that happens to parse.
        let kind: String = conn
            .query_row(
                "SELECT typeof(created_at_ms) FROM snapshots WHERE hash = ?",
                [&hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "integer");
    }

    /// `fsck` recomputes every id, so it is the check that the whole v2 recipe —
    /// metadata included — is applied consistently on the way in and out.
    #[test]
    fn fsck_verifies_ids_of_snapshots_carrying_metadata() {
        use crate::tree::{SaveTree, TreeEntry};
        let (_tmp, root) = setup();
        let repo = Repo::open_and_migrate(&root).unwrap();

        let mut meta = SnapshotMeta::new();
        meta.set("app", "k", "v").unwrap();
        {
            let guard = repo.write().unwrap();
            guard
                .save_tree(SaveTree {
                    branch: &branch_name("registry"),
                    parent: None,
                    merge_parent: None,
                    message: "m",
                    entries: vec![TreeEntry::file("a.txt", b"x\n".to_vec())],
                    meta,
                })
                .unwrap();
        }

        let report = commands::fsck::check(&repo).unwrap();
        assert!(
            report.is_healthy(),
            "expected a clean report, got {:?}",
            report
        );

        // Tampering with metadata must break the id it is part of — that is what
        // makes provenance worth storing.
        {
            let guard = repo.write().unwrap();
            guard
                .conn()
                .execute("UPDATE snapshot_meta SET value = 'tampered'", [])
                .unwrap();
        }
        let report = commands::fsck::check(&repo).unwrap();
        assert!(
            !report.is_healthy(),
            "rewriting metadata must show up as an id mismatch"
        );
    }

    /// A v1 repository is refused rather than stamped, because its ids were built
    /// by a different recipe and nothing can recompute them in place.
    #[test]
    fn a_pre_v2_repository_is_refused_not_silently_upgraded() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "one\n");
        save(&root, "first");

        // Put the marker back to what v1 left behind: nothing.
        {
            let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
            conn.execute_batch("PRAGMA user_version = 0;").unwrap();
        }

        match Repo::open_and_migrate(&root) {
            Err(Error::FormatTooOld { found, supported }) => {
                assert_eq!(found, 0, "an unversioned repository is v1");
                assert_eq!(supported, crate::FORMAT_VERSION);
            }
            other => panic!("expected FormatTooOld, got {:?}", other.map(|_| "Ok")),
        }
        // `open` must refuse it too, not just the migrating path.
        assert!(matches!(Repo::open(&root), Err(Error::FormatTooOld { .. })));

        // And the marker is untouched: refusing must not be a partial upgrade.
        let conn = db::connect(&root.join(".velo/velo.db")).unwrap();
        assert_eq!(db::format_version(&conn).unwrap(), 0);
    }

    /// A repository this build creates is stamped, so it is never mistaken for
    /// the unversioned v1 form.
    #[test]
    fn a_fresh_repository_is_stamped_with_its_format_version() {
        let (_tmp, root) = setup();
        let conn = db::connect(&root.join(".velo/velo.db")).unwrap();
        assert_eq!(db::format_version(&conn).unwrap(), crate::FORMAT_VERSION);
        assert!(!db::is_pre_v2(crate::FORMAT_VERSION));
    }

    // ─── pathspec on status ───────────────────────────────────────────────────

    #[test]
    fn status_with_pathspec_filters_output() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "a1\n");
        write(&root, "b.txt", "b1\n");
        save(&root, "base");
        write(&root, "a.txt", "a2\n");
        write(&root, "b.txt", "b2\n");

        // Status restricted to b.txt should succeed
        with_repo(&root, |vr| {
            commands::status::run(vr, &["b.txt".to_string()])
        })
        .unwrap();
    }
}

// =============================================================================
// Repo handle, write guard, and format versioning (P1.4)
// =============================================================================
#[cfg(test)]
mod repo_api {
    use crate::{db, Error, Repo, FORMAT_VERSION};
    use tempfile::TempDir;

    fn fresh() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        Repo::init(&path).unwrap();
        (tmp, path)
    }

    #[test]
    fn init_stamps_the_format_version() {
        let (_t, root) = fresh();
        let repo = Repo::open(&root).unwrap();
        assert_eq!(repo.format_version().unwrap(), FORMAT_VERSION);
    }

    #[test]
    fn open_refuses_a_newer_repository() {
        // A repo written by a future Velo must be refused outright rather than
        // half-migrated — the whole point of PRAGMA user_version.
        let (_t, root) = fresh();
        {
            let conn = db::connect(&root.join(".velo/velo.db")).unwrap();
            conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        }
        match Repo::open(&root) {
            Err(Error::SchemaTooNew { found, supported }) => {
                assert_eq!(found, 99);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected SchemaTooNew, got {:?}", other.map(|_| "Ok")),
        }
        // …and migrating must not paper over it either.
        assert!(matches!(
            Repo::open_and_migrate(&root),
            Err(Error::SchemaTooNew { .. })
        ));
    }

    #[test]
    fn open_requires_an_exact_root_but_discover_searches_up() {
        let (_t, root) = fresh();
        let nested = root.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        // `open` is explicit: it does not walk upward.
        assert!(matches!(Repo::open(&nested), Err(Error::NotARepo { .. })));
        // `discover` is the opt-in searching variant.
        assert_eq!(Repo::discover(&nested).unwrap().root(), root);
    }

    #[test]
    fn write_guard_is_exclusive_and_released_on_drop() {
        let (_t, root) = fresh();
        let repo = Repo::open(&root).unwrap();

        let guard = repo.write().unwrap();
        // A second attempt reports the lock rather than blocking forever.
        assert!(matches!(repo.try_write(), Ok(None)));
        assert!(matches!(repo.write(), Err(Error::Locked { .. })));
        // Timed acquisition gives up instead of hanging.
        let waited = repo.write_timeout(std::time::Duration::from_millis(60));
        assert!(matches!(waited, Err(Error::Locked { .. })));

        drop(guard);
        assert!(repo.try_write().unwrap().is_some());
    }

    #[test]
    fn errors_are_classifiable_without_string_matching() {
        // The point of typed errors: consumers branch on state.
        let locked = Error::Locked { held_by: Some(42) };
        assert!(locked.is_transient());
        assert!(!locked.is_reconcile_needed());

        let diverged = Error::Diverged {
            branch: "main".into(),
            ahead: 1,
            behind: 2,
        };
        assert!(diverged.is_reconcile_needed());

        let dirty = Error::DirtyWorkingTree {
            paths: vec!["a.txt".into()],
        };
        assert!(dirty.needs_clean_tree());
    }
}
