//! `velo grep <pattern>` — search tracked files for a regex or literal pattern.
//!
//! Searches the working tree by default. With a snapshot, searches the stored
//! content at that snapshot without touching the working tree.
//!
//! Returns matches as data — including the byte ranges that matched, so a
//! consumer can highlight them however it likes. Rendering lives in `velo-cli`.

use std::path::Path;

use rusqlite::params;

use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;
use crate::{Repo, SnapshotId};

/// One line of output: either a match or a context line around one.
#[derive(Clone, Debug)]
pub struct MatchLine {
    /// 1-based line number in the file.
    pub line_no: usize,
    pub text: String,
    /// True when the pattern matched on this line, false for context.
    pub is_match: bool,
    /// Byte ranges within `text` covered by the pattern. Empty for context lines.
    pub spans: Vec<(usize, usize)>,
    /// True when lines were skipped between this line and the previous one, so a
    /// consumer can draw an elision marker.
    pub gap_before: bool,
}

/// Every match in one file.
#[derive(Clone, Debug)]
pub struct FileMatches {
    pub path: String,
    /// Matches with their context. Empty when the search asked for names only.
    pub lines: Vec<MatchLine>,
}

/// The snapshot a search was run against.
#[derive(Clone, Debug)]
pub struct SearchedSnapshot {
    pub hash: SnapshotId,
    pub message: String,
}

/// Everything `velo grep` found.
#[derive(Clone, Debug)]
pub struct GrepResults {
    /// The pattern as the user typed it.
    pub pattern: String,
    /// `None` when the working tree was searched.
    pub snapshot: Option<SearchedSnapshot>,
    /// Files with at least one match, in path order.
    pub files: Vec<FileMatches>,
}

impl GrepResults {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Total number of matching lines across all files.
    pub fn match_count(&self) -> usize {
        self.files
            .iter()
            .map(|f| f.lines.iter().filter(|l| l.is_match).count())
            .sum()
    }
}

/// Search for `pattern`.
///
/// `context` is the number of lines to include either side of a match;
/// `names_only` skips collecting them entirely.
/// How to search.
///
/// A struct rather than three unlabelled booleans and a bare `usize`:
/// `run(repo, pat, None, false, true, 2)` said nothing at the call site about
/// which flag was which.
#[derive(Clone, Debug, Default)]
pub struct Options<'a> {
    /// Search this snapshot instead of the working tree.
    pub snapshot: Option<&'a SnapshotId>,
    /// Match without regard to case.
    pub case_insensitive: bool,
    /// Report only the paths that matched, not the lines.
    pub names_only: bool,
    /// Lines of context to include either side of a match.
    pub context: usize,
}

/// Search for `pattern`.
pub fn run(repo: &Repo, pattern: &str, options: Options<'_>) -> Result<GrepResults> {
    let Options {
        snapshot,
        case_insensitive,
        names_only,
        context,
    } = options;
    let re = build_regex(pattern, case_insensitive)?;

    let (searched, files) = match snapshot {
        Some(target) => {
            let (snap, files) = grep_snapshot(repo, &re, target, names_only, context)?;
            (Some(snap), files)
        }
        None => (
            None,
            grep_working_tree(repo.root(), &re, names_only, context),
        ),
    };

    Ok(GrepResults {
        pattern: pattern.to_string(),
        snapshot: searched,
        files,
    })
}

// ── Working tree search ────────────────────────────────────────────────────────

fn grep_working_tree(
    root: &Path,
    re: &regex::Regex,
    names_only: bool,
    context: usize,
) -> Vec<FileMatches> {
    let mut out = Vec::new();
    for entry in crate::commands::walk_with_meta(root) {
        let rel = db::normalise(entry.path.strip_prefix(root).unwrap().to_str().unwrap());
        // Unreadable as text means binary; nothing to search.
        let Ok(content) = std::fs::read_to_string(&entry.path) else {
            continue;
        };
        if let Some(lines) = collect_matches(&content, re, names_only, context) {
            out.push(FileMatches { path: rel, lines });
        }
    }
    out
}

// ── Snapshot search ────────────────────────────────────────────────────────────

fn grep_snapshot(
    repo: &Repo,
    re: &regex::Regex,
    target: &str,
    names_only: bool,
    context: usize,
) -> Result<(SearchedSnapshot, Vec<FileMatches>)> {
    let conn = repo.conn();
    let objects_dir = repo.root().join(".velo/objects");

    let hash = crate::commands::resolve_snapshot_id(repo, target)?;
    let message: String = conn
        .query_row(
            "SELECT message FROM snapshots WHERE hash = ?",
            [&hash],
            |r| r.get(0),
        )
        .map_err(|_| VeloError::not_found(crate::error::RefKind::Snapshot, target))?;

    let mut stmt =
        conn.prepare("SELECT path, hash FROM file_map WHERE snapshot_hash = ? ORDER BY path")?;
    let files: Vec<(String, String)> = stmt
        .query_map(params![hash], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut out = Vec::new();
    for (path, object) in files {
        let Ok(bytes) = storage::read_object(&objects_dir, &object) else {
            continue;
        };
        let Ok(content) = String::from_utf8(bytes) else {
            continue; // binary
        };
        if let Some(lines) = collect_matches(&content, re, names_only, context) {
            out.push(FileMatches { path, lines });
        }
    }

    Ok((SearchedSnapshot { hash, message }, out))
}

// ── Match collection ───────────────────────────────────────────────────────────

/// Find matches in `content`, expanded by `context` lines either side.
///
/// Returns `None` when nothing matched. Overlapping context windows are merged so
/// no line appears twice; `gap_before` marks where lines were skipped.
fn collect_matches(
    content: &str,
    re: &regex::Regex,
    names_only: bool,
    context: usize,
) -> Option<Vec<MatchLine>> {
    let lines: Vec<&str> = content.lines().collect();
    let matching: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| re.is_match(l))
        .map(|(i, _)| i)
        .collect();

    if matching.is_empty() {
        return None;
    }
    if names_only {
        return Some(Vec::new());
    }

    let mut out: Vec<MatchLine> = Vec::new();
    // Highest line index emitted so far, to merge windows and spot gaps.
    let mut highest: Option<usize> = None;

    for &mi in &matching {
        let start = mi.saturating_sub(context);
        let end = (mi + context + 1).min(lines.len());
        let gap = highest.is_some_and(|last| start > last + 1);

        // `i` is a line number used for numbering, dedup and match lookup — not
        // just for indexing — so an index loop reads clearer than enumerate().
        #[allow(clippy::needless_range_loop)]
        for i in start..end {
            if highest.is_some_and(|last| i <= last) {
                continue; // already emitted by an earlier window
            }
            // A line is a match on its own merits, not because it happens to be
            // the one this window was built around — a match swallowed by an
            // earlier window's context is still a match.
            let is_match = matching.binary_search(&i).is_ok();
            out.push(MatchLine {
                line_no: i + 1,
                text: lines[i].to_string(),
                is_match,
                spans: if is_match {
                    re.find_iter(lines[i])
                        .map(|m| (m.start(), m.end()))
                        .collect()
                } else {
                    Vec::new()
                },
                gap_before: gap && i == start,
            });
            highest = Some(i);
        }
    }

    Some(out)
}

fn build_regex(pattern: &str, case_insensitive: bool) -> Result<regex::Regex> {
    let p = if case_insensitive {
        format!("(?i){}", pattern)
    } else {
        pattern.to_string()
    };
    regex::Regex::new(&p)
        .map_err(|e| VeloError::invalid(format!("Invalid regex '{}': {}", pattern, e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(content: &str, pattern: &str, context: usize) -> Vec<MatchLine> {
        collect_matches(
            content,
            &build_regex(pattern, false).unwrap(),
            false,
            context,
        )
        .unwrap()
    }

    /// A match inside an earlier match's context window is still a match.
    /// It used to be rendered as plain context, unhighlighted.
    #[test]
    fn overlapping_windows_keep_every_match_marked() {
        let lines = find("hit\nb\nc\nd\nhit\nf\nhit\n", "hit", 2);
        let marked: Vec<usize> = lines
            .iter()
            .filter(|l| l.is_match)
            .map(|l| l.line_no)
            .collect();
        assert_eq!(marked, vec![1, 5, 7]);
        // Every marked line carries the span needed to highlight it.
        assert!(lines
            .iter()
            .filter(|l| l.is_match)
            .all(|l| !l.spans.is_empty()));
    }

    #[test]
    fn lines_are_never_emitted_twice() {
        let lines = find("hit\nb\nhit\nd\nhit\n", "hit", 3);
        let nos: Vec<usize> = lines.iter().map(|l| l.line_no).collect();
        assert_eq!(nos, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn gap_is_marked_only_where_lines_were_skipped() {
        let content = "hit\nb\nc\nd\ne\nf\nhit\n";
        let lines = find(content, "hit", 0);
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].gap_before);
        assert!(lines[1].gap_before, "lines 2-6 were skipped");
    }

    #[test]
    fn spans_cover_every_occurrence_on_a_line() {
        let lines = find("x hit y hit z\n", "hit", 0);
        assert_eq!(lines[0].spans, vec![(2, 5), (8, 11)]);
    }

    #[test]
    fn no_match_returns_none() {
        let re = build_regex("zzz", false).unwrap();
        assert!(collect_matches("a\nb\n", &re, false, 2).is_none());
    }

    #[test]
    fn names_only_skips_line_collection() {
        let re = build_regex("hit", false).unwrap();
        let lines = collect_matches("hit\n", &re, true, 2).unwrap();
        assert!(lines.is_empty(), "names-only should not collect lines");
    }

    #[test]
    fn invalid_regex_is_an_input_error() {
        let err = build_regex("[", false).unwrap_err();
        assert!(matches!(err, VeloError::InvalidInput { .. }));
    }
}
