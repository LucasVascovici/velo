//! Objects stored in the database (14.11-a).

use crate::commands::{self, fsck::Problem};
use crate::{db, BranchName, Repo, SnapshotId, SnapshotMeta};
use crate::{Error, TagName};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn with_repo<T>(root: &Path, f: impl FnOnce(&Repo) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    f(&repo)
}

fn with_write<T>(root: &Path, f: impl FnOnce(&crate::WriteGuard) -> T) -> T {
    let repo = Repo::open_and_migrate(root).expect("open repository");
    let guard = repo.write().expect("take write lock");
    f(&guard)
}

fn setup() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    commands::init::run(&path).unwrap();
    (tmp, path)
}

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

fn sid(hash: impl AsRef<str>) -> SnapshotId {
    SnapshotId::from_stored(hash.as_ref())
}

fn branch_name(name: &str) -> BranchName {
    name.parse().expect("valid branch name")
}

fn save(root: &Path, msg: &str) -> String {
    with_write(root, |g| {
        commands::save::run(g, Some(msg), commands::save::Options::default())
    })
    .unwrap();
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    conn.query_row(
        "SELECT hash FROM snapshots ORDER BY rowid DESC LIMIT 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
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

// =========================================================================
// Database object location (docs/FORMAT.md 7.2)
// =========================================================================

/// A repository whose objects live in `velo.db` rather than in files.
fn setup_db() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();
    crate::commands::init::run_with(
        &path,
        crate::InitOptions::new().objects(crate::ObjectLocation::Database),
    )
    .unwrap();
    (tmp, path)
}

/// Plain files under `.velo/objects`; zero when the directory is absent.
fn object_file_count(root: &Path) -> usize {
    fs::read_dir(root.join(".velo/objects"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

fn object_rows(root: &Path) -> i64 {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    conn.query_row("SELECT count(*) FROM objects", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn database_objects_round_trip_through_the_tree_api() {
    use crate::tree::{SaveTree, TreeEntry};
    let (_tmp, root) = setup_db();
    let repo = Repo::open(&root).unwrap();
    assert_eq!(repo.object_location(), crate::ObjectLocation::Database);
    let branch = branch_name("main");
    let snap = {
        let guard = repo.write().unwrap();
        guard
            .save_tree(SaveTree {
                branch: &branch,
                parent: None,
                merge_parent: None,
                message: "in the database",
                entries: vec![
                    TreeEntry::file("a.txt", b"alpha\n".to_vec()),
                    TreeEntry::file("dir/b.txt", b"beta\n".to_vec()),
                ],
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(1_000),
                renames: &[],
            })
            .unwrap()
    };
    let tree = repo.tree_at(&snap).unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(repo.read_file_at(&snap, "a.txt").unwrap(), b"alpha\n");
    assert_eq!(repo.read_file_at(&snap, "dir/b.txt").unwrap(), b"beta\n");

    assert_eq!(object_file_count(&root), 0, "no object files on disk");
    assert_eq!(object_rows(&root), 2);
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    let setting: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'objects'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(setting, "database");
    assert!(with_repo(&root, commands::fsck::check)
        .unwrap()
        .is_healthy());
}

#[test]
fn database_objects_survive_a_parallel_working_tree_save_and_restore() {
    let (_tmp, root) = setup_db();
    for i in 0..150 {
        write(
            &root,
            &format!("d{}/f{}.txt", i % 7, i),
            &format!("content of file {i}\n"),
        );
    }
    let snap = save(&root, "many files");
    assert_eq!(object_file_count(&root), 0);
    assert_eq!(object_rows(&root), 151, "150 files and .veloignore");

    // Wipe the working files, then bring them back from the database.
    for i in 0..150 {
        fs::remove_file(root.join(format!("d{}/f{}.txt", i % 7, i))).unwrap();
    }
    with_write(&root, |vr| {
        commands::restore::run(
            vr,
            &sid(&snap),
            commands::restore::Options {
                force: true,
                ..Default::default()
            },
        )
    })
    .unwrap();
    for i in 0..150 {
        assert_eq!(
            read(&root, &format!("d{}/f{}.txt", i % 7, i)),
            format!("content of file {i}\n")
        );
    }
    assert!(with_repo(&root, commands::fsck::check)
        .unwrap()
        .is_healthy());
}

#[test]
fn a_bundle_moves_between_object_locations_with_the_same_ids() {
    let bd = TempDir::new().unwrap();
    for (from_db, to_db) in [(false, true), (true, false)] {
        let (_ta, a) = if from_db { setup_db() } else { setup() };
        write(&a, "f.txt", "v1\n");
        save(&a, "s1");
        write(&a, "f.txt", "v2\n");
        write(&a, "g.txt", "second\n");
        let tip = save(&a, "s2");

        let bundle = bd.path().join(format!("{from_db}.velo"));
        with_repo(&a, |vr| commands::bundle::create(vr, &bundle, None)).unwrap();

        let (_tb, b) = if to_db { setup_db() } else { setup() };
        with_write(&b, |vr| commands::bundle::apply(vr, &bundle)).unwrap();

        assert!(snapshot_exists(&b, &tip), "same snapshot id on both sides");
        let repo_b = Repo::open(&b).unwrap();
        assert_eq!(repo_b.read_file_at(&sid(&tip), "f.txt").unwrap(), b"v2\n");
        assert_eq!(
            repo_b.read_file_at(&sid(&tip), "g.txt").unwrap(),
            b"second\n"
        );
        assert!(commands::fsck::check(&repo_b).unwrap().is_healthy());
        if to_db {
            assert_eq!(object_file_count(&b), 0);
        }
    }
}

#[test]
fn gc_removes_unreferenced_database_objects() {
    let (_tmp, root) = setup_db();
    write(&root, "f.txt", "main\n");
    save(&root, "main save");
    write(&root, "only_in_the_second.txt", "second only\n");
    save(&root, "second save");
    // Undo moves the snapshot to the trash, where `keep_days: 0` expires it
    // and leaves its object unreferenced.
    with_write(&root, commands::undo::run).unwrap();
    let before = object_rows(&root);

    let collected = with_write(&root, |vr| {
        commands::gc::run(
            vr,
            commands::gc::Options {
                keep_days: 0,
                ..Default::default()
            },
        )
    })
    .unwrap();
    assert!(collected.objects >= 1, "{collected:?}");
    assert!(object_rows(&root) < before, "rows were removed");
    assert!(with_repo(&root, commands::fsck::check)
        .unwrap()
        .is_healthy());
}

#[test]
fn fsck_reports_a_tampered_database_blob() {
    let (_tmp, root) = setup_db();
    write(&root, "f.txt", "genuine\n");
    save(&root, "s1");
    let hash: String = {
        let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
        let frame = zstd::encode_all(&b"tampered"[..], 1).unwrap();
        let hash: String = conn
            .query_row("SELECT hash FROM objects LIMIT 1", [], |r| r.get(0))
            .unwrap();
        conn.execute("UPDATE objects SET data = ?", [frame])
            .unwrap();
        hash
    };
    let report = with_repo(&root, commands::fsck::check).unwrap();
    assert!(
        report.problems.iter().any(|p| matches!(
            p,
            Problem::CorruptObject { hash: h, .. } if *h == hash
        )),
        "expected CorruptObject for {hash}, got {:?}",
        report.problems
    );
}

#[test]
fn the_object_location_is_remembered_across_opens() {
    let (_tmp, root) = setup_db();
    assert_eq!(
        Repo::open(&root).unwrap().object_location(),
        crate::ObjectLocation::Database
    );
    assert_eq!(
        Repo::open_and_migrate(&root).unwrap().object_location(),
        crate::ObjectLocation::Database
    );
    let (_tmp2, files) = setup();
    assert_eq!(
        Repo::open(&files).unwrap().object_location(),
        crate::ObjectLocation::Files
    );
    // `init` is `init_with` the defaults.
    let tmp3 = TempDir::new().unwrap();
    assert_eq!(
        Repo::init(tmp3.path()).unwrap().object_location(),
        crate::ObjectLocation::Files
    );
}

#[test]
fn a_repository_without_a_settings_row_keeps_its_objects_in_files() {
    let (_tmp, root) = setup();
    write(&root, "f.txt", "x\n");
    save(&root, "s1");
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db")).unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM settings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);

    // A repository written before the tables existed has neither of them.
    conn.execute_batch("DROP TABLE settings; DROP TABLE objects;")
        .unwrap();
    drop(conn);
    assert_eq!(
        Repo::open(&root).unwrap().object_location(),
        crate::ObjectLocation::Files
    );
    // Migrating again is idempotent and does not change the answer.
    let migrated = Repo::open_and_migrate(&root).unwrap();
    assert_eq!(migrated.object_location(), crate::ObjectLocation::Files);
    drop(migrated);
    let migrated = Repo::open_and_migrate(&root).unwrap();
    assert_eq!(migrated.object_location(), crate::ObjectLocation::Files);
    assert!(commands::fsck::check(&migrated).unwrap().is_healthy());
}

// =========================================================================
// Single-file repositories (docs/FORMAT.md 1, "Single-file layout")
// =========================================================================

fn file_save(
    repo: &Repo,
    branch: &str,
    parent: Option<&SnapshotId>,
    files: &[(&str, &str)],
) -> SnapshotId {
    use crate::tree::{SaveTree, TreeEntry};
    let branch = branch_name(branch);
    let guard = repo.write().unwrap();
    guard
        .save_tree(SaveTree {
            branch: &branch,
            parent,
            merge_parent: None,
            message: "m",
            entries: files
                .iter()
                .map(|(p, c)| TreeEntry::file(*p, c.as_bytes().to_vec()))
                .collect(),
            meta: SnapshotMeta::new(),
            author: None,
            timestamp_ms: Some(1_000),
            renames: &[],
        })
        .unwrap()
}

#[test]
fn single_file_repository_runs_the_store_only_api() {
    use commands::merge::{self, MergeCommit, Resolution};
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("notes.velo");
    let repo = Repo::create_file(&file).unwrap();
    assert!(!repo.has_working_tree());
    assert_eq!(repo.object_location(), crate::ObjectLocation::Database);
    assert!(matches!(
        Repo::create_file(&file).unwrap_err(),
        Error::AlreadyInitialized { .. }
    ));

    let m1 = file_save(&repo, "main", None, &[("a.txt", "one\n")]);
    let m2 = file_save(&repo, "main", Some(&m1), &[("a.txt", "one\ntwo\n")]);
    let d1 = file_save(
        &repo,
        "draft",
        Some(&m1),
        &[("a.txt", "one\n"), ("b.txt", "b\n")],
    );

    // History from an id, and the default (the `main` branch).
    let from = commands::history::run(
        &repo,
        commands::history::Options {
            from: Some(&d1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(from.entries.len(), 2);
    let dflt = commands::history::run(&repo, Default::default()).unwrap();
    assert_eq!(dflt.entries[0].hash, m2);
    assert!(dflt.current.is_none());

    // Blame defaults to the tip of `main`.
    let blame = commands::blame::run(&repo, Path::new("a.txt"), Default::default()).unwrap();
    assert_eq!(blame.snapshot, m2);
    assert_eq!(blame.lines.len(), 2);

    // Merge plan and commit.
    let plan = merge::plan(&repo, &m2, &d1).unwrap();
    assert_eq!(plan.conflicts().count(), 0);
    let merge_branch = branch_name("main");
    let merged = {
        let guard = repo.write().unwrap();
        merge::commit(
            &guard,
            MergeCommit {
                branch: &merge_branch,
                ours: &m2,
                theirs: &d1,
                resolutions: &[] as &[(String, Resolution)],
                message: "merge",
                meta: SnapshotMeta::new(),
                author: None,
                timestamp_ms: Some(10),
            },
        )
        .unwrap()
    };
    assert_eq!(repo.read_file_at(&merged, "b.txt").unwrap(), b"b\n");

    // With no position, a tag must name its snapshot.
    {
        let guard = repo.write().unwrap();
        let tag: TagName = "v1".parse().unwrap();
        assert!(commands::tag::create(&guard, &tag, None, false).is_err());
        commands::tag::create(&guard, &tag, Some(&m1), false).unwrap();
    }
    assert_eq!(commands::branches::list(&repo).unwrap().len(), 2);

    assert!(commands::fsck::check(&repo).unwrap().is_healthy());
    {
        let guard = repo.write().unwrap();
        commands::gc::run(&guard, Default::default()).unwrap();
    }
    assert!(commands::fsck::check(&repo).unwrap().is_healthy());

    // Reopen: contents intact; only the database, its WAL siblings and the
    // lock file exist, and no `.velo`.
    drop(repo);
    let repo = Repo::open_file(&file).unwrap();
    assert_eq!(repo.read_file_at(&merged, "a.txt").unwrap(), b"one\ntwo\n");
    let mut names: Vec<String> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for n in &names {
        assert!(
            n == "notes.velo"
                || n == "notes.velo.lock"
                || n == "notes.velo-wal"
                || n == "notes.velo-shm",
            "unexpected file {n}"
        );
    }
    drop(repo);
    Repo::open_file_and_migrate(&file).unwrap();
}

#[test]
fn single_file_repository_ignores_a_neighbouring_velo_directory() {
    let tmp = TempDir::new().unwrap();
    // A directory repository in the same folder, on another branch, with a
    // checked-out snapshot of its own.
    let dir_root = tmp.path().to_path_buf();
    commands::init::run(&dir_root).unwrap();
    write(&dir_root, "x.txt", "x\n");
    let dir_snap = save(&dir_root, "dir snapshot");
    fs::write(dir_root.join(".velo/HEAD"), "elsewhere").unwrap();
    assert_eq!(
        fs::read_to_string(dir_root.join(".velo/PARENT")).unwrap(),
        dir_snap
    );

    let file = tmp.path().join("single.velo");
    let repo = Repo::create_file(&file).unwrap();
    let m1 = file_save(&repo, "main", None, &[("a.txt", "a\n")]);
    let history = commands::history::run(&repo, Default::default()).unwrap();
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].hash, m1);
    assert!(history.current.is_none());
    let blame = commands::blame::run(&repo, Path::new("a.txt"), Default::default()).unwrap();
    assert_eq!(blame.snapshot, m1);
    let branches = commands::branches::list(&repo).unwrap();
    assert!(!branches.iter().any(|b| b.name.as_str() == "elsewhere"));
    {
        let guard = repo.write().unwrap();
        let tag: TagName = "t".parse().unwrap();
        assert!(commands::tag::create(&guard, &tag, None, false).is_err());
    }
    assert!(commands::fsck::check(&repo).unwrap().is_healthy());
}

#[test]
fn single_file_repository_refuses_working_tree_commands() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::create_file(&tmp.path().join("r.velo")).unwrap();
    let m1 = file_save(&repo, "main", None, &[("a.txt", "a\n")]);
    fn refused<T: std::fmt::Debug>(r: crate::error::Result<T>, what: &str) {
        match r.unwrap_err() {
            Error::Unsupported { detail } => {
                assert!(detail.contains(what), "{detail}");
                assert!(detail.contains("needs a working tree"), "{detail}");
            }
            other => panic!("{what}: {other:?}"),
        }
    }
    {
        let guard = repo.write().unwrap();
        refused(
            commands::restore::run(&guard, &m1, Default::default()),
            "restore",
        );
        refused(commands::switch::run(&guard, "main", false), "switch");
        refused(
            commands::save::run(&guard, Some("m"), Default::default()),
            "save",
        );
        refused(
            commands::merge::run(&guard, commands::merge::Mode::Bring { source: "main" }),
            "merge",
        );
        refused(commands::squash::run(&guard, 1, "s"), "squash");
        refused(commands::stash::push(&guard, None), "stash");
        refused(commands::undo::run(&guard), "undo");
        refused(commands::redo::run(&guard), "redo");
        refused(
            commands::mv::run(&guard, Path::new("a"), Path::new("b")),
            "mv",
        );
    }
    refused(commands::status::run(&repo, &[]), "status");
    refused(commands::diff::run(&repo, &None), "diff");
    refused(commands::diff::between(&repo, &m1, None, &[]), "diff");
    commands::diff::between(&repo, &m1, Some(&m1), &[]).unwrap();
    refused(commands::grep::run(&repo, "a", Default::default()), "grep");
}

#[test]
fn single_file_repositories_contend_on_a_sibling_lock() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("r.velo");
    let a = Repo::create_file(&file).unwrap();
    let b = Repo::open_file(&file).unwrap();
    let held = a.write().unwrap();
    assert!(matches!(b.write().unwrap_err(), Error::Locked { .. }));
    assert!(tmp.path().join("r.velo.lock").is_file());
    assert!(!tmp.path().join(".velo").exists());
    drop(held);
    b.write().unwrap();
}

#[test]
fn open_file_refuses_a_directory_repositorys_database() {
    let (_tmp, root) = setup();
    let err = Repo::open_file(&root.join(".velo/velo.db")).unwrap_err();
    assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");
    let err = Repo::open_file_and_migrate(&root.join(".velo/velo.db")).unwrap_err();
    assert!(matches!(err, Error::InvalidInput { .. }), "{err:?}");
}

#[cfg(feature = "bundle")]
#[test]
fn a_single_file_repository_exchanges_bundles_with_a_directory_one() {
    let tmp = TempDir::new().unwrap();
    let single = Repo::create_file(&tmp.path().join("s.velo")).unwrap();
    let m1 = file_save(&single, "main", None, &[("a.txt", "a\n")]);
    let m2 = file_save(&single, "main", Some(&m1), &[("a.txt", "a\nb\n")]);
    let bundle = tmp.path().join("out.velobundle");
    commands::bundle::create(&single, &bundle, None).unwrap();

    let (_tmp2, dir_root) = setup();
    with_write(&dir_root, |g| commands::bundle::apply(g, &bundle)).unwrap();
    with_repo(&dir_root, |r| {
        assert_eq!(r.read_file_at(&m2, "a.txt").unwrap(), b"a\nb\n");
    });

    // And back: a directory repository's history into a fresh single file.
    let back = tmp.path().join("back.velobundle");
    with_repo(&dir_root, |r| commands::bundle::create(r, &back, None)).unwrap();
    let other = Repo::create_file(&tmp.path().join("o.velo")).unwrap();
    {
        let guard = other.write().unwrap();
        commands::bundle::apply(&guard, &back).unwrap();
    }
    assert_eq!(other.read_file_at(&m2, "a.txt").unwrap(), b"a\nb\n");
    assert!(commands::fsck::check(&other).unwrap().is_healthy());
}
