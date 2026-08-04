//! Test fixtures for Velo.
//!
//! Downstream projects shouldn't have to reinvent temp-repository setup, so the
//! fixture helpers that grew inside Velo's own suite live here instead.
//!
//! ```
//! use velo_testkit::TempRepo;
//!
//! let repo = TempRepo::new();
//! repo.write("app.rs", "fn main() {}\n");
//! let id = repo.save("initial");
//! assert!(!id.is_empty());
//! assert!(repo.is_clean());
//! ```
//!
//! The temporary directory is removed when the [`TempRepo`] is dropped.

use std::path::{Path, PathBuf};

use tempfile::TempDir;
use velo_core::{commands, Repo};

/// A throwaway repository in a temporary directory.
#[derive(Debug)]
pub struct TempRepo {
    // Field order matters: `repo` drops before `dir`, so the SQLite connection
    // closes before the directory is removed.
    repo: Repo,
    dir: TempDir,
}

impl Default for TempRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TempRepo {
    /// Create and initialise a repository in a fresh temporary directory.
    ///
    /// # Panics
    /// If the repository cannot be created — fixtures should fail loudly.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let repo = Repo::init(dir.path()).expect("init repository");
        TempRepo { repo, dir }
    }

    /// The repository root.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The open [`Repo`] handle.
    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// Absolute path of a repo-relative path.
    pub fn join(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// Write a file, creating parent directories as needed.
    pub fn write(&self, rel: &str, contents: &str) {
        let p = self.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(p, contents).expect("write file");
    }

    /// Read a file back.
    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.join(rel)).expect("read file")
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.join(rel).exists()
    }

    /// Snapshot the working tree, returning the new snapshot id.
    ///
    /// # Panics
    /// If there is nothing to save, or the save fails.
    pub fn save(&self, message: &str) -> String {
        commands::save::run(&self.write_guard(), message, false)
            .expect("save")
            .into_result()
            .expect("something to save")
            .hash
    }

    /// Create (or switch to) a branch.
    pub fn switch(&self, branch: &str) {
        commands::switch::run(&self.write_guard(), branch, false).expect("switch");
    }

    /// Switch, discarding tracked modifications.
    pub fn switch_force(&self, branch: &str) {
        commands::switch::run(&self.write_guard(), branch, true).expect("switch --force");
    }

    /// Take the write lock, for the mutating helpers above.
    ///
    /// # Panics
    /// If the lock is held — a fixture is single-threaded by construction, so
    /// that would be a bug in the test rather than contention.
    fn write_guard(&self) -> velo_core::WriteGuard<'_> {
        self.repo.write().expect("take the repository write lock")
    }

    /// The snapshot the working tree is based on (`.velo/PARENT`), or `""`.
    pub fn head_snapshot(&self) -> String {
        std::fs::read_to_string(self.join(".velo/PARENT"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// The current branch name (`.velo/HEAD`).
    pub fn branch(&self) -> String {
        std::fs::read_to_string(self.join(".velo/HEAD"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// True when nothing is unsaved.
    pub fn is_clean(&self) -> bool {
        commands::get_dirty_files(&self.repo).is_empty()
    }

    /// Verify integrity; useful as a final assertion in downstream tests.
    pub fn fsck(&self) -> velo_core::Result<()> {
        let report = commands::fsck::check(&self.repo)?;
        if report.is_healthy() {
            Ok(())
        } else {
            Err(velo_core::Error::corrupt(format!(
                "{} integrity problem(s): {}",
                report.problems.len(),
                report
                    .problems
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            )))
        }
    }
}
