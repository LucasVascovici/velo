use rusqlite::{Connection, Result};
use std::path::Path;

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
        CREATE TABLE IF NOT EXISTS index_cache (
            path     TEXT PRIMARY KEY,
            mtime_ns INTEGER NOT NULL,
            size     INTEGER NOT NULL,
            hash     TEXT    NOT NULL
        );
        -- Named stash shelves.
        -- Each shelf stores the dirty state of the working tree at stash time.
        -- The snapshot_hash references a regular snapshot row (branch = '_stash').
        CREATE TABLE IF NOT EXISTS stash (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            name          TEXT NOT NULL UNIQUE,
            snapshot_hash TEXT NOT NULL,
            branch        TEXT NOT NULL,
            parent_hash   TEXT NOT NULL DEFAULT '',
            created_at    DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        -- Files with active merge conflicts.
        -- The three object hashes are all we need to recompute hunks on demand.
        CREATE TABLE IF NOT EXISTS conflict_files (
            path          TEXT PRIMARY KEY,
            ancestor_hash TEXT NOT NULL,
            our_hash      TEXT NOT NULL,
            their_hash    TEXT NOT NULL
        );

        -- Per-hunk resolution decisions.
        -- decision: 'ours' | 'theirs' | 'both_ours' | 'both_theirs' | 'manual'
        -- manual_content: newline-delimited lines (only set when decision='manual')
        CREATE TABLE IF NOT EXISTS hunk_decisions (
            file_path      TEXT    NOT NULL,
            hunk_id        INTEGER NOT NULL,
            decision       TEXT    NOT NULL,
            manual_content TEXT,
            PRIMARY KEY (file_path, hunk_id)
        );

        CREATE INDEX IF NOT EXISTS idx_filemap_snap  ON file_map (snapshot_hash);
        CREATE INDEX IF NOT EXISTS idx_filemap_path  ON file_map (path);
        CREATE INDEX IF NOT EXISTS idx_snap_branch   ON snapshots (branch, created_at);
        CREATE INDEX IF NOT EXISTS idx_trash_branch  ON trash (branch, deleted_at);
        CREATE INDEX IF NOT EXISTS idx_stash_name    ON stash (name);"
    )?;
    Ok(())
}

pub fn get_conn_at_path(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous",  "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size",   -65_536_i64)?;
    conn.pragma_update(None, "mmap_size",    268_435_456_i64)?;
    conn.pragma_update(None, "temp_store",   "MEMORY")?;
    Ok(())
}

#[inline]
pub fn normalise(rel: &str) -> String { rel.replace('\\', "/") }

#[inline]
pub fn db_to_path(db_path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(db_path)
}