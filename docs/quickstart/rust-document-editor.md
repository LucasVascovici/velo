# Rust: version a document editor in five minutes

Use the `velody-core` library to give an editor autosave, history and restore,
with no working tree: snapshots are built from memory.

## Install

Add the crate. It is not on crates.io yet, so depend on it from git at a
release tag (see [INSTALL.md](../INSTALL.md)):

```toml
[dependencies]
velody-core = { git = "https://github.com/LucasVascovici/velody", tag = "vX.Y.Z" }
```

## Save, read the history, restore

```rust quickstart
use velody_core::commands::history;
use velody_core::tree::{SaveChanges, TreeEntry};
use velody_core::{BranchName, Repo};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("velody-quickstart-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let repo = Repo::init(&dir)?;
    let branch: BranchName = "main".parse()?;

    // Autosave: one snapshot per edit, sending only the file that changed.
    let first = repo.write()?.save_changes(
        SaveChanges::new(&branch, None, "first draft")
            .put(TreeEntry::file("doc.md", b"# Title\n".to_vec())),
    )?;
    let second = repo.write()?.save_changes(
        SaveChanges::new(&branch, Some(&first.snapshot), "second draft")
            .put(TreeEntry::file("doc.md", b"# Title\n\nmore text\n".to_vec())),
    )?;

    let log = history::run(&repo, history::Options { branch: Some(&branch), ..Default::default() })?;
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.entries[0].hash, second.snapshot);

    // Restore: read the first draft back from its snapshot.
    let old = repo.read_file_at(&first.snapshot, "doc.md")?;
    assert_eq!(old, b"# Title\n");
    println!("ok: {} snapshots", log.entries.len());

    drop(repo);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
```

The cookbook has the [longer recipe](../cookbook/document-editor.md).
