use std::fs;
use std::path::Path;

use crate::error::{Result, VelodyError};

const DEFAULT_VELOIGNORE: &str =
    "# Velody ignore file — patterns here are excluded from tracking.\n\
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
    pub velody_dir: std::path::PathBuf,
    /// The default branch name.
    pub branch: String,
    /// True when a default `.veloignore` was written (false if one existed).
    pub wrote_veloignore: bool,
}

/// Choices made when a repository is created.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct InitOptions {
    /// Where objects are kept. Recorded in the repository and fixed for its
    /// lifetime.
    pub objects: crate::ObjectLocation,
}

impl InitOptions {
    /// The defaults: objects in files.
    pub fn new() -> Self {
        Self::default()
    }

    /// Choose where objects are kept.
    pub fn objects(mut self, location: crate::ObjectLocation) -> Self {
        self.objects = location;
        self
    }
}

/// Create a repository at `root`.
pub fn run(root: &Path) -> Result<Initialised> {
    run_with(root, InitOptions::default())
}

/// Create a repository at `root` with `options`.
pub fn run_with(root: &Path, options: InitOptions) -> Result<Initialised> {
    let velody_dir = root.join(".velo");

    // ── Guard: already initialised ───────────────────────────────────────────
    if velody_dir.is_dir() {
        return Err(VelodyError::AlreadyInitialized {
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
                    return Err(VelodyError::NestedRepo { outer: check });
                }
                if !check.pop() {
                    break;
                }
            }
        }
    }

    // ── Create directory structure ────────────────────────────────────────────
    fs::create_dir_all(&velody_dir)?;
    if options.objects == crate::ObjectLocation::Files {
        fs::create_dir_all(velody_dir.join("objects"))?;
        fs::create_dir_all(velody_dir.join("chunks"))?;
    }
    let conn = crate::db::create_at_path(&velody_dir.join("velo.db"))?;
    fs::write(velody_dir.join("HEAD"), "main")?;
    fs::write(velody_dir.join("PARENT"), "")?;

    // 'main' exists from the moment the repository does, even before the first
    // save — so switching away and back, or merging into it, behaves sanely.
    {
        crate::commands::register_branch(&conn, "main", "")?;
        if options.objects != crate::ObjectLocation::Files {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('objects', ?)",
                [options.objects.as_setting()],
            )?;
        }
    }

    // ── Write a default .veloignore if none exists ────────────────────────────
    let veloignore = root.join(".veloignore");
    let wrote_veloignore = !veloignore.exists();
    if wrote_veloignore {
        fs::write(&veloignore, DEFAULT_VELOIGNORE)?;
    }

    Ok(Initialised {
        velody_dir,
        branch: "main".into(),
        wrote_veloignore,
    })
}
