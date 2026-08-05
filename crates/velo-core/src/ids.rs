//! The identifiers Velo's API speaks in.
//!
//! These are newtypes over `String`, not aliases, so a branch name cannot be
//! passed where a snapshot id belongs. That mattered enough to be worth the
//! wrapping: before it, `repo.tree_at("main")` compiled and failed at run time,
//! because a resolved id and the text a user typed had the same type.
//!
//! # Ids versus specs
//!
//! A **spec** is what a person types — `"HEAD"`, `"v1.0"`, `"a1b2c3"`, `"main"`,
//! `"origin/main"`. Any string is a plausible attempt at one, so specs stay
//! `&str`: wrapping them would add ceremony and catch nothing.
//!
//! An **id** is the result of resolving a spec. Those are typed, so the only ways
//! to obtain one are to resolve a spec, read one out of the repository, or parse
//! text with validation. "Resolve before you look something up" stops being a
//! convention you can forget:
//!
//! ```no_run
//! # fn main() -> Result<(), velo_core::Error> {
//! let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
//!
//! // repo.tree_at("v1.0") does not compile — that is a spec, not an id.
//! let id = velo_core::commands::resolve_snapshot_id(&repo, "v1.0")?;
//! let tree = repo.tree_at(&id)?;
//! # let _ = tree;
//! # Ok(())
//! # }
//! ```
//!
//! # Why they deref to `str`
//!
//! Each derefs to `str`, so formatting, slicing and comparison work unchanged.
//! That costs nothing in safety — deref only applies where a `&str` is genuinely
//! wanted, and a `SnapshotId` still cannot be passed where a `BranchName` is
//! required — and it keeps every renderer and query readable.

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::ToSql;

use crate::error::{Result, VeloError};

/// Shared plumbing for a validated newtype over `String`.
///
/// The interesting part of each type is its validation; everything else is the
/// same, so it lives here rather than being written out four times.
macro_rules! id_newtype {
    (
        $(#[$meta:meta])*
        $name:ident, $what:literal, validate = $validate:expr
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Wrap `text` without checking it.
            ///
            /// For values coming out of the repository, which are already
            /// whatever they are — validating them on the way out would turn a
            /// corrupt row into a panic somewhere unhelpful. `fsck` is what
            /// reports those.
            pub(crate) fn from_stored(text: impl Into<String>) -> Self {
                $name(text.into())
            }

            /// The underlying text.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume it for the owned `String`.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            /// `f.pad`, not `f.write_str`: these appear in aligned tables, and
            /// `write_str` silently ignores width and alignment, so `{:<20}`
            /// would format correctly for a `String` and do nothing here.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(&self.0)
            }
        }

        // Comparison against text, in both directions, exactly as `String` has.
        // Asking "is this branch `main`?" or checking an id against a value read
        // from SQL is constant, and it gives nothing up: comparing to a literal
        // is not the same as *passing* one where an id belongs.
        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }

        impl PartialEq<$name> for str {
            fn eq(&self, other: &$name) -> bool {
                self == other.0
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                self == &other.0
            }
        }

        impl FromStr for $name {
            type Err = VeloError;

            /// Parse text into an id, rejecting anything that could not be one.
            ///
            /// This is the only public way in, so a typo cannot become an id
            /// silently.
            fn from_str(text: &str) -> Result<Self> {
                let validate: fn(&str) -> bool = $validate;
                if validate(text) {
                    Ok($name(text.to_string()))
                } else {
                    Err(VeloError::invalid(format!(
                        "'{}' is not a valid {}.",
                        text, $what
                    )))
                }
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                match value {
                    ValueRef::Text(_) => Ok($name::from_stored(value.as_str()?)),
                    _ => Err(FromSqlError::InvalidType),
                }
            }
        }
    };
}

id_newtype!(
    /// A resolved snapshot id.
    ///
    /// Obtained from [`commands::resolve_snapshot_id`](crate::commands::resolve_snapshot_id),
    /// from a save, or by parsing hex. Displayed in full; use [`SnapshotId::short`]
    /// for the abbreviated form.
    SnapshotId,
    "snapshot id",
    validate = |text| !text.is_empty() && text.len() <= 64 && is_hex(text)
);

id_newtype!(
    /// The content hash naming an object in the store.
    ///
    /// Full BLAKE3 hex — objects are never addressed by an abbreviation, because
    /// the store is a flat directory keyed on the whole hash.
    ObjectHash,
    "object hash",
    validate = |text| text.len() == 64 && is_hex(text)
);

id_newtype!(
    /// A branch name.
    ///
    /// Slashes are allowed and meaningful (`feature/api`,
    /// `remotes/origin/main`). Rejected: empty, surrounding whitespace, and
    /// control characters, none of which round-trip through the ref files.
    BranchName,
    "branch name",
    validate = is_plain_name
);

id_newtype!(
    /// A tag name. Validated like a branch name.
    TagName,
    "tag name",
    validate = is_plain_name
);

impl SnapshotId {
    /// The abbreviated form used in output.
    ///
    /// Display only: an abbreviation is not an id, which is why this returns
    /// `&str` rather than another `SnapshotId`.
    pub fn short(&self) -> &str {
        &self.0[..crate::commands::SNAP_HASH_LEN.min(self.0.len())]
    }
}

fn is_hex(text: &str) -> bool {
    text.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_plain_name(text: &str) -> bool {
    !text.is_empty() && text.trim() == text && !text.chars().any(|c| c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_id_must_look_like_one() {
        assert!("a1b2c3d4e5f60718".parse::<SnapshotId>().is_ok());
        // The whole point: a branch name is not an id.
        assert!("main".parse::<SnapshotId>().is_err());
        assert!("v1.0".parse::<SnapshotId>().is_err());
        assert!("origin/main".parse::<SnapshotId>().is_err());
        assert!("".parse::<SnapshotId>().is_err());
        assert!("z".repeat(16).parse::<SnapshotId>().is_err());
        assert!("a".repeat(65).parse::<SnapshotId>().is_err());
    }

    #[test]
    fn an_object_hash_is_the_full_width() {
        let full = "a".repeat(64);
        assert!(full.parse::<ObjectHash>().is_ok());
        // An abbreviation cannot name an object: the store is keyed on the whole
        // hash, so a short one would simply not be found.
        assert!("a".repeat(16).parse::<ObjectHash>().is_err());
        assert!("a".repeat(63).parse::<ObjectHash>().is_err());
    }

    #[test]
    fn names_allow_slashes_but_not_junk() {
        assert!("feature/api".parse::<BranchName>().is_ok());
        assert!("remotes/origin/main".parse::<BranchName>().is_ok());
        assert!("_deleted_old".parse::<BranchName>().is_ok());
        assert!("v1.0".parse::<TagName>().is_ok());
        assert!("".parse::<BranchName>().is_err());
        assert!(" leading".parse::<BranchName>().is_err());
        assert!("trailing ".parse::<BranchName>().is_err());
        assert!("has\nnewline".parse::<BranchName>().is_err());
    }

    #[test]
    fn short_abbreviates_without_panicking_on_a_short_id() {
        let id = SnapshotId::from_stored("a1b2c3d4e5f60718");
        assert_eq!(id.short(), "a1b2c3d4e5f60718");
        // A legacy or truncated id must not index out of bounds.
        let stubby = SnapshotId::from_stored("a1b2");
        assert_eq!(stubby.short(), "a1b2");
    }

    #[test]
    fn deref_keeps_formatting_and_slicing_working() {
        let id = SnapshotId::from_stored("a1b2c3d4e5f60718");
        assert_eq!(&id[..4], "a1b2");
        assert_eq!(format!("{}", id), "a1b2c3d4e5f60718");
        assert!(id.starts_with("a1b2"));
        // And it is usable anywhere a &str is wanted.
        fn takes_str(s: &str) -> usize {
            s.len()
        }
        assert_eq!(takes_str(&id), 16);
    }

    #[test]
    fn display_honours_width_and_alignment() {
        // Regression: with `f.write_str` these all came out unpadded, which
        // quietly broke every aligned table that formats an id or a name.
        let tag = TagName::from_stored("v1.0");
        assert_eq!(format!("[{:<8}]", tag), "[v1.0    ]");
        assert_eq!(format!("[{:>8}]", tag), "[    v1.0]");
        assert_eq!(format!("[{:^8}]", tag), "[  v1.0  ]");
        // And it still matches what the equivalent `&str` would produce.
        assert_eq!(format!("{:<8}", tag), format!("{:<8}", "v1.0"));
    }

    #[test]
    fn ids_compare_against_text_both_ways() {
        let branch = BranchName::from_stored("main");
        assert_eq!(branch, "main");
        assert_eq!("main", branch);
        assert_eq!(branch, String::from("main"));
        assert_eq!(String::from("main"), branch);
        assert_ne!(branch, "dev");
    }

    #[test]
    fn distinct_types_do_not_mix() {
        // Not a runtime assertion — the point is that this file compiles while
        // `let _: SnapshotId = BranchName::from_stored("main");` would not.
        let branch = BranchName::from_stored("main");
        let id = SnapshotId::from_stored("a1b2c3d4e5f60718");
        assert_ne!(branch.as_str(), id.as_str());
    }
}
