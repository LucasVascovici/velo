//! The repository handle.
//!
//! A [`Repo`] owns one SQLite connection for its lifetime, rather than reopening
//! (and re-running migrations) per operation. Reads go straight through; writes
//! take an explicit [`WriteGuard`] so a caller can perform several mutations
//! under **one** lock.
//!
//! Discovery is a distinct, explicit call: [`Repo::discover`] searches upward,
//! [`Repo::open`] does not. Nothing here searches the filesystem implicitly.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::lock::RepoLock;
use crate::progress::{Observer, Phase, PhaseGuard, Silent};
use crate::{db, BranchName, SnapshotId, FORMAT_VERSION};

/// An open Velo repository.
///
/// `Send` but **not** `Sync` — `rusqlite::Connection` cannot be shared between
/// threads. Use one `Repo` per thread, or `Arc<Mutex<Repo>>`.
pub struct Repo {
    root: PathBuf,
    conn: rusqlite::Connection,
    /// Where long operations report progress. `Silent` unless a caller supplies
    /// one via [`Repo::observing`].
    observer: Box<dyn Observer>,
    /// Which paths count as the working tree. Everything, unless narrowed via
    /// [`Repo::scoped`].
    scope: crate::Scope,
    /// Which merge driver handles which path. Line-based diff3 for everything,
    /// unless set via [`Repo::merging`].
    drivers: crate::Drivers,
    /// Who hears about commits made through this handle. See [`crate::events`].
    listeners: Vec<Box<dyn crate::events::Listener>>,
    /// Where objects live, read once when the handle opens.
    object_location: crate::ObjectLocation,
}

impl std::fmt::Debug for Repo {
    /// Hand-written because `dyn Observer` isn't `Debug` — and requiring it of
    /// every consumer's progress bar would be a poor trade.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo").field("root", &self.root).finish()
    }
}

impl Repo {
    /// Create a repository at `root`.
    ///
    /// Fails with [`Error::AlreadyInitialized`] if one exists there, or
    /// [`Error::NestedRepo`] if an enclosing repository is found.
    pub fn init(root: &Path) -> Result<Self> {
        Self::init_with(root, crate::InitOptions::default())
    }

    /// Create a repository at `root` with explicit [`InitOptions`], such as
    /// keeping objects in the database. The choice is recorded and cannot be
    /// changed afterwards.
    pub fn init_with(root: &Path, options: crate::InitOptions) -> Result<Self> {
        crate::commands::init::run_with(root, options)?;
        Self::open(root)
    }

    /// Where this repository keeps its objects.
    pub fn object_location(&self) -> crate::ObjectLocation {
        self.object_location
    }

    /// Open the repository rooted exactly at `root` (i.e. `root/.velo` must
    /// exist). Does **not** search parent directories — use [`Repo::discover`].
    ///
    /// Refuses a repository whose format is newer than [`FORMAT_VERSION`], and
    /// does not migrate: see [`Repo::open_and_migrate`].
    pub fn open(root: &Path) -> Result<Self> {
        if !root.join(".velo").is_dir() {
            return Err(Error::NotARepo {
                searched_from: root.to_path_buf(),
            });
        }
        let conn = db::connect(&root.join(".velo/velo.db"))?;
        match db::format_version(&conn)? {
            v if v > FORMAT_VERSION => {
                return Err(Error::SchemaTooNew {
                    found: v,
                    supported: FORMAT_VERSION,
                })
            }
            v if db::is_pre_v2(v) => {
                return Err(Error::FormatTooOld {
                    found: v,
                    supported: FORMAT_VERSION,
                })
            }
            v if v < FORMAT_VERSION => {
                return Err(Error::MigrationRequired {
                    found: v,
                    supported: FORMAT_VERSION,
                })
            }
            _ => {}
        }
        let object_location = crate::ObjectLocation::read(&conn)?;
        Ok(Repo {
            root: root.to_path_buf(),
            conn,
            observer: Box::new(Silent),
            scope: crate::Scope::new(),
            drivers: crate::Drivers::new(),
            listeners: Vec::new(),
            object_location,
        })
    }

    /// Open, applying any pending migration first.
    ///
    /// Separate from [`Repo::open`] on purpose: a background process must not
    /// silently upgrade a repository another tool is mid-use. The caller decides
    /// when that happens.
    pub fn open_and_migrate(root: &Path) -> Result<Self> {
        if !root.join(".velo").is_dir() {
            return Err(Error::NotARepo {
                searched_from: root.to_path_buf(),
            });
        }
        let conn = db::connect(&root.join(".velo/velo.db"))?;
        let found = db::format_version(&conn)?;
        if found > FORMAT_VERSION {
            return Err(Error::SchemaTooNew {
                found,
                supported: FORMAT_VERSION,
            });
        }
        // Refused rather than migrated: stamping v2 over v1 rows would leave
        // 16-character ids computed by the v1 recipe in a database claiming to
        // be v2, and `fsck` could then only report the damage after the fact.
        if db::is_pre_v2(found) {
            return Err(Error::FormatTooOld {
                found,
                supported: FORMAT_VERSION,
            });
        }
        db::migrate(&conn)?;
        // v2 -> v3 adds the chunk directory; nothing else on disk changes. A
        // database-located repository has no object files, so no directory.
        if crate::ObjectLocation::read(&conn)? == crate::ObjectLocation::Files {
            std::fs::create_dir_all(root.join(".velo/chunks"))?;
        }
        let object_location = crate::ObjectLocation::read(&conn)?;
        Ok(Repo {
            root: root.to_path_buf(),
            conn,
            observer: Box::new(Silent),
            scope: crate::Scope::new(),
            drivers: crate::Drivers::new(),
            listeners: Vec::new(),
            object_location,
        })
    }

    /// Search `start` and its ancestors for a repository and open it.
    pub fn discover(start: &Path) -> Result<Self> {
        match crate::commands::find_repo_root(start) {
            Some(root) => Self::open_and_migrate(&root),
            None => Err(Error::NotARepo {
                searched_from: start.to_path_buf(),
            }),
        }
    }

    /// Report progress from long operations to `observer`.
    ///
    /// Consumes and returns the handle, so there is no mutating setter and no
    /// interior mutability: a repository's reporting is decided once, where it is
    /// opened.
    ///
    /// ```no_run
    /// # use velo_core::{Repo, progress::{Observer, Phase}};
    /// struct Bar;
    /// impl Observer for Bar {
    ///     fn advance(&self, phase: Phase, by: u64) { /* redraw */ }
    /// }
    /// # fn main() -> Result<(), velo_core::Error> {
    /// let repo = Repo::discover(std::path::Path::new("."))?.observing(Bar);
    /// # Ok(()) }
    /// ```
    pub fn observing(mut self, observer: impl Observer + 'static) -> Self {
        self.observer = Box::new(observer);
        self
    }

    /// Subscribe to the changes committed through this handle.
    ///
    /// Consumes the handle like [`Repo::observing`]; call it again to add more
    /// listeners, which are notified in registration order. Same-handle only —
    /// see [`crate::events`].
    pub fn listening(mut self, listener: impl crate::events::Listener + 'static) -> Self {
        self.listeners.push(Box::new(listener));
        self
    }

    /// Tell every listener about a change that has already committed.
    pub(crate) fn emit(&self, event: crate::events::Event) {
        for listener in &self.listeners {
            listener.notify(&event);
        }
    }

    /// Announce a committed snapshot: `Saved`, then `Merged` when it has a
    /// second parent.
    pub(crate) fn emit_saved(
        &self,
        snapshot: &SnapshotId,
        branch: &BranchName,
        parent: Option<&SnapshotId>,
        merge_parent: Option<&SnapshotId>,
    ) {
        if self.listeners.is_empty() {
            return;
        }
        use crate::events::Event;
        self.emit(Event::Saved {
            snapshot: snapshot.clone(),
            branch: branch.clone(),
            parent: parent.cloned(),
            merge_parent: merge_parent.cloned(),
        });
        if let (Some(ours), Some(theirs)) = (parent, merge_parent) {
            self.emit(Event::Merged {
                snapshot: snapshot.clone(),
                into: branch.clone(),
                ours: ours.clone(),
                theirs: theirs.clone(),
            });
        }
    }

    /// Emit `Saved` for rows known only by their stored strings (the commands
    /// that predate typed ids). Empty strings mean "no parent".
    pub(crate) fn emit_saved_raw(
        &self,
        snapshot: &str,
        branch: &str,
        parent: &str,
        merge_parent: &str,
    ) {
        if self.listeners.is_empty() {
            return;
        }
        let opt = |s: &str| {
            let s = s.trim();
            (!s.is_empty()).then(|| SnapshotId::from_stored(s))
        };
        self.emit_saved(
            &SnapshotId::from_stored(snapshot),
            &BranchName::from_stored(branch.trim()),
            opt(parent).as_ref(),
            opt(merge_parent).as_ref(),
        );
    }

    /// Announce history that arrived from elsewhere, if any did.
    pub(crate) fn emit_imported(&self, snapshots: usize) {
        if snapshots > 0 {
            self.emit(crate::events::Event::Imported { snapshots });
        }
    }

    /// Open a phase of work. The returned guard closes it when dropped.
    ///
    /// `total` is `None` when the size isn't known in advance.
    pub(crate) fn phase(&self, phase: Phase, total: Option<u64>) -> PhaseGuard<'_> {
        PhaseGuard::new(&*self.observer, phase, total)
    }

    /// The handle's observer, used when a per-call one is not supplied.
    pub(crate) fn observer(&self) -> &dyn Observer {
        &*self.observer
    }

    /// Narrow what this handle treats as the working tree.
    ///
    /// Unlike an observer — which belongs to *one operation*, and is passed per
    /// call for that reason — a scope describes what the repository *contains* as
    /// far as this application is concerned. That does not change between
    /// operations, so it belongs to the handle.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), velo_core::Error> {
    /// use velo_core::{Repo, Scope};
    ///
    /// // Keep an application's own cache out of the user's history without
    /// // writing `.veloignore` into their folder.
    /// let repo = Repo::discover(std::path::Path::new("."))?
    ///     .scoped(Scope::new().ignore(".myapp-cache/**")?);
    /// # let _ = repo;
    /// # Ok(()) }
    /// ```
    pub fn scoped(mut self, scope: crate::Scope) -> Self {
        self.scope = scope;
        self
    }

    /// Choose merge drivers by path pattern.
    ///
    /// Like a [`Scope`](crate::Scope), this lives on the handle rather than on
    /// one merge: which files are JSON is a property of the repository, not of
    /// one merge. Plan, merge, cherry-pick and rebase all consult it, so they
    /// cannot disagree about how a file is merged.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), velo_core::Error> {
    /// use velo_core::{Drivers, Repo};
    /// use velo_merge::LineDriver;
    ///
    /// let repo = Repo::discover(std::path::Path::new("."))?
    ///     .merging(Drivers::new().with("*.cfg", LineDriver)?);
    /// # let _ = repo;
    /// # Ok(()) }
    /// ```
    pub fn merging(mut self, drivers: crate::Drivers) -> Self {
        self.drivers = drivers;
        self
    }

    /// The handle's merge drivers.
    pub(crate) fn drivers(&self) -> &crate::Drivers {
        &self.drivers
    }

    /// The handle's scope, for the directory walk.
    pub(crate) fn scope(&self) -> &crate::Scope {
        &self.scope
    }

    /// The repository root (the directory *containing* `.velo`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The object store: the one way commands reach stored file content.
    pub(crate) fn objects(&self) -> crate::storage::ObjectStore<'_> {
        match self.object_location {
            crate::ObjectLocation::Files => {
                crate::storage::ObjectStore::at(self.root.join(".velo/objects"))
            }
            crate::ObjectLocation::Database => crate::storage::ObjectStore::Database(&self.conn),
        }
    }

    /// The long-lived connection, for command implementations inside this crate.
    ///
    /// Deliberately not public: SQLite is an implementation detail, and exposing
    /// it would put `rusqlite` back in the public API.
    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// The newest snapshot on `branch`, or `None` when it has none yet.
    ///
    /// A branch's tip is *derived* — the newest snapshot carrying that name — so
    /// this is the direct question. Answering it by resolving the branch as a
    /// spec and interpreting [`Error::NotFound`] works, but conflates "the branch
    /// is unborn" with "there is no such branch"; see [`Repo::branch_exists`].
    pub fn branch_tip(&self, branch: &BranchName) -> Result<Option<SnapshotId>> {
        Ok(crate::commands::branch_tip(&self.conn, branch.as_str()).map(SnapshotId::from_stored))
    }

    /// Whether `branch` exists, including one created but not yet committed to.
    ///
    /// Distinct from [`Repo::branch_tip`] returning `Some`: `velo switch new` and
    /// a fresh `init` both create a branch that exists with no snapshots on it.
    pub fn branch_exists(&self, branch: &BranchName) -> Result<bool> {
        Ok(crate::commands::all_branch_names(&self.conn)
            .iter()
            .any(|b| b == branch.as_str()))
    }

    /// Everything recorded about one snapshot, by id.
    ///
    /// A consumer holds ids, not specs, and previously had to format one back
    /// into text for `show::run` to resolve again — or walk history and filter.
    ///
    /// Returns [`history::Entry`](crate::commands::history::Entry) rather than a
    /// near-duplicate type: it already carries exactly this, already typed. Use
    /// [`commands::show::run`](crate::commands::show::run) when the diff against
    /// the parent is wanted too, since computing that is not free.
    pub fn snapshot(&self, id: &SnapshotId) -> Result<crate::commands::history::Entry> {
        crate::commands::history::snapshot(self, id)
    }

    /// Every snapshot whose metadata satisfies all of `filters`, across all
    /// branches, newest first.
    ///
    /// One indexed query instead of walking history and calling
    /// [`Repo::snapshot_meta`] per entry. Soft-deleted and stash branches are
    /// excluded, exactly as `history::Options { all: true }` excludes them. To
    /// scope to one branch or ancestry, or to combine with a path filter, use
    /// [`commands::history::run`](crate::commands::history::run) with
    /// `Options::meta`.
    ///
    /// Store only: needs no working tree.
    ///
    /// # Errors
    /// [`Error::InvalidInput`] when `filters` is empty — "everything" is not a
    /// query, and an accidental empty slice should not return a whole history.
    pub fn find_snapshots(
        &self,
        filters: &[crate::commands::history::MetaFilter<'_>],
    ) -> Result<Vec<crate::commands::history::Entry>> {
        if filters.is_empty() {
            return Err(Error::invalid(
                "find_snapshots needs at least one metadata filter.",
            ));
        }
        let history = crate::commands::history::run(
            self,
            crate::commands::history::Options {
                all: true,
                meta: filters,
                ..Default::default()
            },
        )?;
        Ok(history.entries)
    }

    /// A value that changes whenever the repository's history does.
    ///
    /// For an application that has to notice a second window, a `pull`, or the
    /// user running `velo` in the same folder: poll one integer instead of every
    /// branch tip. Equal tokens mean nothing tracked has changed; a different
    /// token means something has, and the caller should re-read what it cares
    /// about.
    ///
    /// Covers snapshots, branches and tags. It deliberately does **not** cover
    /// the working tree — that is the filesystem's business, and a consumer with
    /// its own storage has no working tree at all.
    pub fn head_token(&self) -> Result<u64> {
        let mut h = blake3::Hasher::new();

        let (count, newest): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM snapshots",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        h.update(&count.to_le_bytes());
        h.update(&newest.to_le_bytes());

        // Branch tips and tags can move without any row being added, so counting
        // is not enough — the values themselves have to go in.
        for (table, columns) in [("branches", "name, tip"), ("tags", "name, snapshot_hash")] {
            let sql = format!("SELECT {} FROM {} ORDER BY 1", columns, table);
            let mut stmt = self.conn.prepare(&sql)?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (a, b) = row?;
                h.update(a.as_bytes());
                h.update(b" ");
                h.update(b.as_bytes());
                h.update(
                    b"
",
                );
            }
        }

        let digest = h.finalize();
        let mut first = [0u8; 8];
        first.copy_from_slice(&digest.as_bytes()[..8]);
        Ok(u64::from_le_bytes(first))
    }

    /// Repository format version as recorded on disk.
    pub fn format_version(&self) -> Result<u32> {
        Ok(db::format_version(&self.conn)?)
    }

    // ── Write access ─────────────────────────────────────────────────────────

    /// Take the exclusive repository lock, failing immediately if another
    /// process holds it.
    ///
    /// Hold the guard across a group of mutations so they share one lock. Drop it
    /// promptly: while it is alive, no other process can write. In particular do
    /// not hold it across user interaction — a GUI that takes the lock and then
    /// shows a modal will wedge every other process.
    pub fn write(&self) -> Result<WriteGuard<'_>> {
        Ok(WriteGuard {
            _lock: RepoLock::acquire(&self.root)?,
            repo: self,
        })
    }

    /// Like [`Repo::write`] but returns `Ok(None)` instead of erroring when the
    /// lock is already held.
    pub fn try_write(&self) -> Result<Option<WriteGuard<'_>>> {
        match RepoLock::try_acquire(&self.root)? {
            Some(lock) => Ok(Some(WriteGuard {
                _lock: lock,
                repo: self,
            })),
            None => Ok(None),
        }
    }

    /// Retry acquiring the write lock until `timeout` elapses.
    pub fn write_timeout(&self, timeout: Duration) -> Result<WriteGuard<'_>> {
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_millis(5);
        loop {
            if let Some(guard) = self.try_write()? {
                return Ok(guard);
            }
            if Instant::now() >= deadline {
                return Err(Error::Locked { held_by: None });
            }
            std::thread::sleep(backoff.min(Duration::from_millis(100)));
            backoff *= 2;
        }
    }
}

/// Proof that the caller holds the repository write lock.
///
/// Every mutating operation takes `&WriteGuard`, so it is impossible to mutate
/// without having acquired the lock — the type system enforces what used to be a
/// convention.
#[derive(Debug)]
pub struct WriteGuard<'a> {
    _lock: RepoLock,
    repo: &'a Repo,
}

impl WriteGuard<'_> {
    /// The repository this guard grants write access to.
    pub fn repo(&self) -> &Repo {
        self.repo
    }

    /// Convenience: the repository root.
    pub fn root(&self) -> &Path {
        self.repo.root()
    }

    /// The repository's connection.
    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        self.repo.conn()
    }

    /// Open a phase of work — see [`Repo::phase`].
    pub(crate) fn phase(&self, phase: Phase, total: Option<u64>) -> PhaseGuard<'_> {
        self.repo.phase(phase, total)
    }

    /// Begin a transaction on the shared connection.
    ///
    /// `Connection::transaction` needs `&mut Connection`, which a shared
    /// connection can't give out — hence `unchecked_transaction`, whose one rule
    /// is "don't nest". Holding this guard is what makes that safe: there is a
    /// single connection per `Repo`, a single guard per lock, and mutating code
    /// reaches a transaction only through here.
    pub(crate) fn transaction(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.repo.conn().unchecked_transaction()?)
    }
}
