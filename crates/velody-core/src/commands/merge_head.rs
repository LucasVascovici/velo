//! The `.velo/MERGE_HEAD` file: what a merge, cherry-pick or rebase in
//! progress is combining.
//!
//! The text is `<pre-merge-id>:<source>:<source-id>`. `source` is the name the
//! user typed (or `cherry-pick/<8hex>` / `rebase/<8hex>`), kept for messages.
//! `source-id` is the snapshot that name resolved to *when the merge ran*. The
//! id is what save and abort act on: a name such as `origin/main` can move
//! (a `velody fetch` between the merge and the save) and the merge would then
//! record a second parent its tree does not contain.
//!
//! A merge started on 6.0.1 wrote only `<pre-merge-id>:<source>`. That form is
//! still read, with `source_id` empty, and callers fall back to resolving the
//! name as they always did. This is working state, not repository format.

use std::path::Path;

use crate::error::Result;
use crate::ids::SnapshotId;
use crate::storage;

/// A parsed `MERGE_HEAD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeHead {
    /// The snapshot the working tree was at before the merge started.
    pub pre: String,
    /// The source's display name.
    pub source: String,
    /// The resolved source snapshot, absent in the 6.0.1 two-field form.
    pub source_id: Option<String>,
}

fn path(root: &Path) -> std::path::PathBuf {
    root.join(".velo/MERGE_HEAD")
}

fn parse(text: &str) -> MergeHead {
    let text = text.trim();
    let (pre, rest) = text.split_once(':').unwrap_or((text, ""));
    if let Some((source, tail)) = rest.rsplit_once(':') {
        if tail.parse::<SnapshotId>().is_ok() {
            return MergeHead {
                pre: pre.to_string(),
                source: source.to_string(),
                source_id: Some(tail.to_string()),
            };
        }
    }
    MergeHead {
        pre: pre.to_string(),
        source: rest.trim().to_string(),
        source_id: None,
    }
}

/// Read `MERGE_HEAD`, or `None` when absent or unreadable.
pub(crate) fn read(root: &Path) -> Option<MergeHead> {
    std::fs::read_to_string(path(root)).ok().map(|t| parse(&t))
}

/// Write `MERGE_HEAD` atomically in the three-field form.
pub(crate) fn write(root: &Path, head: &MergeHead) -> Result<()> {
    let text = match &head.source_id {
        Some(id) => format!("{}:{}:{}", head.pre.trim(), head.source, id),
        None => format!("{}:{}", head.pre.trim(), head.source),
    };
    Ok(storage::write_atomic(&path(root), text.as_bytes())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_parse() {
        let id = "a".repeat(crate::commands::SNAP_ID_LEN);
        let new = parse(&format!("pre:origin/main:{id}"));
        assert_eq!(new.source, "origin/main");
        assert_eq!(new.source_id.as_deref(), Some(id.as_str()));
        let old = parse("pre:origin/main");
        assert_eq!(old.source, "origin/main");
        assert_eq!(old.source_id, None);
    }
}
