//! Embeddable core of Velo: a content-addressed, versioned content store with
//! branching, three-way merging and repository sync.
//!
//! # Discipline boundary
//!
//! Every operation takes a [`Repo`] (reads) or a [`WriteGuard`] (mutations), so
//! the type system distinguishes the two and it is impossible to mutate without
//! holding the repository lock. SQLite is a private implementation detail: it
//! appears nowhere in the public API.
//!
//! This crate performs **no terminal output and reads no process environment**.
//! It returns data and typed errors; presenting them is the caller's job. The
//! lints below enforce that mechanically rather than by convention — if you find
//! yourself wanting `println!` here, the value belongs in a returned struct.
//!
//! Specifically, `velo-core` contains no `println!`/`eprintln!`/`print!`, no
//! `process::exit`, no `env::current_dir`/`current_exe`, no `env::var`, no
//! `$EDITOR` spawning, no terminal detection and no colour codes. Anything that
//! depends on the surrounding process is passed in — see
//! [`transport::Spawn`] for how a subprocess remote is configured.
//!
//! # Entry point
//!
//! [`Repo`] is the handle: [`Repo::init`], [`Repo::open`], [`Repo::discover`].
//! Mutations go through a write guard so several changes share one lock and one
//! transaction:
//!
//! ```no_run
//! # fn main() -> Result<(), velo_core::Error> {
//! let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
//! println!("format v{}", repo.format_version()?);   // reads need no lock
//! {
//!     let _w = repo.write()?;   // exclusive until the guard drops
//!     // …several mutations share this one lock and transaction…
//! }
//! # Ok(()) }
//! ```
//!
//! Typed outcomes mean callers branch on state rather than parsing messages:
//!
//! ```no_run
//! # use velo_core::{Repo, Error};
//! # fn demo(repo: &Repo) {
//! match repo.write() {
//!     Ok(_guard) => { /* proceed */ }
//!     Err(Error::Locked { held_by }) => eprintln!("busy: {:?}", held_by),
//!     Err(e) if e.is_reconcile_needed() => eprintln!("pull first"),
//!     Err(e) => eprintln!("{e}"),
//! }
//! # }
//! ```
//!
//! # Threading
//!
//! [`Repo`] is `Send` but not `Sync`: SQLite connections are not shareable.
//! Use one `Repo` per thread, or `Arc<Mutex<Repo>>`, and `spawn_blocking` from
//! async contexts. The core is deliberately synchronous.
#![deny(clippy::print_stdout, clippy::print_stderr)]

pub mod commands;
pub mod db;
pub mod error;
pub mod lock;
pub mod repo;
pub mod serve;
pub mod storage;
pub mod transport;

#[cfg(test)]
mod tests;

// ─── Public surface ───────────────────────────────────────────────────────────

pub use error::{Error, Result};
pub use repo::{Repo, WriteGuard};

/// Re-exported so callers can match on merge outcomes without adding a second
/// dependency.
pub use velo_merge as merge;

/// Repository format version this crate reads and writes.
///
/// Recorded in SQLite's `PRAGMA user_version`. Opening a repository with a
/// higher value fails with [`Error::SchemaTooNew`] rather than risking a
/// half-migration — see `docs/FORMAT.md`.
pub const FORMAT_VERSION: u32 = 1;
