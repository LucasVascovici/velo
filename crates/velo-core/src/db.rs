use rusqlite::{Connection, Result};
use std::path::Path;

pub fn init_db_at_path(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots (
            hash         TEXT PRIMARY KEY,
            message      TEXT NOT NULL,
            branch       TEXT NOT NULL,
            parent_hash  TEXT NOT NULL DEFAULT '',
            merge_parent TEXT NOT NULL DEFAULT '',
            created_at   DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS file_map (
            snapshot_hash TEXT NOT NULL,
            path          TEXT NOT NULL,
            hash          TEXT NOT NULL,
            mode          INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS tags (
            name          TEXT PRIMARY KEY,
            snapshot_hash TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS trash (
            hash         TEXT PRIMARY KEY,
            message      TEXT NOT NULL,
            branch       TEXT NOT NULL,
            parent_hash  TEXT NOT NULL DEFAULT '',
            merge_parent TEXT NOT NULL DEFAULT '',
            created_at   DATETIME,
            deleted_at   DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        -- Tags removed by `undo`, held so `redo` can restore them.
        CREATE TABLE IF NOT EXISTS trash_tags (
            name          TEXT PRIMARY KEY,
            snapshot_hash TEXT NOT NULL
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
        CREATE INDEX IF NOT EXISTS idx_stash_name    ON stash (name);",
    )?;
    Ok(())
}

/// Open a connection and apply pragmas, **without** migrating.
///
/// Callers decide when to migrate (see [`migrate`]) so that opening a repository
/// can never silently rewrite it.
pub fn connect(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

/// The repository format version recorded in `PRAGMA user_version`.
///
/// `0` means "written before versioning existed" — schema-sniffing migrations
/// bring such a repository up to date and stamp the version.
pub fn format_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(v as u32)
}

fn set_format_version(conn: &Connection, version: u32) -> Result<()> {
    // PRAGMA does not accept bound parameters.
    conn.execute_batch(&format!("PRAGMA user_version = {};", version))
}

/// Bring the schema up to the current format version. Idempotent.
pub fn migrate(conn: &Connection) -> Result<()> {
    apply_migrations(conn)?;
    set_format_version(conn, crate::FORMAT_VERSION)?;
    Ok(())
}

/// Open and migrate in one step. Retained for call sites that predate the
/// [`crate::Repo`] handle; new code should go through `Repo`.
pub fn get_conn_at_path(path: &Path) -> Result<Connection> {
    let conn = connect(path)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Idempotent schema migrations — safe to run on every connection open.
fn apply_migrations(conn: &Connection) -> Result<()> {
    // Migration 1: add merge_parent to snapshots (2.3+)
    let has_col: bool = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('snapshots') WHERE name = 'merge_parent'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_col {
        conn.execute_batch(
            "ALTER TABLE snapshots ADD COLUMN merge_parent TEXT NOT NULL DEFAULT '';",
        )?;
    }

    // Migration 2: add merge_parent to trash (2.4+) so undo→redo of a merge
    // commit preserves its second parent instead of silently dropping it.
    let trash_has_mp: bool = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('trash') WHERE name = 'merge_parent'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !trash_has_mp {
        conn.execute_batch("ALTER TABLE trash ADD COLUMN merge_parent TEXT NOT NULL DEFAULT '';")?;
    }

    // Migration 3: trash_tags table (2.4+) so undo→redo restores tags.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS trash_tags (
            name          TEXT PRIMARY KEY,
            snapshot_hash TEXT NOT NULL
        );",
    )?;

    // Migration 4: add mode to file_map (0=regular, 1=executable, 2=symlink)
    // so the file model tracks the executable bit and symlinks.
    let fm_has_mode: bool = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('file_map') WHERE name = 'mode'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !fm_has_mode {
        conn.execute_batch("ALTER TABLE file_map ADD COLUMN mode INTEGER NOT NULL DEFAULT 0;")?;
    }

    // Migration 6: branches as first-class refs.
    //
    // Branch tips used to be derived purely from `snapshots.branch`, so a branch
    // you had created but not yet committed to simply didn't exist — `velo init`
    // followed by `velo switch other` left `main` unresolvable, and commands that
    // look up a branch tip (merge, push, …) failed with "has no snapshots" even
    // though PARENT pointed at a real commit. This table records where a branch
    // points independently of whether anything has been committed on it yet.
    // Existing repos are backfilled from their snapshots below.
    let had_branches: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='branches'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS branches (
            name TEXT PRIMARY KEY,
            tip  TEXT NOT NULL DEFAULT ''
        );",
    )?;
    if !had_branches {
        conn.execute_batch(
            "INSERT OR IGNORE INTO branches (name, tip)
             SELECT branch, '' FROM snapshots
             WHERE branch <> '_stash' AND branch NOT LIKE 'remotes/%';",
        )?;
    }

    // Migration 5: remotes + remote-tracking refs (v3 / sync).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS remotes (
            name TEXT PRIMARY KEY,
            url  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS remote_refs (
            remote TEXT NOT NULL,
            branch TEXT NOT NULL,
            hash   TEXT NOT NULL,
            PRIMARY KEY (remote, branch)
        );",
    )?;

    Ok(())
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size", -65_536_i64)?;
    conn.pragma_update(None, "mmap_size", 268_435_456_i64)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
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
