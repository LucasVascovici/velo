//! Formatting timestamps for output.
//!
//! Five renderers each had their own `short_date` that truncated the stored text
//! at a hand-picked width — 10, 16, 19 characters — which worked only because the
//! stored form happened to be `YYYY-MM-DD HH:MM:SS.mmm`. Format v2 stores epoch
//! milliseconds, so there is no text to slice, and the widths that used to be
//! implicit are named here instead.
//!
//! Times are rendered in UTC, matching how they are stored. Local-time display
//! would be a nicer default for a human, but it would make two people's output
//! for the same snapshot disagree, which is worse for a tool whose whole job is
//! shared history.

use chrono::{DateTime, Utc};

/// `YYYY-MM-DD` — for listings where the time of day is noise.
pub fn date(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// `YYYY-MM-DD HH:MM` — enough to tell two of the day's snapshots apart.
pub fn minutes(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M").to_string()
}

/// `YYYY-MM-DD HH:MM:SS` — full precision, for showing one snapshot.
pub fn seconds(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use velo_core::commands::timestamp_from_ms;

    /// 2026-08-05T09:41:12.345Z
    const WHEN: i64 = 1_785_922_872_345;

    #[test]
    fn each_width_shows_what_it_says_it_does() {
        let at = timestamp_from_ms(WHEN);
        assert_eq!(date(at), "2026-08-05");
        assert_eq!(minutes(at), "2026-08-05 09:41");
        assert_eq!(seconds(at), "2026-08-05 09:41:12");
    }

    #[test]
    fn widths_are_fixed_so_columns_line_up() {
        // The old helpers sliced to a width and silently produced a short string
        // for a short input, which quietly broke alignment. These cannot.
        for ms in [0, WHEN, -1, i64::MAX] {
            let at = timestamp_from_ms(ms);
            assert_eq!(date(at).len(), 10);
            assert_eq!(minutes(at).len(), 16);
            assert_eq!(seconds(at).len(), 19);
        }
    }
}
