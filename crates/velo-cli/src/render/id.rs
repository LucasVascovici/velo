//! Abbreviating ids for output.
//!
//! Format v2 stores snapshot ids whole — 64 hex characters — so every place that
//! prints one has to abbreviate, and they must all abbreviate the same way. Three
//! renderers had their own `short` and four more sliced `[..8]` inline; with v1's
//! 16-character ids the inconsistency was invisible, and with v2's it would have
//! produced a 64-wide column in `velo history`.
//!
//! `docs/FORMAT.md` §8: truncation is a display concern. Never persist one, never
//! put one on the wire, never use one as a key.

use velo_core::commands::SNAP_HASH_LEN;

/// The first [`SNAP_HASH_LEN`] characters of an id.
///
/// Takes `&str` so it works for a `SnapshotId` (which derefs) and for the fields
/// that are still plain text. Shorter input is returned whole rather than
/// panicking — a legacy or hand-edited value must not take the process down in
/// the middle of printing a table.
pub fn short(id: &str) -> &str {
    &id[..SNAP_HASH_LEN.min(id.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviates_to_the_display_width() {
        let full = "a1b2c3d4e5f60718".to_string() + &"0".repeat(48);
        assert_eq!(full.len(), 64);
        assert_eq!(short(&full), "a1b2c3d4e5f60718");
        assert_eq!(short(&full).len(), SNAP_HASH_LEN);
    }

    #[test]
    fn a_short_id_is_returned_whole_rather_than_panicking() {
        assert_eq!(short("a1b2"), "a1b2");
        assert_eq!(short(""), "");
    }
}
