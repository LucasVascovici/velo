use rusqlite::{Connection, Result};
use std::path::Path;

/// Open (or create) a database at `path` and run the full schema migration.
/// WAL mode is enabled on every connection for better concurrency and durability.
pub fn init_db_at_path(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots (
            hash        TEXT PRIMARY KEY,
            message     TEXT NOT NULL,
            branch      TEXT NOT NULL,
            parent_hash TEXT NOT NULL DEFAULT '',
            created_at  DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS file_map (
            snapshot_hash TEXT NOT NULL,
            path          TEXT NOT NULL,
            hash          TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS tags (
            name          TEXT PRIMARY KEY,
            snapshot_hash TEXT NOT NULL
        );

        -- Soft-deleted snapshots kept for `velo redo`.
        -- file_map entries are intentionally preserved (not deleted on undo)
        -- so that redo can restore them without touching the object store.
        CREATE TABLE IF NOT EXISTS trash (
            hash        TEXT PRIMARY KEY,
            message     TEXT NOT NULL,
            branch      TEXT NOT NULL,
            parent_hash TEXT NOT NULL DEFAULT '',
            created_at  DATETIME,
            deleted_at  DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_filemap_snap  ON file_map (snapshot_hash);
        CREATE INDEX IF NOT EXISTS idx_filemap_path  ON file_map (path);
        CREATE INDEX IF NOT EXISTS idx_snap_branch   ON snapshots (branch, created_at);
        CREATE INDEX IF NOT EXISTS idx_trash_branch  ON trash (branch, deleted_at);",
    )?;
    Ok(())
}

/// Open an existing database and apply WAL mode.
pub fn get_conn_at_path(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", "10000")?;
    Ok(())
}

/// Normalise a relative path to forward-slash notation for cross-platform
/// storage in SQLite.  On Linux/macOS this is a no-op.
pub fn normalise(rel: &str) -> String {
    rel.replace('\\', "/")
}

/// Convert a DB-stored (forward-slash) path back to a native `Path`.
/// `std::path::Path` accepts `/` on all platforms, so no conversion needed.
pub fn db_to_path(db_path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(db_path)
}