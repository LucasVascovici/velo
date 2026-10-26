//! `velody gc` — reclaim space and prune bookkeeping that nothing points at.
//!
//! Returns a tally of what was collected; wording lives in `velody-cli`.

use std::collections::HashSet;

use crate::error::Result;
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::WriteGuard;

/// What a collection pass reclaimed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    /// Trash entries aged past the retention window.
    pub expired_trash: usize,
    /// File-map rows for snapshots that no longer exist.
    pub orphan_file_map: usize,
    /// Hunk decisions for files no longer in conflict.
    pub orphan_decisions: usize,
    /// Shelved tags whose snapshot is gone for good.
    pub orphan_shelved_tags: usize,
    /// Index-cache rows for paths no snapshot tracks.
    pub stale_cache: usize,
    /// Objects nothing references any more.
    pub objects: usize,
    /// Chunks no surviving object's manifest names.
    pub chunks: usize,
    /// Bytes the objects and chunks occupied on disk.
    pub bytes_freed: u64,
    /// The retention window that was applied, in days.
    pub keep_days: u32,
    /// This was a dry run: the counts say what a real run would collect, and
    /// nothing was removed.
    pub dry_run: bool,
}

impl Collected {
    /// Nothing needed collecting.
    pub fn is_empty(&self) -> bool {
        self.expired_trash == 0
            && self.orphan_file_map == 0
            && self.orphan_decisions == 0
            && self.orphan_shelved_tags == 0
            && self.stale_cache == 0
            && self.objects == 0
            && self.chunks == 0
    }
}

/// How to run a collection pass.
///
/// The last of the four commands design item 7.6 named to get
/// per-call progress. `gc` is the longest purely local operation velody has, and
/// it reported through the handle observer only — so a GUI running a collection
/// alongside anything else could not tell the two apart, and could not stop
/// either.
#[derive(Default)]
pub struct Options<'a> {
    /// Trash newer than this survives. Trash whose snapshot is signed survives
    /// regardless, and `gc` never deletes a signature row, so an undone signed
    /// snapshot stays recoverable by `redo`.
    pub keep_days: u32,
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between objects. Cancelling keeps what was already collected —
    /// an earlier stopping point, not a broken repository, because everything
    /// deleted was unreachable before the pass started.
    ///
    /// The database work runs first and is not interruptible: it is a handful of
    /// statements in one transaction, and splitting it would trade a bounded
    /// wait for a half-pruned index.
    pub cancel: Option<&'a Cancel>,
    /// Report what would be collected and remove nothing. The database prunes
    /// run inside a transaction that is rolled back, so the counts match a real
    /// run made straight afterwards.
    pub dry_run: bool,
}

/// Collect garbage.
///
/// ```no_run
/// # fn main() -> Result<(), velody_core::Error> {
/// # let mut repo = velody_core::Repo::discover(std::path::Path::new("."))?;
/// # let guard = repo.write()?;
/// use velody_core::commands::gc;
///
/// let collected = gc::run(&guard, gc::Options {
///     keep_days: 30,
///     ..Default::default()
/// })?;
/// # let _ = collected;
/// # Ok(()) }
/// ```
pub fn run(guard: &WriteGuard, options: Options<'_>) -> Result<Collected> {
    let Options {
        keep_days,
        observer,
        cancel,
        dry_run,
    } = options;
    // One transaction for the prunes. A dry run drops it without committing, so
    // the later queries still see the pruned state but nothing persists.
    let conn = guard.conn().unchecked_transaction()?;
    let mut collected = Collected {
        keep_days,
        dry_run,
        ..Default::default()
    };

    // The cutoff is computed here and bound as an integer: `deleted_at_ms` is an
    // INTEGER column, and comparing it with SQLite's TEXT `datetime()` always
    // holds (an integer sorts below any text), which expired every row.
    let cutoff_ms = guard.repo().now_ms() - i64::from(keep_days) * 86_400_000;
    // Rows a live snapshot still builds on are kept: they are the only copy of
    // a parent that an older `undo` removed, and `velody redo` restores it.
    // A signed snapshot's trash entry is kept too: expiring it would make an
    // undone signed snapshot unrecoverable. The table may not exist yet.
    let keep_signed = if crate::sign::table_exists(&conn)? {
        " AND hash NOT IN (SELECT snapshot_hash FROM signatures)"
    } else {
        ""
    };
    collected.expired_trash = conn.execute(
        &format!(
            "DELETE FROM trash WHERE deleted_at_ms <= ?
               AND hash NOT IN (SELECT parent_hash FROM snapshots
                    UNION SELECT merge_parent FROM snapshots WHERE COALESCE(merge_parent, '') <> '')
               {keep_signed}"
        ),
        [cutoff_ms],
    )?;

    collected.orphan_file_map =
        crate::tree_store::delete_orphan_trees(&conn, guard.repo().keyframe_interval())?;

    collected.orphan_decisions = conn.execute(
        "DELETE FROM hunk_decisions WHERE file_path NOT IN (SELECT path FROM conflict_files)",
        [],
    )?;

    collected.orphan_shelved_tags = conn.execute(
        "DELETE FROM trash_tags
         WHERE snapshot_hash NOT IN (SELECT hash FROM snapshots)
           AND snapshot_hash NOT IN (SELECT hash FROM trash)",
        [],
    )?;

    collected.stale_cache = crate::tree_store::prune_stale_index_cache(&conn)?;

    // Anything the database still points at has to survive. Stored trees are
    // the only thing that references objects, and orphaned ones were pruned
    // above.
    let referenced: HashSet<String> = crate::tree_store::referenced_objects(&conn)?;

    // The last database access. A real run makes the prunes durable before any
    // file is touched, so an I/O error later leaves trash and file_map rows
    // that still match the objects on disk; a dry run rolls them back.
    if dry_run {
        drop(conn);
    } else {
        conn.commit()?;
        crate::failpoint::hit("gc.after_prune");
    }

    // No total: the directory is streamed rather than counted first, so a
    // consumer gets liveness without us walking the whole store twice.
    let progress = PhaseGuard::new(
        observer.unwrap_or_else(|| guard.repo().observer()),
        Phase::Collecting,
        None,
    );
    let objects = guard.repo().objects();
    // Objects a dry run left in place but a real run would have removed.
    let mut doomed: HashSet<String> = HashSet::new();
    for (name, size) in objects.list()? {
        // Checked per object, so cancelling takes effect at the next one rather
        // than part-way through a delete.
        if cancel.is_some_and(Cancel::is_cancelled) {
            break;
        }
        progress.tick();
        if referenced.contains(&name) {
            continue;
        }
        if dry_run {
            doomed.insert(name);
        } else {
            objects.remove(&name)?;
            crate::failpoint::hit("gc.after_object");
        }
        collected.objects += 1;
        collected.bytes_freed += size;
    }

    // Chunks live only for the manifests that name them. Which manifests
    // survive is read from disk after the object pass, not from the database,
    // so a cancelled or partial pass can never strand a chunk a remaining
    // object needs. A chunk shared with a survivor is in the set and stays.
    if !cancel.is_some_and(Cancel::is_cancelled) {
        let mut needed: HashSet<String> = HashSet::new();
        for (name, _) in objects.list()? {
            // A dry run left the garbage objects on disk; a real run would not
            // see them here, so neither may the dry run.
            if doomed.contains(&name) {
                continue;
            }
            if let Some(chunks) = objects.chunks_of(&name)? {
                needed.extend(chunks);
            }
        }
        for (name, size) in objects.list_chunks()? {
            if cancel.is_some_and(Cancel::is_cancelled) {
                break;
            }
            progress.tick();
            if needed.contains(&name) {
                continue;
            }
            if !dry_run {
                objects.remove_chunk(&name)?;
                crate::failpoint::hit("gc.after_chunk");
            }
            collected.chunks += 1;
            collected.bytes_freed += size;
        }
    }

    // A cancelled pass reports `Cancelled` rather than a partial tally, as
    // `restore` does: what it did collect is durable either way, and a count
    // returned as if the pass had finished would be the misleading half.
    Cancel::check(cancel)?;

    Ok(collected)
}

/// How [`vacuum`] returns freed pages to the operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Vacuum {
    /// `PRAGMA incremental_vacuum`: cheap and transactional, but only on a
    /// repository whose `auto_vacuum` is already `INCREMENTAL` (one created
    /// from 5.3.0). On any other repository it is a no-op.
    Incremental,
    /// Set `auto_vacuum = INCREMENTAL` and run a full `VACUUM`. This converts a
    /// repository created before 5.3.0. It needs free disk about the size of
    /// `velo.db` and holds the write lock while it runs.
    Full,
}

/// The `auto_vacuum` mode a repository's database is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AutoVacuum {
    /// Freed pages stay in the file (repositories created before 5.3.0).
    None,
    /// SQLite returns freed pages at every commit.
    Full,
    /// Freed pages are returned on request, by [`vacuum`].
    Incremental,
}

impl AutoVacuum {
    fn from_pragma(v: i64) -> Self {
        match v {
            1 => AutoVacuum::Full,
            2 => AutoVacuum::Incremental,
            _ => AutoVacuum::None,
        }
    }
}

/// What [`vacuum`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Vacuumed {
    /// The mode that was requested.
    pub mode: Vacuum,
    /// Size of the database (`page_count * page_size`) before, after a WAL
    /// checkpoint.
    pub bytes_before: u64,
    /// Size of the database after.
    pub bytes_after: u64,
    /// The repository's `auto_vacuum` mode once the call finished.
    pub auto_vacuum: AutoVacuum,
}

fn db_bytes(conn: &rusqlite::Connection) -> Result<u64> {
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages.max(0) as u64 * size.max(0) as u64)
}

fn auto_vacuum_mode(conn: &rusqlite::Connection) -> Result<AutoVacuum> {
    let v: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
    Ok(AutoVacuum::from_pragma(v))
}

/// Return freed pages to the operating system.
///
/// Call it after [`run`] and after retention or compaction, which free pages
/// but leave the file as large as it was. `Incremental` does nothing on a
/// repository created before 5.3.0 (its `auto_vacuum` is off) and reports
/// [`AutoVacuum::None`]; `Full` converts such a repository once.
///
/// `Full` rewrites the whole database: it needs free disk about the size of
/// `velo.db` and holds the write lock for as long as it runs.
pub fn vacuum(guard: &WriteGuard, mode: Vacuum) -> Result<Vacuumed> {
    let conn = guard.conn();
    let bytes_before = db_bytes(conn)?;
    match mode {
        Vacuum::Incremental => {
            if auto_vacuum_mode(conn)? == AutoVacuum::Incremental {
                // `incremental_vacuum` frees one page per row stepped, and
                // `execute_batch` steps a statement once, which would return a
                // single page. Drain every row.
                let mut stmt = conn.prepare("PRAGMA incremental_vacuum")?;
                let mut rows = stmt.query([])?;
                while rows.next()?.is_some() {}
            }
        }
        Vacuum::Full => {
            conn.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?;
        }
    }
    let bytes_after = db_bytes(conn)?;
    Ok(Vacuumed {
        mode,
        bytes_before,
        bytes_after,
        auto_vacuum: auto_vacuum_mode(conn)?,
    })
}
