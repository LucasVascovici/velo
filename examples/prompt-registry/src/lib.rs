//! A versioned prompt registry, built on `velo-core` with no working tree.
//!
//! This exists to answer a question the test suite cannot: is `velo-core`
//! actually pleasant to embed? It is written the way a real consumer would write
//! it — only the public API, no shelling out to the `velo` binary, no scratch
//! directory — and the friction found along the way is recorded in `FINDINGS.md`
//! next to this file rather than smoothed over silently.
//!
//! # The model
//!
//! Each prompt is a path (`prompts/<name>.txt`) inside one snapshot, and every
//! publish records a whole new snapshot on the `registry` branch. Provenance —
//! which model the prompt targets, which eval run scored it — goes in snapshot
//! metadata, where it is covered by the snapshot hash and therefore cannot be
//! rewritten after the fact.
//!
//! ```no_run
//! # fn main() -> Result<(), prompt_registry::RegistryError> {
//! let mut reg = prompt_registry::Registry::create(std::path::Path::new("./reg"))?;
//!
//! let v1 = reg.publish("summarise", "Summarise the following:", "claude-opus-5")?;
//! let v2 = reg.publish("summarise", "Summarise concisely:", "claude-opus-5")?;
//!
//! assert_eq!(reg.get_at(&v1, "summarise")?, "Summarise the following:");
//! assert_eq!(reg.latest("summarise")?, "Summarise concisely:");
//! # let _ = v2;
//! # Ok(()) }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use velo_core::commands;
use velo_core::tree::{SaveTree, TreeEntry};
use velo_core::{BranchName, Repo, SnapshotId, SnapshotMeta, TagName};

/// The branch every registry snapshot is recorded on.
///
/// A name of our own, so the registry can share a repository with anything else
/// without the two disturbing each other's branch tips.
const BRANCH: &str = "registry";

/// The metadata namespace this consumer owns.
const NAMESPACE: &str = "promptreg";

/// What can go wrong. Velo's errors are re-wrapped so callers of *this* crate
/// never have to know what is underneath.
#[derive(Debug)]
pub enum RegistryError {
    /// No prompt by that name exists at the version asked for.
    NoSuchPrompt(String),
    /// The registry has no snapshots yet.
    Empty,
    /// A prompt name that cannot be a path component.
    BadName(String),
    /// Anything the underlying store reported.
    Store(velo_core::Error),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NoSuchPrompt(name) => write!(f, "no prompt named '{}'", name),
            RegistryError::Empty => write!(f, "the registry is empty"),
            RegistryError::BadName(name) => write!(f, "'{}' is not a usable prompt name", name),
            RegistryError::Store(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<velo_core::Error> for RegistryError {
    fn from(e: velo_core::Error) -> Self {
        RegistryError::Store(e)
    }
}

type Result<T> = std::result::Result<T, RegistryError>;

/// One published version of a prompt.
#[derive(Clone, Debug)]
pub struct Version {
    pub id: SnapshotId,
    pub message: String,
    pub at: chrono::DateTime<chrono::Utc>,
    /// The model this version targeted, from the snapshot's metadata.
    pub model: Option<String>,
}

/// A registry backed by one Velo repository.
pub struct Registry {
    repo: Repo,
    branch: BranchName,
}

impl Registry {
    /// Create a registry at `root`, or open it if one is already there.
    pub fn create(root: &Path) -> Result<Self> {
        let repo = match Repo::init(root) {
            Ok(repo) => repo,
            // Being handed an existing directory is normal for a library; the
            // typed error is what makes this recoverable without string-matching.
            Err(velo_core::Error::AlreadyInitialized { .. }) => Repo::open_and_migrate(root)?,
            Err(e) => return Err(e.into()),
        };
        Ok(Registry {
            repo,
            branch: BRANCH.parse()?,
        })
    }

    /// Open an existing registry, refusing to create one.
    pub fn open(root: &Path) -> Result<Self> {
        Ok(Registry {
            repo: Repo::open_and_migrate(root)?,
            branch: BRANCH.parse()?,
        })
    }

    /// Publish a new version of `name`, returning the snapshot recording it.
    ///
    /// Every other prompt is carried forward unchanged: a snapshot is a whole
    /// tree, not a diff, so publishing one prompt means rewriting the tree with
    /// that one entry replaced.
    pub fn publish(&mut self, name: &str, body: &str, model: &str) -> Result<SnapshotId> {
        let path = prompt_path(name)?;

        // Carry forward everything already published, then overwrite one entry.
        // The carried entries reference objects the store already holds, so this
        // costs no decompression, no rehashing and no memory proportional to the
        // registry — only the one prompt being published is new content.
        let parent = self.tip()?;
        let mut entries: BTreeMap<String, TreeEntry> = match &parent {
            Some(tip) => self
                .repo
                .tree_at(tip)?
                .into_iter()
                .map(|f| (f.path.clone(), TreeEntry::stored(f.path, f.object, f.kind)))
                .collect(),
            None => BTreeMap::new(),
        };
        entries.insert(
            path.clone(),
            TreeEntry::file(path, body.as_bytes().to_vec()),
        );

        let mut meta = SnapshotMeta::new();
        meta.set(NAMESPACE, "prompt", name)?;
        meta.set(NAMESPACE, "model", model)?;

        let guard = self.repo.write()?;
        let id = guard.save_tree(SaveTree {
            branch: self.branch.clone(),
            parent: parent.as_ref(),
            message: &format!("publish {}", name),
            entries: entries.into_values().collect(),
            meta,
        })?;
        Ok(id)
    }

    /// Tag a version, so it can be fetched by a name that means something.
    pub fn release(&mut self, tag: &str, version: &SnapshotId) -> Result<()> {
        let tag: TagName = tag.parse()?;
        let guard = self.repo.write()?;
        commands::tag::create(&guard, &tag, Some(version.as_str()), true)?;
        Ok(())
    }

    /// The body of `name` as of `version`.
    pub fn get_at(&self, version: &SnapshotId, name: &str) -> Result<String> {
        let path = prompt_path(name)?;
        let bytes = self
            .repo
            .read_file_at(version, &path)
            .map_err(|_| RegistryError::NoSuchPrompt(name.to_string()))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The body of `name` at the newest version.
    pub fn latest(&self, name: &str) -> Result<String> {
        let tip = self.tip()?.ok_or(RegistryError::Empty)?;
        self.get_at(&tip, name)
    }

    /// The body of `name` at whatever `spec` resolves to — a tag, a hash or
    /// prefix, or a branch.
    pub fn get(&self, spec: &str, name: &str) -> Result<String> {
        let id = commands::resolve_snapshot_id(&self.repo, spec)?;
        self.get_at(&id, name)
    }

    /// Every prompt name published as of the newest version.
    pub fn list(&self) -> Result<Vec<String>> {
        let tip = match self.tip()? {
            Some(tip) => tip,
            None => return Ok(Vec::new()),
        };
        Ok(self
            .repo
            .tree_at(&tip)?
            .into_iter()
            .filter_map(|f| prompt_name(&f.path))
            .collect())
    }

    /// Every version that touched `name`, newest first.
    ///
    /// Metadata is what makes this answerable: without it we would be parsing the
    /// commit message, which is exactly what metadata exists to avoid.
    pub fn versions(&self, name: &str) -> Result<Vec<Version>> {
        let history = commands::history::run(
            &self.repo,
            commands::history::Options {
                branch: Some(&self.branch),
                ..Default::default()
            },
        )?;

        let mut out = Vec::new();
        for entry in history.entries {
            let id = entry.hash;
            let meta = self.repo.snapshot_meta(&id)?;
            if meta.get(NAMESPACE, "prompt") == Some(name) {
                out.push(Version {
                    model: meta.get(NAMESPACE, "model").map(str::to_string),
                    id,
                    message: entry.message,
                    at: entry.created_at,
                });
            }
        }
        Ok(out)
    }

    /// The newest snapshot on the registry branch, if there is one.
    fn tip(&self) -> Result<Option<SnapshotId>> {
        match commands::resolve_snapshot_id(&self.repo, BRANCH) {
            Ok(id) => Ok(Some(id)),
            Err(velo_core::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn prompt_path(name: &str) -> Result<String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(RegistryError::BadName(name.to_string()));
    }
    Ok(format!("prompts/{}.txt", name))
}

fn prompt_name(path: &str) -> Option<String> {
    path.strip_prefix("prompts/")
        .and_then(|rest| rest.strip_suffix(".txt"))
        .map(str::to_string)
}
