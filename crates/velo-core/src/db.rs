use rusqlite::{Connection, Result};
use std::path::Path;

/// The complete v2 schema.
///
/// One authoritative definition rather than an initial schema plus a chain of
/// `ALTER TABLE` migrations. Those existed only to bring v1 repositories forward,
/// and v2 refuses to open one (see [`migrate`]), so they would be dead code that
/// still had to be kept true.
///
/// Every statement is idempotent, so this doubles as the migration for a v2
/// repository written by an earlier build of v2 itself.
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS snapshots (
        hash          TEXT PRIMARY KEY,
        message       TEXT NOT NULL,
        branch        TEXT NOT NULL,
        parent_hash   TEXT NOT NULL DEFAULT '',
        merge_parent  TEXT NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS file_map (
        snapshot_hash TEXT NOT NULL,
        path          TEXT NOT NULL,
        hash          TEXT NOT NULL,
        mode          INTEGER NOT NULL DEFAULT 0
    );
    -- App-namespaced snapshot metadata. Part of snapshot identity, so rows are
    -- immutable: changing one would change the id it belongs to.
    CREATE TABLE IF NOT EXISTS snapshot_meta (
        snapshot_id TEXT NOT NULL,
        namespace   TEXT NOT NULL,
        key         TEXT NOT NULL,
        value       TEXT NOT NULL,
        PRIMARY KEY (snapshot_id, namespace, key)
    );
    CREATE TABLE IF NOT EXISTS tags (
        name          TEXT PRIMARY KEY,
        snapshot_hash TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS trash (
        hash          TEXT PRIMARY KEY,
        message       TEXT NOT NULL,
        branch        TEXT NOT NULL,
        parent_hash   TEXT NOT NULL DEFAULT '',
        merge_parent  TEXT NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL DEFAULT 0,
        deleted_at_ms INTEGER NOT NULL DEFAULT 0
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
        created_at_ms INTEGER NOT NULL DEFAULT 0
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

    -- Branches as first-class refs, so a branch created but not yet committed to
    -- still exists. `tip = ''` means the branch exists but is unborn.
    CREATE TABLE IF NOT EXISTS branches (
        name TEXT PRIMARY KEY,
        tip  TEXT NOT NULL DEFAULT ''
    );

    CREATE TABLE IF NOT EXISTS remotes (
        name TEXT PRIMARY KEY,
        url  TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS remote_refs (
        remote TEXT NOT NULL,
        branch TEXT NOT NULL,
        hash   TEXT NOT NULL,
        PRIMARY KEY (remote, branch)
    );

    CREATE INDEX IF NOT EXISTS idx_filemap_snap  ON file_map (snapshot_hash);
    CREATE INDEX IF NOT EXISTS idx_filemap_path  ON file_map (path);
    CREATE INDEX IF NOT EXISTS idx_snap_branch   ON snapshots (branch, created_at_ms);
    CREATE INDEX IF NOT EXISTS idx_trash_branch  ON trash (branch, deleted_at_ms);
    CREATE INDEX IF NOT EXISTS idx_stash_name    ON stash (name);
    CREATE INDEX IF NOT EXISTS idx_meta_snap     ON snapshot_meta (snapshot_id);
";

/// Create a repository database, stamped with the format version that wrote it.
///
/// Stamping here matters: without it a freshly created repository sits at
/// `user_version = 0`, and `0` means "v1, unversioned" — so a brand-new repo
/// would be indistinguishable from one this build must refuse.
pub fn init_db_at_path(path: &Path) -> Result<()> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    conn.execute_batch(SCHEMA)?;
    set_format_version(&conn, crate::FORMAT_VERSION)?;
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
/// `0` means "written before versioning existed", which is a **v1** repository —
/// v1 wrote no marker at all. See [`is_pre_v2`].
pub fn format_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(v as u32)
}

/// Whether `version` denotes a repository written before format v2.
///
/// `0` (unversioned) and `1` both mean v1. Such a repository cannot be upgraded
/// in place: v2 changes every snapshot id, so its rows are not v2 rows with a
/// stale marker — they are a different format. See `docs/FORMAT.md`.
pub fn is_pre_v2(version: u32) -> bool {
    version < 2
}

fn set_format_version(conn: &Connection, version: u32) -> Result<()> {
    // PRAGMA does not accept bound parameters.
    conn.execute_batch(&format!("PRAGMA user_version = {};", version))
}

/// Bring the schema up to the current format version. Idempotent.
///
/// This applies the v2 schema and stamps the version. It deliberately cannot
/// upgrade a v1 repository — the caller must reject those first (see
/// [`is_pre_v2`] and `Repo::open_and_migrate`), because every id would need
/// recomputing and silently stamping v2 over v1 rows would corrupt the
/// repository in a way `fsck` could only report after the fact.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
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
