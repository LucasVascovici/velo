use rusqlite::{Connection, Result};
use std::path::Path;

/// Open (or create) the database at `path` and apply the full schema.
/// WAL mode + large page cache + mmap are set on every connection.
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

        CREATE TABLE IF NOT EXISTS trash (
            hash        TEXT PRIMARY KEY,
            message     TEXT NOT NULL,
            branch      TEXT NOT NULL,
            parent_hash TEXT NOT NULL DEFAULT '',
            created_at  DATETIME,
            deleted_at  DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        -- Mtime+size content-hash cache.
        -- If mtime_ns AND size match the stored entry the file content has not
        -- changed, so we can skip the disk read entirely (same logic as git index).
        CREATE TABLE IF NOT EXISTS index_cache (
            path     TEXT PRIMARY KEY,
            mtime_ns INTEGER NOT NULL,
            size     INTEGER NOT NULL,
            hash     TEXT    NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_filemap_snap  ON file_map (snapshot_hash);
        CREATE INDEX IF NOT EXISTS idx_filemap_path  ON file_map (path);
        CREATE INDEX IF NOT EXISTS idx_snap_branch   ON snapshots (branch, created_at);
        CREATE INDEX IF NOT EXISTS idx_trash_branch  ON trash (branch, deleted_at);"
    )?;
    Ok(())
}

/// Open an existing database with all performance pragmas applied.
pub fn get_conn_at_path(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous",  "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // 64 MB page cache (negative = kibibytes)
    conn.pragma_update(None, "cache_size",   -65_536_i64)?;
    // 256 MB memory-mapped I/O
    conn.pragma_update(None, "mmap_size",    268_435_456_i64)?;
    // Temp tables in RAM
    conn.pragma_update(None, "temp_store",   "MEMORY")?;
    Ok(())
}

#[inline]
pub fn normalise(rel: &str) -> String {
    rel.replace('\\', "/")
}

#[inline]
pub fn db_to_path(db_path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(db_path)
}