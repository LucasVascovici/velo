//! `velo blame <file>` — attribute each line to the snapshot that last changed it.
//!
//! Algorithm: walk history from HEAD backwards; for each snapshot diff it against
//! its parent. Any line that snapshot introduced is attributed to it, mapped back
//! onto the line numbering of the file as it stands at the starting snapshot.
//! Lines already attributed are left alone, so the first (newest) snapshot to
//! touch a line wins.
//!
//! Returns attributions as data; formatting lives in `velo-cli`.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::params;
use similar::{ChangeTag, TextDiff};

use crate::db;
use crate::error::{RefKind, Result, VeloError};
use crate::storage;
use crate::Repo;

/// The snapshot a line came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineOrigin {
    /// Full snapshot hash. Abbreviate at the point of display.
    pub hash: String,
    /// Raw stored timestamp, RFC-3339-like. Formatting is the consumer's choice.
    pub created_at: String,
    pub message: String,
}

/// One line of the file with its attribution.
#[derive(Clone, Debug)]
pub struct BlameLine {
    /// 1-based line number in the file as of the starting snapshot.
    pub line_no: usize,
    pub text: String,
    /// `None` when history didn't explain this line — a truncated or grafted
    /// ancestry, rather than a normal outcome.
    pub origin: Option<LineOrigin>,
}

/// Attribution for a whole file.
#[derive(Clone, Debug)]
pub struct Blame {
    /// Normalised path that was blamed.
    pub path: String,
    /// The snapshot the file was read at.
    pub snapshot: String,
    pub lines: Vec<BlameLine>,
}

impl Blame {
    /// True when some lines could not be attributed to any snapshot.
    pub fn has_unattributed(&self) -> bool {
        self.lines.iter().any(|l| l.origin.is_none())
    }
}

/// Blame `file` as it stands at `at` (defaulting to the current position).
pub fn run(repo: &Repo, file: &str, at: Option<&str>) -> Result<Blame> {
    let root = repo.root();
    let conn = repo.conn();
    let objects_dir = root.join(".velo/objects");
    let rel = db::normalise(file);

    let start_hash = match at {
        Some(t) => crate::commands::resolve_snapshot_id(repo, t)?.into_string(),
        None => std::fs::read_to_string(root.join(".velo/PARENT"))
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    if start_hash.is_empty() {
        let branch = std::fs::read_to_string(root.join(".velo/HEAD"))
            .unwrap_or_else(|_| "main".into())
            .trim()
            .to_string();
        return Err(VeloError::UnbornBranch { branch });
    }

    let tip_object: String = conn
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
            params![start_hash, rel],
            |r| r.get(0),
        )
        .map_err(|_| {
            VeloError::invalid(format!(
                "'{}' is not tracked in snapshot {}.",
                file,
                &start_hash[..8]
            ))
        })?;

    let tip_text = read_text(&objects_dir, &tip_object)?;
    let total_lines = tip_text.lines().count();

    // Indexed by line number in `tip_text`; filled as the walk goes back in time.
    let mut origins: Vec<Option<LineOrigin>> = vec![None; total_lines];
    let mut remaining = total_lines;

    let mut walk_hash = start_hash.clone();
    while remaining > 0 {
        let Ok((parent_hash, created_at, message)) = conn.query_row(
            "SELECT parent_hash, created_at, message FROM snapshots WHERE hash = ?",
            [&walk_hash],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ) else {
            break; // ancestry ends here
        };
        let (parent_hash, created_at, message): (String, String, String) =
            (parent_hash, created_at, message);

        let origin = LineOrigin {
            hash: walk_hash.clone(),
            created_at,
            message,
        };

        let our_text = read_tracked(conn, &objects_dir, &walk_hash, &rel)?;
        let par_text = read_tracked(conn, &objects_dir, &parent_hash, &rel)?;

        // Lines this snapshot introduced, translated into tip line numbers.
        // The map must run our→tip: the old code built it tip→our and queried it
        // with `our` indices, which attributed lines to the wrong snapshot as soon
        // as an ancestor's line offsets differed from the tip's.
        let our_to_tip = equal_line_map(&tip_text, &our_text);
        for our_idx in introduced_lines(&par_text, &our_text) {
            if let Some(&tip_idx) = our_to_tip.get(&our_idx) {
                if let Some(slot) = origins.get_mut(tip_idx) {
                    if slot.is_none() {
                        *slot = Some(origin.clone());
                        remaining -= 1;
                    }
                }
            }
        }

        if parent_hash.is_empty() {
            // The root snapshot owns whatever is still unattributed.
            for slot in origins.iter_mut().filter(|s| s.is_none()) {
                *slot = Some(origin.clone());
            }
            break;
        }
        walk_hash = parent_hash;
    }

    let lines = tip_text
        .lines()
        .enumerate()
        .map(|(i, text)| BlameLine {
            line_no: i + 1,
            text: text.to_string(),
            origin: origins[i].take(),
        })
        .collect();

    Ok(Blame {
        path: rel,
        snapshot: start_hash,
        lines,
    })
}

/// Content of `path` at `snapshot`, or empty when it isn't tracked there.
fn read_tracked(
    conn: &rusqlite::Connection,
    objects_dir: &Path,
    snapshot: &str,
    path: &str,
) -> Result<String> {
    let object: Option<String> = conn
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
            params![snapshot, path],
            |r| r.get(0),
        )
        .ok();
    match object {
        Some(h) => read_text(objects_dir, &h),
        None => Ok(String::new()),
    }
}

fn read_text(objects_dir: &Path, object: &str) -> Result<String> {
    let bytes = storage::read_object(objects_dir, object)
        .map_err(|_| VeloError::not_found(RefKind::Snapshot, object))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Map each `new` line index to the `old` line index it is unchanged from.
fn equal_line_map(old: &str, new: &str) -> HashMap<usize, usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let diff = TextDiff::from_slices(&old_lines, &new_lines);
    let mut map = HashMap::new();
    for op in diff.ops() {
        if let similar::DiffOp::Equal {
            old_index,
            new_index,
            ..
        } = op
        {
            for i in 0..op.old_range().len() {
                map.insert(new_index + i, old_index + i);
            }
        }
    }
    map
}

/// Indices of `new` lines that were not present in `old`.
fn introduced_lines(old: &str, new: &str) -> Vec<usize> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    TextDiff::from_slices(&old_lines, &new_lines)
        .iter_all_changes()
        .filter(|c| c.tag() == ChangeTag::Insert)
        .filter_map(|c| c.new_index())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `our` gains a line at the front, so its indices are offset from the tip's.
    /// Querying the map in the wrong direction silently mis-attributes lines.
    #[test]
    fn map_translates_ancestor_indices_to_tip_indices() {
        let tip = "Z\nA\nB\n";
        let our = "A\nB\n";
        let map = equal_line_map(tip, our);
        // our line 0 ("A") is tip line 1; our line 1 ("B") is tip line 2.
        assert_eq!(map.get(&0), Some(&1));
        assert_eq!(map.get(&1), Some(&2));
    }

    #[test]
    fn introduced_lines_reports_only_additions() {
        assert_eq!(introduced_lines("A\n", "A\nB\n"), vec![1]);
        assert_eq!(introduced_lines("A\n", "Z\nA\n"), vec![0]);
        assert!(introduced_lines("A\nB\n", "A\nB\n").is_empty());
        // A deletion introduces nothing.
        assert!(introduced_lines("A\nB\n", "A\n").is_empty());
    }

    #[test]
    fn a_line_present_in_neither_is_not_mapped() {
        let map = equal_line_map("A\n", "B\n");
        assert!(map.is_empty());
    }
}
