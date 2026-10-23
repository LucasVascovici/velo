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
    /// Whether this is a directory repository or a single SQLite file.
    layout: Layout,
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
    /// What this handle writes into the lock's holder record, so a refused
    /// writer can say what is running. Set via [`Repo::lock_label`].
    lock_label: Option<String>,
    /// Ceilings on what this handle will allocate for data received from
    /// elsewhere. Set via [`Repo::limited`].
    limits: crate::Limits,
    /// How many links a stored delta chain may reach before a save writes a
    /// keyframe; see [`crate::tree_store::KEYFRAME_INTERVAL`].
    keyframe_interval: u32,
    /// The tree this handle last committed, keyed by its snapshot id, so the next
    /// save on top of it need not read that tree back. A snapshot's tree is
    /// immutable (the id commits to it), so the id alone is a sound key: a
    /// repack, compaction or a second handle's write can change how rows are
    /// stored but not what the tree is, and a snapshot that is gone is refused
    /// by the parent check before the cache is consulted. `RefCell`, so the
    /// handle stays `Send` and not `Sync`.
    last_tree: std::cell::RefCell<Option<(String, std::sync::Arc<crate::tree_store::StoredTree>)>>,
    /// Where bearer tokens for HTTP remotes come from, given a remote's URL.
    /// `None` sends no credentials. Set via [`Repo::credentials`].
    credentials: Option<Credentials>,
    /// Tests only: a fixed clock for this handle, so two repositories driven in
    /// lockstep mint the same snapshot ids. Per handle, never global.
    #[cfg(test)]
    pub(crate) test_now_ms: std::cell::Cell<Option<i64>>,
}

#[cfg(test)]
thread_local! {
    static TEST_KEYFRAME_INTERVAL: std::cell::Cell<u32> =
        const { std::cell::Cell::new(crate::tree_store::KEYFRAME_INTERVAL) };
}

/// Tests only: every repository opened by this test's thread from now on starts
/// at K = 1, i.e. stores whole trees. For tests that inspect `file_map` or
/// exercise operations that do not fold deltas yet.
#[cfg(test)]
pub(crate) fn keyframes_only() {
    TEST_KEYFRAME_INTERVAL.with(|k| k.set(1));
}

/// Tests only: every repository opened by this test's thread from now on starts
/// at K = `k`, so one body can run against several storage shapes.
#[cfg(test)]
pub(crate) fn set_test_keyframe_interval(k: u32) {
    TEST_KEYFRAME_INTERVAL.with(|c| c.set(k));
}

fn initial_keyframe_interval() -> u32 {
    #[cfg(test)]
    {
        TEST_KEYFRAME_INTERVAL.with(|k| k.get())
    }
    #[cfg(not(test))]
    {
        crate::tree_store::KEYFRAME_INTERVAL
    }
}

/// A callback mapping a remote URL to the bearer token to present to it.
type Credentials = Box<dyn Fn(&str) -> Option<String> + Send>;

/// How a repository sits on disk.
#[derive(Clone, Debug)]
enum Layout {
    /// The usual layout: `root/.velo/` beside a working tree.
    Directory { root: PathBuf },
    /// One SQLite file holding everything: no `.velo`, no working tree.
    SingleFile { db: PathBuf },
}

/// The `settings` value that marks a database as a single-file repository.
const SINGLE_FILE: &str = "single-file";

/// Read one row of the `settings` table; `None` when the table or row is absent.
fn read_setting(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    let has_table: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'settings')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(None);
    }
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// Refuse a database whose format this build cannot open as it is.
fn check_format(conn: &rusqlite::Connection) -> Result<()> {
    match db::format_version(conn)? {
        v if v > FORMAT_VERSION => Err(Error::SchemaTooNew {
            found: v,
            supported: FORMAT_VERSION,
        }),
        v if db::is_pre_v2(v) => Err(Error::FormatTooOld {
            found: v,
            supported: FORMAT_VERSION,
        }),
        v if v < FORMAT_VERSION => Err(Error::MigrationRequired {
            found: v,
            supported: FORMAT_VERSION,
        }),
        _ => Ok(()),
    }
}

/// Refuse a database that is not a single-file repository, so a directory
/// repository's `velo.db` is never opened without its object files and refs.
fn check_single_file(conn: &rusqlite::Connection) -> Result<()> {
    if read_setting(conn, "layout")?.as_deref() == Some(SINGLE_FILE) {
        Ok(())
    } else {
        Err(Error::invalid(
            "This database is not a single-file repository; open the directory instead.",
        ))
    }
}

impl std::fmt::Debug for Repo {
    /// Hand-written because `dyn Observer` isn't `Debug` — and requiring it of
    /// every consumer's progress bar would be a poor trade.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo")
            .field("layout", &self.layout)
            .finish()
    }
}

impl Repo {
    /// The wall clock for snapshot and trash stamps made through this handle.
    /// Release builds read the real clock; tests may pin it per handle.
    pub(crate) fn now_ms(&self) -> i64 {
        #[cfg(test)]
        if let Some(ms) = self.test_now_ms.get() {
            return ms;
        }
        crate::commands::snapshot_timestamp_ms()
    }

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
        check_format(&conn)?;
        let object_location = crate::ObjectLocation::read(&conn)?;
        Ok(Repo {
            layout: Layout::Directory {
                root: root.to_path_buf(),
            },
            conn,
            observer: Box::new(Silent),
            scope: crate::Scope::new(),
            drivers: crate::Drivers::new(),
            credentials: None,
            listeners: Vec::new(),
            object_location,
            lock_label: None,
            limits: crate::Limits::default(),
            keyframe_interval: initial_keyframe_interval(),
            last_tree: std::cell::RefCell::new(None),
            #[cfg(test)]
            test_now_ms: std::cell::Cell::new(None),
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
            layout: Layout::Directory {
                root: root.to_path_buf(),
            },
            conn,
            observer: Box::new(Silent),
            scope: crate::Scope::new(),
            drivers: crate::Drivers::new(),
            credentials: None,
            listeners: Vec::new(),
            object_location,
            lock_label: None,
            limits: crate::Limits::default(),
            keyframe_interval: initial_keyframe_interval(),
            last_tree: std::cell::RefCell::new(None),
            #[cfg(test)]
            test_now_ms: std::cell::Cell::new(None),
        })
    }

    /// Create a single-file repository: everything lives in the SQLite file at
    /// `path`, with objects in its `objects` table.
    ///
    /// There is no `.velo` directory and no working tree, so only the store-only
    /// API works; commands that read or write files return
    /// [`Error::Unsupported`]. This is the shape a browser build opens through
    /// the SQLite VFS, and a portable single-file store natively.
    ///
    /// Fails with [`Error::AlreadyInitialized`] if `path` exists.
    pub fn create_file(path: &Path) -> Result<Self> {
        // `path.exists()` alone is always false on wasm32; see `db::connect_existing`.
        if path.exists() || db::connect_existing(path)?.is_some() {
            return Err(Error::AlreadyInitialized {
                at: path.to_path_buf(),
            });
        }
        db::init_db_at_path(path)?;
        {
            let conn = db::connect(path)?;
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('objects', 'database')",
                [],
            )?;
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('layout', ?)",
                [SINGLE_FILE],
            )?;
            crate::commands::register_branch(&conn, "main", "")?;
        }
        Self::open_file(path)
    }

    /// Open the single-file repository at `path`.
    ///
    /// Refuses an older format with [`Error::MigrationRequired`], as
    /// [`Repo::open`] does, and a database that is not a single-file repository
    /// (such as a directory repository's `velo.db`) with [`Error::InvalidInput`].
    pub fn open_file(path: &Path) -> Result<Self> {
        let conn = Self::connect_file(path)?;
        check_format(&conn)?;
        check_single_file(&conn)?;
        Self::from_file(path, conn)
    }

    /// Open a single-file repository, applying any pending migration first.
    pub fn open_file_and_migrate(path: &Path) -> Result<Self> {
        let conn = Self::connect_file(path)?;
        let found = db::format_version(&conn)?;
        if found > FORMAT_VERSION {
            return Err(Error::SchemaTooNew {
                found,
                supported: FORMAT_VERSION,
            });
        }
        if db::is_pre_v2(found) {
            return Err(Error::FormatTooOld {
                found,
                supported: FORMAT_VERSION,
            });
        }
        // Checked before migrating: never rewrite a database that is not ours.
        check_single_file(&conn)?;
        db::migrate(&conn)?;
        Self::from_file(path, conn)
    }

    fn connect_file(path: &Path) -> Result<rusqlite::Connection> {
        // A directory is never a single-file repository; on wasm32 `is_dir` is
        // always false, and SQLite answers whether the file exists.
        if path.is_dir() {
            return Err(Error::NotARepo {
                searched_from: path.to_path_buf(),
            });
        }
        db::connect_existing(path)?.ok_or_else(|| Error::NotARepo {
            searched_from: path.to_path_buf(),
        })
    }

    fn from_file(path: &Path, conn: rusqlite::Connection) -> Result<Self> {
        let object_location = crate::ObjectLocation::read(&conn)?;
        Ok(Repo {
            layout: Layout::SingleFile {
                db: path.to_path_buf(),
            },
            conn,
            observer: Box::new(Silent),
            scope: crate::Scope::new(),
            drivers: crate::Drivers::new(),
            credentials: None,
            listeners: Vec::new(),
            object_location,
            lock_label: None,
            limits: crate::Limits::default(),
            keyframe_interval: initial_keyframe_interval(),
            last_tree: std::cell::RefCell::new(None),
            #[cfg(test)]
            test_now_ms: std::cell::Cell::new(None),
        })
    }

    /// Whether this repository has a working tree. `false` for a single-file
    /// repository, whose only store is the database.
    pub fn has_working_tree(&self) -> bool {
        matches!(self.layout, Layout::Directory { .. })
    }

    /// The branch `HEAD` names: `.velo/HEAD`, or `main` in a single-file
    /// repository. Store-only code must not read `.velo/HEAD` directly: in a
    /// shared directory it could belong to an unrelated repository.
    pub(crate) fn head_branch(&self) -> BranchName {
        match &self.layout {
            Layout::Directory { root } => BranchName::from_stored(
                std::fs::read_to_string(root.join(".velo/HEAD"))
                    .unwrap_or_else(|_| "main".into())
                    .trim(),
            ),
            Layout::SingleFile { .. } => BranchName::from_stored("main"),
        }
    }

    /// The checked-out snapshot (`.velo/PARENT`), or `None` when nothing is
    /// checked out or the repository has no working tree.
    pub(crate) fn position(&self) -> Option<SnapshotId> {
        match &self.layout {
            Layout::Directory { root } => {
                let raw = std::fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
                let raw = raw.trim();
                (!raw.is_empty()).then(|| SnapshotId::from_stored(raw))
            }
            Layout::SingleFile { .. } => None,
        }
    }

    /// The lock file this repository's writers contend on.
    fn lock_path(&self) -> PathBuf {
        match &self.layout {
            Layout::Directory { root } => root.join(".velo/lock"),
            Layout::SingleFile { db } => {
                let mut name = db.as_os_str().to_owned();
                name.push(".lock");
                PathBuf::from(name)
            }
        }
    }

    /// Label this handle's lock in the holder record (typically the command
    /// line). Core never reads argv itself; the caller decides what to say.
    pub fn lock_label(mut self, label: impl Into<String>) -> Self {
        self.lock_label = Some(label.into());
        self
    }

    /// Who currently holds this repository's write lock, if a record exists.
    ///
    /// The record can be stale when nobody holds the lock (it is removed on a
    /// normal release, but not after a crash), and it is informational only.
    /// Whether the pid is still alive is the caller's check.
    pub fn lock_holder(&self) -> Option<crate::lock::LockHolder> {
        RepoLock::holder_at(&self.lock_path())
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

    /// Supply bearer tokens for HTTP remotes.
    ///
    /// The provider is given a remote's URL and returns the token to send as
    /// `Authorization: Bearer <token>`, or `None` for no credentials. Core reads
    /// no environment, so the caller decides where tokens come from; they never
    /// belong in the URL itself, which `transport::open` refuses. Tokens are
    /// never stored in the repository and never appear in errors.
    ///
    /// ```no_run
    /// # fn main() -> Result<(), velo_core::Error> {
    /// let repo = velo_core::Repo::discover(std::path::Path::new("."))?
    ///     .credentials(|_url| std::env::var("VELO_TOKEN").ok());
    /// # let _ = repo;
    /// # Ok(()) }
    /// ```
    pub fn credentials(
        mut self,
        provider: impl Fn(&str) -> Option<String> + Send + 'static,
    ) -> Self {
        self.credentials = Some(Box::new(provider));
        self
    }

    /// The bearer token configured for a remote URL, if any.
    pub(crate) fn token_for_url(&self, url: &str) -> Option<String> {
        self.credentials.as_ref().and_then(|f| f(url))
    }

    /// Set the ceilings on what this handle allocates when reading data that
    /// came from another machine (bundles, packs, git streams).
    ///
    /// Like [`Repo::scoped`], this belongs to the handle: it is a property of
    /// how much the application trusts its sources, not of one call. Hitting a
    /// ceiling is [`Error::LimitExceeded`](crate::Error::LimitExceeded).
    ///
    /// ```no_run
    /// # fn main() -> Result<(), velo_core::Error> {
    /// use velo_core::{Limits, Repo};
    ///
    /// let repo = Repo::discover(std::path::Path::new("."))?
    ///     .limited(Limits::default().with_max_object_bytes(64 << 20));
    /// # let _ = repo;
    /// # Ok(()) }
    /// ```
    pub fn limited(mut self, limits: crate::Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The cached tree of `snapshot`, when it is the one this handle last wrote.
    pub(crate) fn cached_tree(
        &self,
        snapshot: &str,
    ) -> Option<std::sync::Arc<crate::tree_store::StoredTree>> {
        match &*self.last_tree.borrow() {
            Some((id, tree)) if id == snapshot => Some(std::sync::Arc::clone(tree)),
            _ => None,
        }
    }

    /// Remember `tree` as the tree of the snapshot just committed.
    pub(crate) fn remember_tree(&self, snapshot: &str, tree: crate::tree_store::StoredTree) {
        *self.last_tree.borrow_mut() = Some((snapshot.to_string(), std::sync::Arc::new(tree)));
    }

    /// Forget the cached tree. Tests and the profile harness use it to measure a save
    /// without the cache.
    #[cfg(test)]
    pub(crate) fn clear_last_tree(&self) {
        *self.last_tree.borrow_mut() = None;
    }

    /// The handle's keyframe interval K, at least 1; 1 stores every tree whole.
    pub(crate) fn keyframe_interval(&self) -> u32 {
        self.keyframe_interval
    }

    /// Tests and fuzzing only: sets the keyframe interval K (at least 1).
    #[doc(hidden)]
    #[cfg(any(test, feature = "failpoints", feature = "fuzzing"))]
    pub fn set_keyframe_interval(&mut self, k: u32) {
        assert!(k >= 1, "keyframe interval must be at least 1");
        self.keyframe_interval = k;
    }

    /// The handle's read limits.
    pub fn limits(&self) -> &crate::Limits {
        &self.limits
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
    ///
    /// For a single-file repository this is the database file's parent
    /// directory. There is **no working tree** there: it is only where the file
    /// happens to sit, and nothing velo does will read or write it.
    pub fn root(&self) -> &Path {
        match &self.layout {
            Layout::Directory { root } => root,
            Layout::SingleFile { db } => match db.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            },
        }
    }

    /// The object store: the one way commands reach stored file content.
    pub(crate) fn objects(&self) -> crate::storage::ObjectStore<'_> {
        match self.object_location {
            crate::ObjectLocation::Files => {
                crate::storage::ObjectStore::at(self.root().join(".velo/objects"))
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

    /// The `Locked` error for a busy lock, naming the holder when it is known.
    fn locked_error(&self) -> Error {
        let holder = RepoLock::holder_at(&self.lock_path());
        Error::Locked {
            held_by: holder.as_ref().map(|h| h.pid),
            holder,
        }
    }

    /// Take the exclusive repository lock, failing immediately if another
    /// process holds it.
    ///
    /// Hold the guard across a group of mutations so they share one lock. Drop it
    /// promptly: while it is alive, no other process can write. In particular do
    /// not hold it across user interaction — a GUI that takes the lock and then
    /// shows a modal will wedge every other process.
    pub fn write(&self) -> Result<WriteGuard<'_>> {
        let lock =
            match RepoLock::try_acquire_labelled_at(&self.lock_path(), self.lock_label.as_deref())?
            {
                Some(lock) => lock,
                None => self.retry_unattributed_refusal()?,
            };
        Ok(WriteGuard {
            _lock: lock,
            repo: self,
        })
    }

    /// A refusal with no holder record is usually a lock that was just released
    /// but not yet free: a process forked by another thread keeps a copy of the
    /// lock file's descriptor until it execs. Wait a few milliseconds for that
    /// before reporting `Locked`. A refusal with a holder record is a real
    /// holder and is reported at once.
    fn retry_unattributed_refusal(&self) -> Result<RepoLock> {
        #[cfg(not(target_family = "wasm"))]
        for _ in 0..20 {
            if RepoLock::holder_at(&self.lock_path()).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            if let Some(lock) =
                RepoLock::try_acquire_labelled_at(&self.lock_path(), self.lock_label.as_deref())?
            {
                return Ok(lock);
            }
        }
        Err(self.locked_error())
    }

    /// Like [`Repo::write`] but returns `Ok(None)` instead of erroring when the
    /// lock is already held.
    pub fn try_write(&self) -> Result<Option<WriteGuard<'_>>> {
        match RepoLock::try_acquire_labelled_at(&self.lock_path(), self.lock_label.as_deref())? {
            Some(lock) => Ok(Some(WriteGuard {
                _lock: lock,
                repo: self,
            })),
            None => Ok(None),
        }
    }

    /// Retry acquiring the write lock until `timeout` elapses.
    pub fn write_timeout(&self, timeout: Duration) -> Result<WriteGuard<'_>> {
        // `Instant::now()` panics on wasm32-unknown-unknown, and the wasm
        // `RepoLock` always succeeds, so there is nothing to wait for: take it once.
        #[cfg(target_family = "wasm")]
        {
            let _ = timeout;
            self.write()
        }
        #[cfg(not(target_family = "wasm"))]
        self.write_timeout_native(timeout)
    }

    #[cfg(not(target_family = "wasm"))]
    fn write_timeout_native(&self, timeout: Duration) -> Result<WriteGuard<'_>> {
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_millis(5);
        loop {
            if let Some(guard) = self.try_write()? {
                return Ok(guard);
            }
            if Instant::now() >= deadline {
                return Err(self.locked_error());
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

impl Drop for WriteGuard<'_> {
    /// Bound the WAL once the writes made under this guard have committed.
    /// The lock is still held (fields drop after this), so the checkpoint
    /// cannot race another writer.
    fn drop(&mut self) {
        #[cfg(not(target_family = "wasm"))]
        if let Some(path) = self.repo.conn().path().filter(|p| !p.is_empty()) {
            db::bound_wal(self.repo.conn(), Path::new(path));
        }
    }
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
