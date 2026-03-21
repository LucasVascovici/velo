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

pub fn run(root: &Path) -> Result<()> {
    let velo_dir = root.join(".velo");

    // ── Guard: already initialised ───────────────────────────────────────────
    if velo_dir.is_dir() {
        return Err(VeloError::AlreadyInitialized);
    }

    // ── Guard: nested repository ─────────────────────────────────────────────
    // Walk upward from the *parent* of root to detect an enclosing repo.
    {
        let mut check = root.to_path_buf();
        if check.pop() {
            // moved to parent
            loop {
                if check.join(".velo").is_dir() {
                    return Err(VeloError::NestedRepo(check));
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

    // ── Write a default .veloignore if none exists ────────────────────────────
    let veloignore = root.join(".veloignore");
    if !veloignore.exists() {
        fs::write(&veloignore, DEFAULT_VELOIGNORE)?;
    }

    println!(
        "{} Initialized empty Velo repository in {}",
        console::style("✔").green().bold(),
        console::style(velo_dir.display()).cyan()
    );
    println!(
        "  Default .veloignore written. Branch: {}",
        console::style("main").cyan().bold()
    );

    Ok(())
}
