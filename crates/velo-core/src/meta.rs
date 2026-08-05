//! App-namespaced metadata attached to a snapshot.
//!
//! Consumers need somewhere to put structured state — which eval run produced
//! this, which tool version wrote it, which upstream ticket it closes. Without a
//! place for it they encode it into the message string or invent a sidecar file,
//! and both rot. This is the place for it.
//!
//! # Metadata is part of the snapshot's identity
//!
//! It is covered by the snapshot hash, which makes it **immutable**: there is no
//! in-place edit, and changing a value produces a different snapshot. That is the
//! whole point. Metadata is mostly provenance, and provenance that can be
//! silently rewritten is worth nothing — see decision D1 in `docs/FORMAT.md`.
//!
//! It also means metadata travels with bundles and sync automatically. It has to:
//! a receiving peer recomputes every id, so metadata that failed to arrive would
//! show up as an id mismatch rather than as missing data.
//!
//! # Ordering is not the caller's problem
//!
//! The hash recipe requires entries sorted by `(namespace, key)`. Rather than
//! documenting that and hoping, the entries live in a `BTreeMap` keyed on exactly
//! that pair, so [`SnapshotMeta::iter`] yields canonical order by construction and
//! two callers inserting the same pairs in different orders get the same id.

use std::collections::BTreeMap;

use crate::error::{Result, VeloError};

/// The namespace Velo keeps for itself.
///
/// Reserved so that a future Velo-level field cannot collide with an app that got
/// there first. Rejected by [`SnapshotMeta::set`].
pub const RESERVED_NAMESPACE: &str = "velo";

/// Structured, app-namespaced key/values attached to one snapshot.
///
/// Namespaces keep unrelated consumers out of each other's way: pick one string
/// (`"promptreg"`, or reverse-DNS if you prefer) and own everything under it.
/// Keys and values are opaque UTF-8 — Velo never interprets them.
///
/// ```
/// # fn main() -> Result<(), velo_core::Error> {
/// let mut meta = velo_core::SnapshotMeta::new();
/// meta.set("promptreg", "eval_run", "2026-08-05T11:04Z")?;
/// meta.set("promptreg", "model", "claude-opus-5")?;
///
/// assert_eq!(meta.get("promptreg", "model"), Some("claude-opus-5"));
/// // Iteration is sorted by (namespace, key), whatever order they went in.
/// let keys: Vec<_> = meta.iter().map(|(_, k, _)| k).collect();
/// assert_eq!(keys, ["eval_run", "model"]);
/// # Ok(()) }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotMeta {
    entries: BTreeMap<(String, String), String>,
}

impl SnapshotMeta {
    /// An empty set.
    ///
    /// Note that "no metadata" and "metadata absent" are the same thing to the
    /// hash: an empty set still emits the section marker, so a snapshot saved
    /// without metadata has one well-defined id.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach `value` under `namespace`/`key`, replacing any previous value.
    ///
    /// # Errors
    /// If `namespace` is empty, is the reserved [`RESERVED_NAMESPACE`], or if any
    /// argument contains a NUL — NUL is the recipe's field separator, so allowing
    /// it would let two different metadata sets hash identically.
    pub fn set(
        &mut self,
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<()> {
        let (namespace, key, value) = (namespace.into(), key.into(), value.into());

        if namespace.is_empty() {
            return Err(VeloError::invalid("A metadata namespace cannot be empty."));
        }
        if namespace == RESERVED_NAMESPACE {
            return Err(VeloError::invalid(format!(
                "The '{}' metadata namespace is reserved. Use your own.",
                RESERVED_NAMESPACE
            )));
        }
        // A NUL would be indistinguishable from the separator the id recipe uses,
        // so two distinct sets could produce one hash.
        for (what, text) in [("namespace", &namespace), ("key", &key), ("value", &value)] {
            if text.contains('\0') {
                return Err(VeloError::invalid(format!(
                    "A metadata {} cannot contain a NUL byte.",
                    what
                )));
            }
        }

        self.entries.insert((namespace, key), value);
        Ok(())
    }

    /// The value at `namespace`/`key`, if it is set.
    pub fn get(&self, namespace: &str, key: &str) -> Option<&str> {
        // Borrowing a (&str, &str) key out of a (String, String) map needs owned
        // keys; metadata sets are small, so the allocation is not worth avoiding.
        self.entries
            .get(&(namespace.to_string(), key.to_string()))
            .map(String::as_str)
    }

    /// Every `(namespace, key, value)`, sorted by `(namespace, key)`.
    ///
    /// This is the order the id recipe hashes in, which is why it is the only
    /// order this type will hand out.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.entries
            .iter()
            .map(|((ns, key), value)| (ns.as_str(), key.as_str(), value.as_str()))
    }

    /// How many pairs are set.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is set.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Insert a row read back out of the repository, unvalidated.
    ///
    /// Rows already are whatever they are; rejecting one here would turn a bad
    /// row into an error in the middle of an unrelated read, which is `fsck`'s
    /// job to report instead.
    pub(crate) fn insert_stored(&mut self, namespace: String, key: String, value: String) {
        self.entries.insert((namespace, key), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iteration_is_canonically_ordered_whatever_the_insert_order() {
        let mut a = SnapshotMeta::new();
        a.set("z_app", "k", "1").unwrap();
        a.set("a_app", "b", "2").unwrap();
        a.set("a_app", "a", "3").unwrap();

        let mut b = SnapshotMeta::new();
        b.set("a_app", "a", "3").unwrap();
        b.set("z_app", "k", "1").unwrap();
        b.set("a_app", "b", "2").unwrap();

        let order: Vec<_> = a.iter().collect();
        assert_eq!(
            order,
            [
                ("a_app", "a", "3"),
                ("a_app", "b", "2"),
                ("z_app", "k", "1")
            ]
        );
        // Same pairs, different insert order, same sequence — so the same id.
        assert_eq!(order, b.iter().collect::<Vec<_>>());
        assert_eq!(a, b);
    }

    #[test]
    fn setting_the_same_pair_twice_replaces_it() {
        let mut meta = SnapshotMeta::new();
        meta.set("app", "k", "old").unwrap();
        meta.set("app", "k", "new").unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta.get("app", "k"), Some("new"));
    }

    #[test]
    fn a_namespace_must_be_usable_and_not_ours() {
        let mut meta = SnapshotMeta::new();
        assert!(meta.set("", "k", "v").is_err());
        assert!(meta.set(RESERVED_NAMESPACE, "k", "v").is_err());
        // Reverse-DNS and plain names are both fine.
        assert!(meta.set("com.example.tool", "k", "v").is_ok());
        assert!(meta.set("promptreg", "k", "v").is_ok());
        // A namespace merely containing the reserved word is not reserved.
        assert!(meta.set("velocity", "k", "v").is_ok());
    }

    #[test]
    fn nul_is_rejected_everywhere_because_it_separates_fields() {
        let mut meta = SnapshotMeta::new();
        assert!(meta.set("a\0b", "k", "v").is_err());
        assert!(meta.set("app", "k\0", "v").is_err());
        assert!(meta.set("app", "k", "v\0w").is_err());
        assert!(meta.is_empty(), "a rejected pair must not be stored");
    }

    #[test]
    fn empty_and_populated_are_distinguishable() {
        let empty = SnapshotMeta::new();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.iter().count(), 0);

        let mut one = SnapshotMeta::new();
        one.set("app", "k", "").unwrap();
        // An empty *value* is a real pair — not the same as no pair.
        assert!(!one.is_empty());
        assert_ne!(one, empty);
    }
}
