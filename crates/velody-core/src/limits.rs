//! Ceilings on what velody will allocate for data that came from elsewhere.
//!
//! A bundle, a pack on the wire or a git stream is written by someone else, and
//! every length it states is a claim, not a fact. Without a ceiling, a few
//! bytes can ask for gigabytes: a zstd frame that expands a thousandfold, a
//! length prefix that says four billion, a count of a million rows. Each read
//! is therefore bounded by a [`Limits`] value held on the [`Repo`](crate::Repo)
//! handle, and hitting a bound is
//! [`Error::LimitExceeded`](crate::Error::LimitExceeded), never an allocation
//! failure.
//!
//! The ceilings apply to data being *received*. Objects the user saved
//! themselves are read back unbounded, since a file larger than the import
//! ceiling is legitimate to store.

use std::fmt;

/// Which ceiling was hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Limit {
    /// [`Limits::max_object_bytes`].
    ObjectBytes,
    /// [`Limits::max_pack_bytes`].
    PackBytes,
    /// [`Limits::max_string_bytes`].
    StringBytes,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Limit::ObjectBytes => "object size",
            Limit::PackBytes => "pack size",
            Limit::StringBytes => "string length",
        })
    }
}

/// Explicit read ceilings. The defaults are generous for a real repository and
/// hostile to a decompression bomb.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Decompressed size of one received object. Default 1 GiB: larger than
    /// any file a version-controlled project normally holds, small enough that
    /// a hostile frame cannot exhaust memory on its own.
    pub max_object_bytes: u64,
    /// Size of one bundle, pack or request body. Default 16 GiB: a bundle
    /// carries many objects, so this sits well above the per-object ceiling.
    pub max_pack_bytes: u64,
    /// One length-prefixed string: a path, message, branch name or metadata
    /// value. Default 16 MiB: far beyond any honest value, far below a 4 GiB
    /// length prefix.
    pub max_string_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_object_bytes: 1 << 30,
            max_pack_bytes: 16 << 30,
            max_string_bytes: 16 << 20,
        }
    }
}

impl Limits {
    /// Set [`Limits::max_object_bytes`].
    pub fn with_max_object_bytes(mut self, n: u64) -> Self {
        self.max_object_bytes = n;
        self
    }

    /// Set [`Limits::max_pack_bytes`].
    pub fn with_max_pack_bytes(mut self, n: u64) -> Self {
        self.max_pack_bytes = n;
        self
    }

    /// Set [`Limits::max_string_bytes`].
    pub fn with_max_string_bytes(mut self, n: u32) -> Self {
        self.max_string_bytes = n;
        self
    }
}
