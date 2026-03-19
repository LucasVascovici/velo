pub mod init;
pub mod save;
pub mod restore;
pub mod status;
pub mod logs;
pub mod undo;
pub mod redo;
pub mod diff;
pub mod switch;
pub mod tag;
pub mod merge;
pub mod resolve;
pub mod branches;
pub mod gc;

use ignore::WalkBuilder;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use rayon::prelude::*;

use crate::error::{Result, VeloError};

/// Number of hex characters used for snapshot hashes (48 bits of entropy).
pub const SNAP_HASH_LEN: usize = 12;

// ─── Status ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Clone)]
pub enum FileStatus {
    New,
    Modified,
    Deleted,
}

// ─── Repository discovery ────────────────────────────────────────────────────

/// Walk upward from `start` until a directory containing `.velo/` is found.
/// Returns `None` if no repository is found all the way to the filesystem root.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".velo").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve a user-supplied snapshot identifier (tag name OR hash prefix) to a
/// full hash stored in the database.
pub fn resolve_snapshot_id(root: &Path, input: &str) -> Result<String> {
    let conn = crate::db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    // 1. Try as a tag name
    if let Ok(h) = conn.query_row(
        "SELECT snapshot_hash FROM tags WHERE name = ?",
        [input],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(h);
    }

    // 2. Try as an exact or prefix hash
    let rows: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT hash FROM snapshots WHERE hash LIKE ? || '%'",
        )?;
        let collected: Vec<String> = stmt
            .query_map([input], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    match rows.len() {
        0 => Err(VeloError::InvalidInput(format!(
            "No snapshot or tag found matching '{}'.",
            input
        ))),
        1 => Ok(rows.into_iter().next().unwrap()),
        _ => Err(VeloError::InvalidInput(format!(
            "Ambiguous prefix '{}' matches {} snapshots. Use more characters.",
            input,
            rows.len()
        ))),
    }
}

// ─── File enumeration ────────────────────────────────────────────────────────

/// Return all tracked files under `root` (respecting `.veloignore` /
/// `.gitignore`).  Conflict files (`.conflict` extension) are always excluded
/// since they are ephemeral merge artefacts, not repository content.
pub fn get_tracked_files(root: &Path) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .add_custom_ignore_filename(".veloignore")
        .add_custom_ignore_filename(".gitignore");
    builder.filter_entry(|e| {
        let n = e.file_name().to_str().unwrap_or("");
        n != ".velo" && n != ".git" && n != "target" && !n.ends_with(".conflict")
    });
    builder
        .build()
        .filter_map(|r| r.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.into_path())
        .collect()
}

/// Return a map of `rel_path -> FileStatus` for every file that differs from
/// the current snapshot (PARENT).  Paths are normalised to forward-slashes.
///
/// Complexity: one DB round-trip + one filesystem walk.
pub fn get_dirty_files(root: &Path) -> HashMap<String, FileStatus> {
    let mut dirty = HashMap::new();
    let db_path = root.join(".velo/velo.db");
    if !db_path.exists() {
        return dirty;
    }

    let conn = match crate::db::get_conn_at_path(&db_path) {
        Ok(c) => c,
        Err(_) => return dirty,
    };
    let parent_hash = std::fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();

    let mut stmt = conn
        .prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ?")
        .unwrap();
    let mut db_files: HashMap<String, String> = stmt
        .query_map([parent_hash.trim()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let tracked = get_tracked_files(root);
    
    let results: Vec<(String, String)> = tracked.into_par_iter().map(|path: std::path::PathBuf| {
        let rel = crate::db::normalise(path.strip_prefix(root).unwrap().to_str().unwrap());
        let current_hash = stream_hash(&path);
        (rel, current_hash)
    }).collect();

    for (rel, current_hash) in results {
        if let Some(db_hash) = db_files.remove(&rel) {
            if db_hash != current_hash {
                dirty.insert(rel, FileStatus::Modified);
            }
        } else {
            dirty.insert(rel, FileStatus::New);
        }
    }

    for rel in db_files.into_keys() {
        dirty.insert(rel, FileStatus::Deleted);
    }
    dirty
}

/// Return the list of active `.conflict` files (only meaningful while a merge
/// is in progress).
pub fn get_conflict_files(root: &Path) -> Vec<String> {
    let merge_head = root.join(".velo/MERGE_HEAD");
    if !merge_head.exists() {
        return vec![];
    }
    let mut out = Vec::new();
    if let Ok(entries) = walkdir_all(root) {
        for p in entries {
            if p.extension().map(|e| e == "conflict").unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(crate::db::normalise(rel.to_str().unwrap_or("")));
                }
            }
        }
    }
    out.sort();
    out
}

fn walkdir_all(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let n = name.to_str().unwrap_or("");
        if n == ".velo" || n == ".git" {
            continue;
        }
        if path.is_dir() {
            if let Ok(mut sub) = walkdir_all(&path) {
                out.append(&mut sub);
            }
        } else {
            out.push(path);
        }
    }
    Ok(out)
}

// ─── Hashing ─────────────────────────────────────────────────────────────────

/// Stream-hash a file without loading it entirely into memory.
pub fn stream_hash(path: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        let _ = std::io::copy(&mut file, &mut hasher);
    }
    hasher.finalize().to_hex().to_string()
}

/// Return `true` if the file likely contains binary data (null byte in first
/// 1 KiB).  Prevents terminal corruption during diff output.
pub fn is_binary(path: &Path) -> bool {
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut buf = [0u8; 1024];
        if let Ok(n) = file.read(&mut buf) {
            return buf[..n].contains(&0);
        }
    }
    false
}

// ─── Filesystem helpers ───────────────────────────────────────────────────────

/// Remove the directory `dir` and all empty ancestors up to (but not
/// including) `root`.  Silently ignores non-empty directories and errors.
pub fn remove_empty_parents(dir: &Path, root: &Path) {
    let mut current = dir.to_path_buf();
    loop {
        if current == root {
            break;
        }
        match std::fs::remove_dir(&current) {
            Ok(_) => {}
            Err(_) => break, // Non-empty or error — stop climbing
        }
        if !current.pop() {
            break;
        }
    }
}