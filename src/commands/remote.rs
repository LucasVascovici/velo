//! `velo remote` — manage named remotes (filesystem paths for now).
//!
//! A remote's `url` is a path to another Velo repository (a directory
//! containing `.velo/`). Remotes and their last-known branch tips are stored in
//! the `remotes` / `remote_refs` tables.

use std::path::{Path, PathBuf};

use console::style;

use crate::db;
use crate::error::{Result, VeloError};

pub fn add(root: &Path, name: &str, url: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') {
        return Err(VeloError::InvalidInput(
            "Remote name must be non-empty and contain no '/'.".into(),
        ));
    }
    // Best-effort validation: warn (don't fail) if it isn't a repo yet — the
    // path may become one later, and clone creates remotes for repos that exist.
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    conn.execute(
        "INSERT OR REPLACE INTO remotes (name, url) VALUES (?, ?)",
        [name, url],
    )?;
    println!("{} Added remote '{}' → {}", style("✔").green().bold(), name, url);
    if resolve_remote_root(url).is_err() {
        println!(
            "  {} '{}' is not a Velo repository yet.",
            style("note:").yellow(),
            url
        );
    }
    Ok(())
}

pub fn list(root: &Path) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let mut stmt = conn.prepare("SELECT name, url FROM remotes ORDER BY name")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    if rows.is_empty() {
        println!("{}", style("No remotes configured. Add one with 'velo remote add <name> <path>'.").dim());
        return Ok(());
    }
    println!("{:<16} {}", style("Remote").bold(), style("URL").bold());
    for (name, url) in rows {
        println!("{:<16} {}", style(&name).cyan(), url);
    }
    Ok(())
}

pub fn remove(root: &Path, name: &str) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let n = conn.execute("DELETE FROM remotes WHERE name = ?", [name])?;
    if n == 0 {
        return Err(VeloError::InvalidInput(format!("No remote named '{}'.", name)));
    }
    conn.execute("DELETE FROM remote_refs WHERE remote = ?", [name])?;
    println!("{} Removed remote '{}'.", style("✔").green().bold(), name);
    Ok(())
}

// ─── Shared helpers (used by clone / fetch / push / pull) ─────────────────────

/// Look up a remote's URL by name.
pub(crate) fn remote_url(conn: &rusqlite::Connection, name: &str) -> Result<String> {
    conn.query_row("SELECT url FROM remotes WHERE name = ?", [name], |r| r.get(0))
        .map_err(|_| {
            VeloError::InvalidInput(format!(
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
        Err(VeloError::InvalidInput(format!(
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
