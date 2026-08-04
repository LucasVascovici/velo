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
use crate::{db, FORMAT_VERSION};

/// An open Velo repository.
///
/// `Send` but **not** `Sync` — `rusqlite::Connection` cannot be shared between
/// threads. Use one `Repo` per thread, or `Arc<Mutex<Repo>>`.
#[derive(Debug)]
pub struct Repo {
    root: PathBuf,
    conn: rusqlite::Connection,
}

impl Repo {
    /// Create a repository at `root`.
    ///
    /// Fails with [`Error::AlreadyInitialized`] if one exists there, or
    /// [`Error::NestedRepo`] if an enclosing repository is found.
    pub fn init(root: &Path) -> Result<Self> {
        crate::commands::init::run(root)?;
        Self::open(root)
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
            v if v < FORMAT_VERSION && v != 0 => {
                return Err(Error::MigrationRequired {
                    found: v,
                    supported: FORMAT_VERSION,
                })
            }
            _ => {}
        }
        Ok(Repo {
            root: root.to_path_buf(),
            conn,
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
        db::migrate(&conn)?;
        Ok(Repo {
            root: root.to_path_buf(),
            conn,
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

    /// The repository root (the directory *containing* `.velo`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The long-lived connection, for command implementations inside this crate.
    ///
    /// Deliberately not public: SQLite is an implementation detail, and exposing
    /// it would put `rusqlite` back in the public API.
    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        &self.conn
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
