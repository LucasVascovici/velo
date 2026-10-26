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
    -- v4: a snapshot stored as a delta against its first parent. No row here
    -- means keyframe, so every pre-v4 `file_map` snapshot is already one and
    -- migration rewrites nothing (docs/FORMAT.md, section 11.6).
    CREATE TABLE IF NOT EXISTS tree_deltas (
        snapshot_hash TEXT PRIMARY KEY,
        base_hash     TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS tree_delta_rows (
        snapshot_hash TEXT NOT NULL,
        path          TEXT NOT NULL,
        hash          TEXT,
        mode          INTEGER,
        PRIMARY KEY (snapshot_hash, path)
    ) WITHOUT ROWID;
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

    -- Paths a snapshot moved.  A rename cannot be recovered from `file_map`,
    -- which holds whole trees: there a move is a delete and an add, and telling
    -- those apart afterwards is guesswork.  So it is recorded when it happens,
    -- by the caller who knows it happened.
    --
    -- Not part of a snapshot's identity, deliberately.  Identity answers what
    -- a tree is, and two identical trees are the same snapshot however each was
    -- arrived at.  `fsck` checks the edges structurally instead.
    CREATE TABLE IF NOT EXISTS renames (
        snapshot_hash TEXT NOT NULL,
        from_path     TEXT NOT NULL,
        to_path       TEXT NOT NULL,
        PRIMARY KEY (snapshot_hash, to_path)
    );

    -- Moves made in the working tree, waiting for the save that will record
    -- them.  Velody has no staging area because the disk is the snapshot; a
    -- rename is the one fact the disk cannot hold, since after the move the
    -- tree is identical to a delete plus an add.  Keyed by destination: two
    -- files cannot arrive at one path.
    CREATE TABLE IF NOT EXISTS pending_renames (
        from_path TEXT NOT NULL,
        to_path   TEXT PRIMARY KEY
    );

    -- Ids that compaction removed or re-minted, mapped to what replaced them
    -- (docs/FORMAT.md 11.3).  Local only: not part of identity, not in bundles,
    -- and never collected by `gc`, since the whole point is to outlive the rows
    -- it describes.
    CREATE TABLE IF NOT EXISTS compactions (
        old_hash        TEXT PRIMARY KEY,
        new_hash        TEXT NOT NULL,
        compacted_at_ms INTEGER NOT NULL
    );

    -- Signatures over snapshot ids (docs/FORMAT.md 11.1).  Outside identity: a
    -- signature never changes a snapshot's id, and a countersignature is just
    -- another row.  Keys and signatures are lowercase hex.
    CREATE TABLE IF NOT EXISTS signatures (
        snapshot_hash TEXT NOT NULL,
        algorithm     TEXT NOT NULL,
        public_key    TEXT NOT NULL,
        signature     TEXT NOT NULL,
        PRIMARY KEY (snapshot_hash, public_key)
    );

    -- Repository settings. `objects` = 'database' puts object content in the
    -- `objects` table below; no row means files under .velo/objects.
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    -- Object content when the repository's object location is the database:
    -- one zstd frame of the full content per object (docs/FORMAT.md 7.2).
    CREATE TABLE IF NOT EXISTS objects (
        hash TEXT PRIMARY KEY,
        data BLOB NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_filemap_snap  ON file_map (snapshot_hash);
    CREATE INDEX IF NOT EXISTS idx_filemap_path  ON file_map (path);
    CREATE INDEX IF NOT EXISTS idx_tree_deltas_base ON tree_deltas (base_hash);
    CREATE INDEX IF NOT EXISTS idx_delta_rows_path  ON tree_delta_rows (path);
    CREATE INDEX IF NOT EXISTS idx_snap_branch   ON snapshots (branch, created_at_ms);
    CREATE INDEX IF NOT EXISTS idx_trash_branch  ON trash (branch, deleted_at_ms);
    CREATE INDEX IF NOT EXISTS idx_stash_name    ON stash (name);
    CREATE INDEX IF NOT EXISTS idx_meta_snap     ON snapshot_meta (snapshot_id);
    CREATE INDEX IF NOT EXISTS idx_meta_lookup   ON snapshot_meta (namespace, key, value);
    -- The backward walk asks 'which snapshot renamed something to this path',
    -- so `to_path` leads.  The primary key already covers the per-snapshot
    -- lookup that follows.
    CREATE INDEX IF NOT EXISTS idx_renames_to     ON renames (to_path);
";

/// Create a repository database, stamped with the format version that wrote it.
///
/// Stamping here matters: without it a freshly created repository sits at
/// `user_version = 0`, and `0` means "v1, unversioned" — so a brand-new repo
/// would be indistinguishable from one this build must refuse.
pub fn init_db_at_path(path: &Path) -> Result<()> {
    create_at_path(path).map(|_| ())
}

/// Create the database file, schema and version marker, and hand back the open
/// connection so `init` need not reopen and re-run the schema to register
/// `main`.
pub fn create_at_path(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // `auto_vacuum` only takes effect before the first table exists, so it is
    // set ahead of the pragmas and the schema. INCREMENTAL lets retention and
    // `gc` hand freed pages back cheaply (`gc::vacuum`). Opening an existing
    // repository never changes it: only a full `VACUUM` converts an old one.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    apply_pragmas(&conn)?;
    conn.execute_batch(SCHEMA)?;
    set_format_version(&conn, crate::FORMAT_VERSION)?;
    Ok(conn)
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

/// Open an **existing** database file, never creating one, and apply pragmas.
///
/// Returns `Ok(None)` when there is no such file. Existence is asked of SQLite,
/// not `std::fs`: on wasm32 a single-file repository lives in a SQLite VFS
/// (in memory, or the origin-private file system) that `std::fs` cannot see,
/// and every `std::fs` query there fails.
pub fn connect_existing(path: &Path) -> Result<Option<Connection>> {
    use rusqlite::{ffi::ErrorCode, OpenFlags};
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    match Connection::open_with_flags(path, flags) {
        Ok(conn) => {
            apply_pragmas(&conn)?;
            Ok(Some(conn))
        }
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::CannotOpen => Ok(None),
        Err(e) => Err(e),
    }
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
    if schema_is_current(conn)? {
        return Ok(());
    }
    // One transaction, so a kill between the schema script and the version
    // stamp rolls both back and leaves the repository as it was (v3 stays v3).
    // `user_version` lives in the database header and is transactional.
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SCHEMA)?;
    crate::failpoint::hit("migrate.before_stamp");
    set_format_version(&tx, crate::FORMAT_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// Whether the version marker is current **and** every table and index the
/// `SCHEMA` script declares exists, so re-running it would change nothing.
///
/// The version alone is not enough: a repository written by an older v3 build
/// carries `user_version = 3` yet lacks tables added since (`compactions`),
/// and those must still appear on open. The expected count is derived from
/// `SCHEMA` itself so it cannot drift from the script.
fn schema_is_current(conn: &Connection) -> Result<bool> {
    if format_version(conn)? != crate::FORMAT_VERSION {
        return Ok(false);
    }
    let expected = SCHEMA.matches("CREATE TABLE IF NOT EXISTS").count()
        + SCHEMA.matches("CREATE INDEX IF NOT EXISTS").count();
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master          WHERE type IN ('table','index') AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    Ok(present as usize == expected)
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
    conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
    Ok(())
}

/// After a checkpoint SQLite resets the WAL and keeps the file at most this
/// large, instead of at the high-water mark of the busiest transaction. It is
/// a per-connection setting, not stored in the file, so it is not a format
/// change.
pub(crate) const WAL_SIZE_LIMIT_BYTES: i64 = 1 << 20;

/// A WAL larger than this after a commit is checkpointed and truncated. The
/// default autocheckpoint copies pages back but leaves the file at its
/// high-water mark; on a handle held open for a whole agent run that is dead
/// disk an embedder sees.
pub(crate) const WAL_TRUNCATE_BYTES: u64 = 2 << 20;

/// Truncate the WAL of a long-lived handle once it has grown past
/// [`WAL_TRUNCATE_BYTES`]. Call it after a commit, never inside a transaction.
///
/// It is best effort: a reader holding the WAL makes the checkpoint report
/// busy and nothing more happens; any other failure is ignored too, because
/// the commit it follows is already durable and a WAL that is not truncated is
/// merely larger, never wrong. A crash on either side of the checkpoint leaves
/// a WAL that SQLite replays.
pub(crate) fn bound_wal(conn: &Connection, db_path: &Path) {
    if db_path.as_os_str().is_empty() {
        return;
    }
    let mut wal = db_path.as_os_str().to_owned();
    wal.push("-wal");
    let big = std::fs::metadata(std::path::PathBuf::from(wal))
        .map(|m| m.len() > WAL_TRUNCATE_BYTES)
        .unwrap_or(false);
    if !big {
        return;
    }
    // A TRUNCATE checkpoint waits on the busy handler for readers to finish,
    // which would stall every save behind a long read. Give up at once instead
    // and restore rusqlite's default five seconds afterwards.
    let _ = conn.busy_timeout(std::time::Duration::ZERO);
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        r.get::<_, i64>(0)
    });
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
}

#[inline]
pub fn normalise(rel: &str) -> String {
    rel.replace('\\', "/")
}

#[inline]
pub fn db_to_path(db_path: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(db_path)
}
