//! `velo remote` — manage named remotes.
//!
//! A remote's `url` is a path to another Velo repository (a directory containing
//! `.velo/`), or an `ssh://` / `child:` transport spec. Remotes and their
//! last-known branch tips live in the `remotes` / `remote_refs` tables.
//! Formatting lives in `velo-cli`.

use std::path::PathBuf;

use crate::error::{RefKind, Result, VeloError};
use crate::Repo;
use crate::WriteGuard;

/// A configured remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// The outcome of adding a remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub name: String,
    pub url: String,
    /// True when the URL doesn't point at a Velo repository yet. Not an error —
    /// the path may become one later, and `clone` registers remotes for repos
    /// that already exist — but worth telling the user about.
    pub unreachable: bool,
}

/// Register `url` under `name`, replacing any existing remote of that name.
pub fn add(guard: &WriteGuard, name: &str, url: &str) -> Result<Added> {
    if name.is_empty() || name.contains('/') {
        return Err(VeloError::invalid(
            "Remote name must be non-empty and contain no '/'.",
        ));
    }
    guard.conn().execute(
        "INSERT OR REPLACE INTO remotes (name, url) VALUES (?, ?)",
        [name, url],
    )?;
    Ok(Added {
        name: name.to_string(),
        url: url.to_string(),
        unreachable: resolve_remote_root(url).is_err(),
    })
}

/// Every configured remote, ordered by name.
pub fn list(repo: &Repo) -> Result<Vec<Remote>> {
    let conn = repo.conn();
    let mut stmt = conn.prepare("SELECT name, url FROM remotes ORDER BY name")?;
    let remotes: Vec<Remote> = stmt
        .query_map([], |r| {
            Ok(Remote {
                name: r.get(0)?,
                url: r.get(1)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(remotes)
}

/// Forget `name` and every tracking ref recorded under it.
pub fn remove(guard: &WriteGuard, name: &str) -> Result<()> {
    let conn = guard.conn();
    let n = conn.execute("DELETE FROM remotes WHERE name = ?", [name])?;
    if n == 0 {
        return Err(VeloError::not_found(RefKind::Remote, name));
    }
    conn.execute("DELETE FROM remote_refs WHERE remote = ?", [name])?;
    Ok(())
}

// ─── Shared helpers (used by clone / fetch / push / pull) ─────────────────────

/// Look up a remote's URL by name.
pub(crate) fn remote_url(conn: &rusqlite::Connection, name: &str) -> Result<String> {
    conn.query_row("SELECT url FROM remotes WHERE name = ?", [name], |r| {
        r.get(0)
    })
    .map_err(|_| {
        VeloError::invalid(format!(
            "No remote named '{}'. Add one with 'velo remote add'.",
            name
        ))
    })
}

/// Validate that `url` points at a Velo repository and return its root path.
pub(crate) fn resolve_remote_root(url: &str) -> Result<PathBuf> {
    let root = PathBuf::from(url);
    if root.join(".velo/velo.db").is_file() {
        Ok(root)
    } else {
        Err(VeloError::invalid(format!(
            "'{}' is not a Velo repository (no .velo found).",
            url
        )))
    }
}

/// Record a remote branch's tip in `remote_refs`.
pub(crate) fn set_remote_ref(
    conn: &rusqlite::Connection,
    remote: &str,
    branch: &str,
    hash: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO remote_refs (remote, branch, hash) VALUES (?, ?, ?)",
        [remote, branch, hash],
    )?;
    Ok(())
}
