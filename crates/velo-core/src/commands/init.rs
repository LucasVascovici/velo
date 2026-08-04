use std::fs;
use std::path::Path;

use crate::error::{Result, VeloError};

const DEFAULT_VELOIGNORE: &str = "# Velo ignore file — patterns here are excluded from tracking.\n\
# Syntax follows .gitignore rules.\n\
\n\
# Build artefacts\n\
target/\n\
dist/\n\
build/\n\
\n\
# Dependency directories\n\
node_modules/\n\
__pycache__/\n\
.venv/\n\
\n\
# Compiled bytecode\n\
*.pyc\n\
*.pyo\n\
\n\
# Logs & temporary files\n\
*.log\n\
*.tmp\n\
*.swp\n\
\n\
# OS metadata\n\
.DS_Store\n\
Thumbs.db\n\
\n\
# Environment secrets\n\
.env\n\
.env.*\n";

/// A freshly created repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Initialised {
    /// The `.velo` directory that was created.
    pub velo_dir: std::path::PathBuf,
    /// The default branch name.
    pub branch: String,
    /// True when a default `.veloignore` was written (false if one existed).
    pub wrote_veloignore: bool,
}

/// Create a repository at `root`.
pub fn run(root: &Path) -> Result<Initialised> {
    let velo_dir = root.join(".velo");

    // ── Guard: already initialised ───────────────────────────────────────────
    if velo_dir.is_dir() {
        return Err(VeloError::AlreadyInitialized {
            at: root.to_path_buf(),
        });
    }

    // ── Guard: nested repository ─────────────────────────────────────────────
    // Walk upward from the *parent* of root to detect an enclosing repo.
    {
        let mut check = root.to_path_buf();
        if check.pop() {
            // moved to parent
            loop {
                if check.join(".velo").is_dir() {
                    return Err(VeloError::NestedRepo { outer: check });
                }
                if !check.pop() {
                    break;
                }
            }
        }
    }

    // ── Create directory structure ────────────────────────────────────────────
    fs::create_dir_all(velo_dir.join("objects"))?;
    crate::db::init_db_at_path(&velo_dir.join("velo.db"))?;
    fs::write(velo_dir.join("HEAD"), "main")?;
    fs::write(velo_dir.join("PARENT"), "")?;

    // 'main' exists from the moment the repository does, even before the first
    // save — so switching away and back, or merging into it, behaves sanely.
    {
        let conn = crate::db::get_conn_at_path(&velo_dir.join("velo.db"))?;
        crate::commands::register_branch(&conn, "main", "")?;
    }

    // ── Write a default .veloignore if none exists ────────────────────────────
    let veloignore = root.join(".veloignore");
    let wrote_veloignore = !veloignore.exists();
    if wrote_veloignore {
        fs::write(&veloignore, DEFAULT_VELOIGNORE)?;
    }

    Ok(Initialised {
        velo_dir,
        branch: "main".into(),
        wrote_veloignore,
    })
}
