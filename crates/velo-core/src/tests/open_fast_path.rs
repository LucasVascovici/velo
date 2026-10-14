//! `db::migrate` skips the schema script only when it would change nothing.

use tempfile::TempDir;

use crate::{db, Repo};

fn setup() -> TempDir {
    let tmp = TempDir::new().unwrap();
    crate::commands::init::run(tmp.path()).unwrap();
    tmp
}

fn table_exists(path: &std::path::Path, name: &str) -> bool {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name = ?",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        > 0
}

#[test]
fn opening_a_v3_repo_missing_an_additive_table_creates_it() {
    let tmp = setup();
    let path = tmp.path().join(".velo").join("velo.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE compactions;").unwrap();
        assert_eq!(db::format_version(&conn).unwrap(), crate::FORMAT_VERSION);
    }
    assert!(!table_exists(&path, "compactions"));
    Repo::open_and_migrate(tmp.path()).unwrap();
    assert!(table_exists(&path, "compactions"));
}

#[test]
fn opening_a_v3_repo_missing_an_index_creates_it() {
    let tmp = setup();
    let path = tmp.path().join(".velo").join("velo.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP INDEX idx_meta_snap;").unwrap();
    }
    Repo::open_and_migrate(tmp.path()).unwrap();
    assert!(table_exists(&path, "idx_meta_snap"));
}

#[test]
fn reopening_a_current_repo_is_a_no_op() {
    let tmp = setup();
    Repo::open_and_migrate(tmp.path()).unwrap();
    let repo = Repo::open_and_migrate(tmp.path()).unwrap();
    drop(repo);
    let path = tmp.path().join(".velo").join("velo.db");
    assert!(table_exists(&path, "compactions"));
}
