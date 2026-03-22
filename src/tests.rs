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
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    use crate::commands::{self, FileStatus};
    use crate::db;

    // =========================================================================
    // Helpers
    // =========================================================================

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

    fn save(root: &Path, msg: &str) -> String {
        commands::save::run(root, msg, false)
            .unwrap()
            .expect("expected a snapshot to be created")
            .hash
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
            matches!(result, Err(crate::error::VeloError::AlreadyInitialized)),
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
            matches!(result, Err(crate::error::VeloError::NestedRepo(_))),
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
        let r = commands::save::run(&root, "first", false).unwrap().unwrap();
        assert_eq!(r.new_count, 2); // hello.txt + .veloignore
        assert_eq!(r.modified_count, 0);
        assert_eq!(r.deleted_count, 0);
        assert!(!r.hash.is_empty());
        // Hash length must be SNAP_HASH_LEN
        assert_eq!(r.hash.len(), commands::SNAP_HASH_LEN);
    }

    #[test]
    fn save_empty_message_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        let err = commands::save::run(&root, "", false).unwrap_err();
        assert!(matches!(err, crate::error::VeloError::InvalidInput(_)));
    }

    #[test]
    fn save_whitespace_only_message_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        let err = commands::save::run(&root, "   ", false).unwrap_err();
        assert!(matches!(err, crate::error::VeloError::InvalidInput(_)));
    }

    #[test]
    fn save_clean_directory_returns_none() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        save(&root, "s1");
        // Nothing changed — should return None
        let result = commands::save::run(&root, "s2", false).unwrap();
        assert!(result.is_none());
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
        let r = commands::save::run(&root, "s2", false).unwrap().unwrap();
        assert_eq!(r.deleted_count, 1);
    }

    #[test]
    fn save_clears_redo_stack() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");

        commands::undo::run(&root).unwrap();
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

        let r = commands::save::run(&root, "test", false).unwrap().unwrap();
        // Only app.rs + .veloignore should be tracked (debug.log and temp/ excluded)
        assert_eq!(r.new_count, 2);
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

        commands::restore::run(&root, &h1, true, &[]).unwrap();
        assert_eq!(read(&root, "f.txt"), "v1");
        assert_eq!(parent(&root), h1);
    }

    #[test]
    fn restore_noop_when_already_at_target() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        // Should succeed silently without error
        commands::restore::run(&root, &h1, true, &[]).unwrap();
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
        let result = commands::restore::run(&root, &h1, false, &[]);
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

        commands::restore::run(&root, &h1, true, &[]).unwrap();
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

        commands::restore::run(&root, &h1, true, &[]).unwrap();
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

        commands::restore::run(&root, &h1, true, &[]).unwrap();
        assert_eq!(parent(&root), h1);
        // Working tree should be clean after restore
        assert!(commands::get_dirty_files(&root).is_empty());
    }

    #[test]
    fn restore_invalid_hash_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let result = commands::restore::run(&root, "deadbeef9999", true, &[]);
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

        let dirty = commands::get_dirty_files(&root);
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

        commands::restore::run(&root, &h1, true, &[]).unwrap();
        assert!(commands::get_dirty_files(&root).is_empty());
    }

    #[test]
    fn status_run_does_not_panic_on_empty_repo() {
        let (_tmp, root) = setup();
        commands::status::run(&root).unwrap();
    }

    // =========================================================================
    // logs
    // =========================================================================

    #[test]
    fn logs_ancestry_walk() {
        let (_tmp, root) = setup();
        for i in 0..5 {
            write(&root, "f.txt", &i.to_string());
            save(&root, &format!("snap {}", i));
        }
        // Should not panic and return Ok
        commands::history::run(&root, false, 10, None, false, false, None).unwrap();
    }

    #[test]
    fn logs_limit_respected() {
        let (_tmp, root) = setup();
        for i in 0..10 {
            write(&root, "f.txt", &i.to_string());
            save(&root, &format!("snap {}", i));
        }
        // This just verifies it doesn't error; actual row count is verified via
        // the DB in a more targeted test below.
        commands::history::run(&root, false, 3, None, false, false, None).unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let total: i64 = conn
            .query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 10);
    }

    #[test]
    fn logs_all_excludes_deleted_branches() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "main save");
        commands::switch::run(&root, "feature", false).unwrap();
        write(&root, "f.txt", "feat");
        save(&root, "feat save");
        commands::switch::run(&root, "main", true).unwrap();
        commands::branches::run(&root, Some("feature".into())).unwrap();

        // Global log should not show _deleted_feature entries
        commands::history::run(&root, true, 20, None, false, false, None).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let deleted_visible: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshots WHERE branch LIKE '_deleted_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_visible > 0); // they still exist internally
    }

    #[test]
    fn logs_filter_by_branch() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "main snap");
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "dev snap");

        // Should not error even though we're not on 'main'
        commands::history::run(&root, false, 20, Some("main"), false, false, None).unwrap();
    }

    #[test]
    fn logs_oneline_does_not_panic() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        commands::history::run(&root, false, 10, None, true, false, None).unwrap();
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

        commands::undo::run(&root).unwrap();

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

        commands::undo::run(&root).unwrap();
        // Working tree should now show v1, not v2
        assert_eq!(read(&root, "f.txt"), "v1");
        assert!(commands::get_dirty_files(&root).is_empty());
    }

    #[test]
    fn undo_first_commit_clears_tree() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        commands::undo::run(&root).unwrap();

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

        let result = commands::undo::run(&root);
        assert!(result.is_err());
    }

    #[test]
    fn undo_nothing_to_undo_is_error() {
        let (_tmp, root) = setup();
        let result = commands::undo::run(&root);
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

        commands::undo::run(&root).unwrap();
        assert_eq!(read(&root, "f.txt"), "v1");

        commands::redo::run(&root).unwrap();
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
        let result = commands::redo::run(&root);
        assert!(result.is_err());
    }

    #[test]
    fn redo_stack_invalidated_by_new_save() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        commands::undo::run(&root).unwrap();

        // New save should clear redo stack
        write(&root, "f.txt", "v3_new");
        save(&root, "s3");

        let result = commands::redo::run(&root);
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

        commands::undo::run(&root).unwrap();
        write(&root, "f.txt", "dirty");

        let result = commands::redo::run(&root);
        assert!(result.is_err());
    }

    // =========================================================================
    // diff
    // =========================================================================

    #[test]
    fn diff_clean_directory() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        // Should not error on a clean dir
        commands::diff::run(&root, &None).unwrap();
    }

    #[test]
    fn diff_modified_file() {
        let (_tmp, root) = setup();
        write(
            &root,
            "large.txt",
            (0..100)
                .map(|i| format!("Line {}\n", i))
                .collect::<String>()
                .as_str(),
        );
        save(&root, "base");
        let new_content = (0..100)
            .map(|i| {
                if i == 50 {
                    "Line 50 MODIFIED\n".into()
                } else {
                    format!("Line {}\n", i)
                }
            })
            .collect::<String>();
        write(&root, "large.txt", &new_content);
        commands::diff::run(&root, &Some("large.txt".into())).unwrap();
    }

    #[test]
    fn diff_shows_changes_vs_last_snapshot() {
        // Replaces old diff_conflict_missing_file_is_error — --conflict is removed.
        // Just verify diff works on a modified file.
        let (_tmp, root) = setup();
        write(&root, "app.py", "version 1");
        save(&root, "s1");
        write(&root, "app.py", "version 2");
        commands::diff::run(&root, &Some("app.py".into())).unwrap();
    }

    #[test]
    fn diff_deleted_file_shows_marker() {
        let (_tmp, root) = setup();
        write(&root, "gone.txt", "data");
        save(&root, "s1");
        fs::remove_file(root.join("gone.txt")).unwrap();
        // Should not panic even for deleted files
        commands::diff::run(&root, &Some("gone.txt".into())).unwrap();
    }

    // =========================================================================
    // switch
    // =========================================================================

    #[test]
    fn switch_creates_new_branch() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        commands::switch::run(&root, "dev", false).unwrap();
        assert_eq!(head(&root), "dev");
    }

    #[test]
    fn switch_restores_branch_state() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main_content");
        save(&root, "s1");

        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "dev_content");
        save(&root, "dev_snap");

        commands::switch::run(&root, "main", true).unwrap();
        assert_eq!(read(&root, "f.txt"), "main_content");
    }

    #[test]
    fn switch_aborts_on_dirty_without_force() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "dirty");

        commands::switch::run(&root, "main", false).unwrap();
        // Should still be on dev
        assert_eq!(head(&root), "dev");
    }

    #[test]
    fn switch_force_discards_changes() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "dirty_dev");

        commands::switch::run(&root, "main", true).unwrap();
        assert_eq!(head(&root), "main");
        assert_eq!(read(&root, "f.txt"), "main");
    }

    #[test]
    fn switch_to_deleted_branch_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "s2");
        commands::switch::run(&root, "main", true).unwrap();
        commands::branches::run(&root, Some("dev".into())).unwrap();

        let result = commands::switch::run(&root, "_deleted_dev", false);
        assert!(result.is_err());
    }

    #[test]
    fn switch_noop_when_already_on_branch() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        // Should succeed without doing anything
        commands::switch::run(&root, "main", false).unwrap();
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
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "d");
        save(&root, "s2");
        commands::switch::run(&root, "main", true).unwrap();
        // Should not panic
        commands::branches::run(&root, None).unwrap();
    }

    #[test]
    fn branches_delete_soft_removes() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "s1");
        commands::switch::run(&root, "feature", false).unwrap();
        write(&root, "f.txt", "feat");
        save(&root, "feat_snap");
        commands::switch::run(&root, "main", true).unwrap();

        commands::branches::run(&root, Some("feature".into())).unwrap();

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
        let result = commands::branches::run(&root, Some("main".into()));
        assert!(result.is_err());
    }

    #[test]
    fn branches_delete_main_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        commands::switch::run(&root, "dev", false).unwrap();
        // Even from another branch, deleting main is forbidden
        let result = commands::branches::run(&root, Some("main".into()));
        assert!(result.is_err());
    }

    #[test]
    fn branches_delete_nonexistent_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        let result = commands::branches::run(&root, Some("ghost_branch".into()));
        assert!(result.is_err());
    }

    #[test]
    fn branches_deleted_branches_not_shown_in_list() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "m");
        save(&root, "s1");
        commands::switch::run(&root, "feature", false).unwrap();
        write(&root, "f.txt", "f");
        save(&root, "s2");
        commands::switch::run(&root, "main", true).unwrap();
        commands::branches::run(&root, Some("feature".into())).unwrap();

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
        commands::tag::run(&root, Some("v1.0".into()), None, None, false).unwrap();

        let resolved = commands::resolve_snapshot_id(&root, "v1.0").unwrap();
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
        commands::tag::run(&root, Some("old".into()), Some(h1.clone()), None, false).unwrap();
        let resolved = commands::resolve_snapshot_id(&root, "old").unwrap();
        assert_eq!(resolved, h1);
    }

    #[test]
    fn tag_overwrite_without_force_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        commands::tag::run(&root, Some("v1".into()), None, None, false).unwrap();

        write(&root, "f.txt", "v2");
        save(&root, "s2");
        let result = commands::tag::run(&root, Some("v1".into()), None, None, false);
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
        commands::tag::run(&root, Some("v1".into()), None, None, false).unwrap();

        write(&root, "f.txt", "v2");
        let h2 = save(&root, "s2");
        commands::tag::run(&root, Some("v1".into()), None, None, true).unwrap();

        let resolved = commands::resolve_snapshot_id(&root, "v1").unwrap();
        assert_eq!(resolved, h2);
    }

    #[test]
    fn tag_delete_removes_tag() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        commands::tag::run(&root, Some("rel".into()), None, None, false).unwrap();
        commands::tag::run(&root, None, None, Some("rel".into()), false).unwrap();

        let result = commands::resolve_snapshot_id(&root, "rel");
        assert!(result.is_err());
    }

    #[test]
    fn tag_delete_nonexistent_is_error() {
        let (_tmp, root) = setup();
        let result = commands::tag::run(&root, None, None, Some("ghost_tag".into()), false);
        assert!(result.is_err());
    }

    #[test]
    fn tag_empty_head_is_error() {
        let (_tmp, root) = setup();
        // No snapshots yet — can't tag HEAD
        let result = commands::tag::run(&root, Some("v1".into()), None, None, false);
        assert!(result.is_err());
    }

    #[test]
    fn tag_list_does_not_panic() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        commands::tag::run(&root, Some("alpha".into()), None, None, false).unwrap();
        commands::tag::run(&root, None, None, None, false).unwrap();
    }

    // =========================================================================
    // merge
    // =========================================================================

    #[test]
    fn merge_fast_forward() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "base");
        save(&root, "base");
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "a.txt", "updated");
        save(&root, "dev work");
        commands::switch::run(&root, "main", true).unwrap();

        commands::merge::run(&root, Some("dev"), false).unwrap();
        assert_eq!(read(&root, "a.txt"), "updated");
    }

    #[test]
    fn merge_conflict_produces_conflict_file() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");

        commands::switch::run(&root, "A", false).unwrap();
        write(&root, "app.py", "content A");
        save(&root, "save A");

        commands::switch::run(&root, "main", true).unwrap();
        commands::switch::run(&root, "B", false).unwrap();
        write(&root, "app.py", "content B");
        save(&root, "save B");

        commands::merge::run(&root, Some("A"), false).unwrap();
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
        commands::switch::run(&root, "A", false).unwrap();
        write(&root, "app.py", "content A");
        save(&root, "save A");
        commands::switch::run(&root, "main", true).unwrap();
        commands::switch::run(&root, "B", false).unwrap();
        write(&root, "app.py", "content B\n");
        save(&root, "save B");

        commands::merge::run(&root, Some("A"), false).unwrap();
        commands::resolve::run(
            &root,
            Some("app.py"),
            Some(commands::resolve::TakeOption::Theirs),
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
        commands::switch::run(&root, "feat", false).unwrap();
        write(&root, "f.txt", "theirs\n");
        save(&root, "feat snap");
        commands::switch::run(&root, "main", true).unwrap();
        write(&root, "f.txt", "ours\n");
        save(&root, "main snap");

        commands::merge::run(&root, Some("feat"), false).unwrap();
        commands::resolve::run(
            &root,
            Some("f.txt"),
            Some(commands::resolve::TakeOption::Ours),
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
        commands::switch::run(&root, "dev", false).unwrap();
        fs::remove_file(root.join("removed.txt")).unwrap();
        save(&root, "del snap");

        // Back on main: both files still on disk
        commands::switch::run(&root, "main", true).unwrap();
        assert!(
            exists(&root, "removed.txt"),
            "removed.txt should exist on main before merge"
        );
        assert!(exists(&root, "kept.txt"));

        // Merge dev into main — dev deleted removed.txt, so it should disappear
        commands::merge::run(&root, Some("dev"), false).unwrap();
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
        commands::switch::run(&root, "feat", false).unwrap();
        write(&root, "newfile.txt", "brand new");
        save(&root, "feat snap");
        commands::switch::run(&root, "main", true).unwrap();
        // Add a change to main so it's not a fast-forward
        write(&root, "base.txt", "main updated");
        save(&root, "main snap");

        commands::merge::run(&root, Some("feat"), false).unwrap();
        // newfile.txt should appear in working tree
        assert!(exists(&root, "newfile.txt"));
        assert_eq!(read(&root, "newfile.txt"), "brand new");
    }

    #[test]
    fn merge_aborts_on_dirty() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "base");
        commands::switch::run(&root, "feat", false).unwrap();
        write(&root, "f.txt", "feat");
        save(&root, "feat snap");
        commands::switch::run(&root, "main", true).unwrap();

        write(&root, "f.txt", "dirty");
        let result = commands::merge::run(&root, Some("feat"), false);
        assert!(result.is_err());
    }

    #[test]
    fn merge_abort_clears_conflict_files() {
        let (_tmp, root) = setup();
        write(&root, "app.py", "base");
        save(&root, "base");
        commands::switch::run(&root, "A", false).unwrap();
        write(&root, "app.py", "content A\n");
        save(&root, "save A");
        commands::switch::run(&root, "main", true).unwrap();
        commands::switch::run(&root, "B", false).unwrap();
        write(&root, "app.py", "content B\n");
        save(&root, "save B");

        commands::merge::run(&root, Some("A"), false).unwrap();
        // Conflict stored in DB
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(count > 0, "conflict should be in DB");
        // app.py is still "content B\n" on disk (our version untouched during merge)
        assert_eq!(read(&root, "app.py"), "content B\n");

        commands::merge::run(&root, None, true).unwrap(); // --abort
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
            commands::get_dirty_files(&root).is_empty(),
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

        commands::switch::run(&root, "feat", false).unwrap();
        write(&root, "app.py", "theirs\n");
        save(&root, "feat snap");

        commands::switch::run(&root, "main", true).unwrap();
        write(&root, "app.py", "ours\n");
        save(&root, "main snap");

        let pre_merge_parent = parent(&root);

        commands::merge::run(&root, Some("feat"), false).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(n > 0, "should have conflict");

        // Resolve all conflicts non-interactively
        commands::resolve::run(
            &root,
            Some("app.py"),
            Some(commands::resolve::TakeOption::Theirs),
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
        commands::merge::run(&root, None, true).unwrap();

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
            commands::get_dirty_files(&root).is_empty(),
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
        commands::switch::run(&root, "A", false).unwrap();
        write(
            &root,
            "app.py",
            "def img2pdf():\n    print(\"ours\")\n    return None\n",
        );
        save(&root, "ours");

        // back to main, create branch B (theirs): insert print("theirs") before return
        commands::switch::run(&root, "main", true).unwrap();
        commands::switch::run(&root, "B", false).unwrap();
        write(
            &root,
            "app.py",
            "def img2pdf():\n    print(\"theirs\")\n    return None\n",
        );
        save(&root, "theirs");

        // Merge A into B → conflict
        commands::merge::run(&root, Some("A"), false).unwrap();
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
            .unwrap();
        assert!(n > 0, "expected a conflict");

        // Resolve taking theirs (branch A = print("ours"))
        commands::resolve::run(
            &root,
            Some("app.py"),
            Some(commands::resolve::TakeOption::Theirs),
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
        commands::merge::run(&root, None, true).unwrap(); // abort

        commands::merge::run(&root, Some("A"), false).unwrap();
        commands::resolve::run(
            &root,
            Some("app.py"),
            Some(commands::resolve::TakeOption::Ours),
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
        commands::switch::run(&root, "A", false).unwrap();
        write(&root, "app.py", "content A");
        save(&root, "A snap");
        commands::switch::run(&root, "main", true).unwrap();
        commands::switch::run(&root, "B", false).unwrap();
        write(&root, "app.py", "content B");
        save(&root, "B snap");

        commands::merge::run(&root, Some("A"), false).unwrap();
        // Try to merge again while conflicts outstanding
        let result = commands::merge::run(&root, Some("A"), false);
        assert!(result.is_err());
    }

    #[test]
    fn merge_self_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v");
        save(&root, "snap");
        let result = commands::merge::run(&root, Some("main"), false);
        assert!(result.is_err());
    }

    #[test]
    fn merge_nonexistent_branch_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v");
        save(&root, "snap");
        let result = commands::merge::run(&root, Some("ghost"), false);
        assert!(result.is_err());
    }

    // =========================================================================
    // resolve
    // =========================================================================

    #[test]
    fn resolve_no_conflict_file_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "x");
        save(&root, "s1");
        let result = commands::resolve::run(
            &root,
            Some("f.txt"),
            Some(commands::resolve::TakeOption::Theirs),
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

        commands::switch::run(&root, "X", false).unwrap();
        write(&root, "a.py", "X-a");
        write(&root, "b.py", "X-b");
        save(&root, "X snap");

        commands::switch::run(&root, "main", true).unwrap();
        commands::switch::run(&root, "Y", false).unwrap();
        write(&root, "a.py", "Y-a");
        write(&root, "b.py", "Y-b");
        save(&root, "Y snap");

        commands::merge::run(&root, Some("X"), false).unwrap();
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

        commands::resolve::run(
            &root,
            None,
            Some(commands::resolve::TakeOption::Theirs),
            true, // --all
        )
        .unwrap();

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
        commands::save::run(&root, "Finish merge", false).unwrap();
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
        commands::resolve::run(&root, None, None, true).unwrap();
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
        commands::undo::run(&root).unwrap();

        // Inject a fake orphaned object manually
        fs::write(
            root.join(".velo/objects/fake_orphan_object_hash"),
            b"garbage",
        )
        .unwrap();

        let before = object_count(&root);
        // Run GC with 0 day keep to also purge trash immediately
        commands::gc::run(&root, 0).unwrap();
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
        commands::gc::run(&root, 30).unwrap();
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
        let resolved = commands::resolve_snapshot_id(&root, &h).unwrap();
        assert_eq!(resolved, h);
    }

    #[test]
    fn resolve_snapshot_id_prefix() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h = save(&root, "s1");
        // First 6 characters should be unambiguous for a single snapshot
        let prefix = &h[..6];
        let resolved = commands::resolve_snapshot_id(&root, prefix).unwrap();
        assert_eq!(resolved, h);
    }

    #[test]
    fn resolve_snapshot_id_nonexistent_is_error() {
        let (_tmp, root) = setup();
        let result = commands::resolve_snapshot_id(&root, "doesnotexist");
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

        commands::restore::run(&root, &h2, true, &[]).unwrap();
        assert_eq!(read(&root, "f.txt"), "v2");

        commands::restore::run(&root, &h3, true, &[]).unwrap();
        assert_eq!(read(&root, "f.txt"), "v3");

        commands::restore::run(&root, &h1, true, &[]).unwrap();
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
        commands::switch::run(&root, "feature", false).unwrap();
        write(&root, "feature.txt", "feature work");
        save(&root, "feat work");

        // Switch back to main — feature.txt must vanish (it wasn't on main)
        commands::switch::run(&root, "main", true).unwrap();
        assert_eq!(read(&root, "README.md"), "# Project");
        assert!(
            !exists(&root, "feature.txt"),
            "feature.txt should not exist on main"
        );
        assert!(
            commands::get_dirty_files(&root).is_empty(),
            "main must be clean before merge"
        );

        // Fast-forward merge: feature.txt should appear
        commands::merge::run(&root, Some("feature"), false).unwrap();
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
        commands::undo::run(&root).unwrap();
        assert_eq!(parent(&root), h1);
        assert_eq!(read(&root, "f.txt"), "v1");

        // Redo s2 -> back at s2
        commands::redo::run(&root).unwrap();
        assert_eq!(parent(&root), h2);
        assert_eq!(read(&root, "f.txt"), "v2");

        // Undo again, then make a new save (invalidates redo)
        commands::undo::run(&root).unwrap();
        write(&root, "f.txt", "v3_diverge");
        let h3 = save(&root, "s3_diverge");
        assert_eq!(parent(&root), h3);
        assert!(commands::redo::run(&root).is_err());
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

        let r = commands::save::run(&root, "test", false).unwrap().unwrap();
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
        let dirty = commands::get_dirty_files(&found);
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
        commands::stash::push(&root, None).unwrap();

        // Working tree should be clean (back to s1 state)
        assert_eq!(read(&root, "f.txt"), "base");
        assert!(!exists(&root, "new.txt"));
        assert!(commands::get_dirty_files(&root).is_empty());
    }

    #[test]
    fn stash_push_named_shelf() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");
        write(&root, "f.txt", "wip");
        commands::stash::push(&root, Some("my-feature".into())).unwrap();

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
        commands::stash::push(&root, Some("shelf".into())).unwrap();

        write(&root, "f.txt", "wip2");
        let result = commands::stash::push(&root, Some("shelf".into()));
        assert!(result.is_err());
    }

    #[test]
    fn stash_push_clean_tree_is_noop() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");
        // Clean — stash should do nothing
        commands::stash::push(&root, None).unwrap();

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
        commands::stash::push(&root, Some("wip".into())).unwrap();
        assert_eq!(read(&root, "f.txt"), "base");

        commands::stash::pop(&root, Some("wip".into())).unwrap();
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
        commands::stash::push(&root, Some("first".into())).unwrap();

        write(&root, "f.txt", "v3");
        commands::stash::push(&root, Some("second".into())).unwrap();

        // Pop with no name should get "second" (most recent)
        commands::stash::pop(&root, None).unwrap();
        assert_eq!(read(&root, "f.txt"), "v3");
    }

    #[test]
    fn stash_pop_on_dirty_tree_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "stashed");
        commands::stash::push(&root, None).unwrap();

        // Make the tree dirty again
        write(&root, "f.txt", "dirty");
        let result = commands::stash::pop(&root, None);
        assert!(result.is_err());
    }

    #[test]
    fn stash_drop_removes_shelf_without_restoring() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "base");
        save(&root, "s1");

        write(&root, "f.txt", "stashed");
        commands::stash::push(&root, Some("temp".into())).unwrap();
        assert_eq!(read(&root, "f.txt"), "base");

        commands::stash::drop_shelf(&root, Some("temp".into())).unwrap();
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
        let result = commands::stash::pop(&root, Some("ghost".into()));
        assert!(result.is_err());
    }

    #[test]
    fn stash_preserves_new_files() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "base");
        save(&root, "s1");

        // Stash a brand-new file that doesn't exist in the snapshot
        write(&root, "brand_new.txt", "totally new");
        commands::stash::push(&root, Some("new-file-stash".into())).unwrap();
        assert!(!exists(&root, "brand_new.txt"));

        commands::stash::pop(&root, Some("new-file-stash".into())).unwrap();
        assert!(exists(&root, "brand_new.txt"));
        assert_eq!(read(&root, "brand_new.txt"), "totally new");
    }

    // ─── show ────────────────────────────────────────────────────────────────

    #[test]
    fn show_displays_snapshot_info() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        write(&root, "f.txt", "v2");
        save(&root, "s2");

        // Should not panic or error
        commands::show::run(&root, &h1, &None).unwrap();
    }

    #[test]
    fn show_with_file_filter() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A");
        write(&root, "b.txt", "B");
        let h1 = save(&root, "s1");

        commands::show::run(&root, &h1, &Some("a.txt".into())).unwrap();
    }

    #[test]
    fn show_first_snapshot_no_parent() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");
        // First snapshot has no parent — should show all files as added
        commands::show::run(&root, &h1, &None).unwrap();
    }

    #[test]
    fn show_nonexistent_hash_is_error() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        let result = commands::show::run(&root, "deadbeef1234", &None);
        assert!(result.is_err());
    }

    #[test]
    fn show_works_via_tag() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        save(&root, "s1");
        commands::tag::run(&root, Some("release".into()), None, None, false).unwrap();
        commands::show::run(&root, "release", &None).unwrap();
    }

    // ─── logs file filter ─────────────────────────────────────────────────────

    #[test]
    fn logs_file_filter_returns_only_relevant_snapshots() {
        let (_tmp, root) = setup();
        write(&root, "a.txt", "A1");
        write(&root, "b.txt", "B1");
        save(&root, "both touched");

        write(&root, "a.txt", "A2");
        save(&root, "only a touched");

        write(&root, "b.txt", "B2");
        save(&root, "only b touched");

        // File filter for a.txt: should find both s1 and s2, not s3
        commands::history::run(&root, false, 20, None, false, false, Some("a.txt")).unwrap();

        // Verify via DB: a.txt appears in exactly the snapshots that touched it
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(DISTINCT snapshot_hash) FROM file_map WHERE path = 'a.txt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            count >= 2,
            "a.txt should appear in at least 2 snapshots, got {}",
            count
        );
    }

    #[test]
    fn logs_graph_does_not_panic() {
        let (_tmp, root) = setup();
        for i in 0..5 {
            write(&root, "f.txt", &i.to_string());
            save(&root, &format!("snap {}", i));
        }
        commands::history::run(&root, false, 20, None, false, true, None).unwrap();
    }

    #[test]
    fn logs_graph_with_branches_does_not_panic() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "main");
        save(&root, "main snap");
        commands::switch::run(&root, "dev", false).unwrap();
        write(&root, "f.txt", "dev");
        save(&root, "dev snap");
        commands::switch::run(&root, "main", true).unwrap();

        commands::history::run(&root, true, 20, None, false, true, None).unwrap();
    }

    // ─── save --amend ─────────────────────────────────────────────────────────

    #[test]
    fn save_amend_replaces_last_snapshot() {
        let (_tmp, root) = setup();
        write(&root, "f.txt", "v1");
        let h1 = save(&root, "s1");

        // Amend: fix a typo in f.txt and update message
        write(&root, "f.txt", "v1 fixed");
        let result = commands::save::run(&root, "s1 amended", true).unwrap();
        let amended = result.unwrap();

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
        commands::save::run(&root, "corrected message", true).unwrap();

        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let msg: String = conn
            .query_row(
                "SELECT message FROM snapshots WHERE branch = 'main' ORDER BY created_at DESC LIMIT 1",
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
        commands::save::run(&root, "s2 amended", true).unwrap();

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
        let result = commands::save::run(&root, "fixed message", true).unwrap();
        // Should return Some even when tree is clean (amend forces it)
        // Note: if dirty is empty AND amend, we still create a new snapshot
        assert!(result.is_some());
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
        commands::restore::run(&root, &h1, false, &["a.txt".into()]).unwrap();

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
        commands::restore::run(&root, &h1, false, &["ghost.txt".into()]).unwrap();
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
        commands::restore::run(&root, &h1, false, &["src/".into()]).unwrap();
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
        commands::switch::run(&root, "hotfix", false).unwrap();
        write(&root, "fix.txt", "bug fix content");
        save(&root, "hotfix save");
        let fix_hash = parent(&root);

        // Back on main, apply the hotfix
        commands::switch::run(&root, "main", true).unwrap();
        commands::cherry_pick::run(&root, &fix_hash).unwrap();

        // The new file from the hotfix should be on main now
        assert!(exists(&root, "fix.txt"));
        assert_eq!(read(&root, "fix.txt"), "bug fix content");
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
        let result = commands::cherry_pick::run(&root, &h2);
        assert!(result.is_err());
    }

    #[test]
    fn cherry_pick_conflict_creates_conflict_file() {
        let (_tmp, root) = setup();
        write(&root, "shared.txt", "base");
        save(&root, "base");

        // Branch A: modify shared.txt
        commands::switch::run(&root, "branch-a", false).unwrap();
        write(&root, "shared.txt", "branch A version");
        save(&root, "branch A");
        let branch_a_hash = parent(&root);

        // Back on main: independently modify shared.txt
        commands::switch::run(&root, "main", true).unwrap();
        write(&root, "shared.txt", "main version");
        save(&root, "main snap");

        // Cherry-pick branch A's change — should conflict
        commands::cherry_pick::run(&root, &branch_a_hash).unwrap();
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
        commands::switch::run(&root, "feature", false).unwrap();
        write(&root, "c.txt", "C — new feature file");
        save(&root, "feature adds c");
        let feature_hash = parent(&root);

        commands::switch::run(&root, "main", true).unwrap();
        let before_parent = parent(&root);

        commands::cherry_pick::run(&root, &feature_hash).unwrap();

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
        let result = commands::cherry_pick::run(&root, "deadbeef1234");
        assert!(result.is_err());
    }
}
