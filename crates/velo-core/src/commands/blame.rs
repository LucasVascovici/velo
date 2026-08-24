//! `velo blame <file>` — attribute each line to the snapshot that last changed it.
//!
//! Walk history back from a starting snapshot; at each one, work out which lines
//! it *introduced* — the lines none of its parents had — and credit them, mapped
//! onto the line numbering of the file as it stands at the start. Lines already
//! attributed are left alone, so the first (newest) snapshot to touch a line wins.
//!
//! Returns attributions as data; formatting lives in `velo-cli`.
//!
//! # Why the walk is a queue
//!
//! It used to follow first parents, and that lost two things.
//!
//! **Merges.** A merge's diff against its first parent contains everything the
//! absorbed branch wrote, so every one of those lines was credited to the merge:
//! the wrong time, the wrong message and — since [`LineOrigin`] gained an author
//! — the wrong person. Where drafts are the normal way to write, content arrives
//! by merge as a matter of course, so this was the common path rather than an
//! edge case.
//!
//! The fix is to ask *which parent explains this line* rather than diffing
//! against one of them: a line is the merge's own only if **no** parent had it,
//! which is exactly a conflict resolution. Everything else keeps walking down
//! whichever parent has it. That makes the walk a queue over a graph — newest
//! first, so the nearest explanation wins — rather than a chain.
//!
//! **Renames.** A snapshot that renamed a file has, on the old path, nothing at
//! all; the parent text came back empty and every line looked introduced, so
//! blame stopped dead at the rename and credited the whole file to it. The path
//! is now re-resolved per step from the recorded rename edges — see
//! [`crate::commands::paths`] — so the walk follows the file rather than the
//! name.

use chrono::{DateTime, Utc};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use rusqlite::params;
use similar::{ChangeTag, TextDiff};

use crate::db;
use crate::error::{RefKind, Result, VeloError};
use crate::meta::Author;
use crate::progress::{Cancel, Observer, Phase, PhaseGuard};
use crate::storage;
use crate::BranchName;
use crate::Repo;
use crate::SnapshotId;

/// The snapshot a line came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineOrigin {
    /// Full snapshot id. Abbreviate at the point of display.
    pub hash: SnapshotId,
    /// Raw stored timestamp, RFC-3339-like. Formatting is the consumer's choice.
    pub created_at: DateTime<Utc>,
    pub message: String,
    /// Who recorded it, when the snapshot carries an author.
    ///
    /// Resolved here rather than left to the caller: blame already walks this
    /// history, and every consumer that wants "who wrote this line" would
    /// otherwise write the same per-snapshot lookup and the same deduplication.
    /// Read once per snapshot visited, not once per line.
    pub author: Option<Author>,
    /// Branch the snapshot was recorded on — display context, as in
    /// [`history::Entry`](crate::commands::history::Entry), and there for the
    /// same reason as `author`: the column is in the row the walk already reads.
    ///
    /// A branch is not part of a snapshot's identity, so this says where the
    /// snapshot was *recorded*, not where it can be reached from. That is
    /// exactly what a consumer showing which draft a line came from wants.
    pub branch: BranchName,
    /// The path the file had at this snapshot.
    ///
    /// Differs from [`Blame::path`] once the walk has crossed a rename, which is
    /// the only way a consumer can tell that it did.
    pub path: PathBuf,
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
    /// Normalised path that was blamed, as of [`Blame::snapshot`].
    pub path: PathBuf,
    /// The snapshot the file was read at.
    pub snapshot: SnapshotId,
    /// The attributed lines. Restricted to [`Options::lines`] when one was
    /// given, so `line_no` is not necessarily `1..=len()`.
    pub lines: Vec<BlameLine>,
}

impl Blame {
    /// True when some lines could not be attributed to any snapshot.
    pub fn has_unattributed(&self) -> bool {
        self.lines.iter().any(|l| l.origin.is_none())
    }

    /// True when the walk followed the file through a rename.
    pub fn crossed_a_rename(&self) -> bool {
        self.lines
            .iter()
            .filter_map(|l| l.origin.as_ref())
            .any(|o| o.path != self.path)
    }
}

/// What to blame, and how.
#[derive(Clone, Default)]
pub struct Options<'a> {
    /// Blame the file as it stood at this snapshot.
    ///
    /// `None` means the current position, falling back to the tip of the
    /// checked-out branch. The fallback is the point: the position lives in
    /// `.velo/PARENT`, which [`save_tree`](crate::tree::SaveTree) deliberately
    /// never writes — so without it the default failed for exactly the
    /// consumers that API exists for.
    pub at: Option<&'a SnapshotId>,
    /// Attribute only these lines, 1-based and half-open, in the file as of
    /// [`at`](Options::at). `None` is the whole file.
    ///
    /// Worth using whenever the answer is: the walk reads and diffs two whole
    /// file texts per snapshot and stops once every requested line is explained,
    /// so a window ends it far sooner. Blaming a gutter's worth of a document
    /// whose first line is original otherwise walks the entire history.
    ///
    /// Out-of-range ends are clamped; a start past the end of the file yields no
    /// lines rather than an error, because a viewport scrolled past a file that
    /// shrank is not a mistake worth failing over.
    pub lines: Option<Range<usize>>,
    /// Where to report progress, overriding the repository's own observer.
    pub observer: Option<&'a dyn Observer>,
    /// Checked between snapshots. A cancelled blame returns
    /// [`Error::Cancelled`](crate::Error::Cancelled) rather than a partial
    /// attribution, which would be indistinguishable from a real answer.
    pub cancel: Option<&'a Cancel>,
}

/// Attribute each line of `file` to the snapshot that last changed it.
///
/// ```no_run
/// # fn main() -> Result<(), velo_core::Error> {
/// # let repo = velo_core::Repo::discover(std::path::Path::new("."))?;
/// use velo_core::commands::blame;
///
/// // The whole file, at the current position.
/// let all = blame::run(&repo, std::path::Path::new("notes.md"), Default::default())?;
///
/// // Just what a viewport is showing.
/// let visible = blame::run(&repo, std::path::Path::new("notes.md"), blame::Options {
///     lines: Some(40..60),
///     ..Default::default()
/// })?;
/// # let _ = (all, visible);
/// # Ok(()) }
/// ```
pub fn run(repo: &Repo, file: &Path, options: Options<'_>) -> Result<Blame> {
    let Options {
        at,
        lines,
        observer,
        cancel,
    } = options;
    let root = repo.root();
    let conn = repo.conn();
    let objects_dir = root.join(".velo/objects");
    let rel = db::normalise(&file.to_string_lossy());

    let start_hash = match at {
        Some(id) => id.to_string(),
        None => default_start(repo)?,
    };

    let tip_object: String = conn
        .query_row(
            "SELECT hash FROM file_map WHERE snapshot_hash = ? AND path = ?",
            params![start_hash, rel],
            |r| r.get(0),
        )
        .map_err(|_| {
            VeloError::invalid(format!(
                "'{}' is not tracked in snapshot {}.",
                file.display(),
                SnapshotId::from_stored(start_hash.clone()).short()
            ))
        })?;

    let tip_text = read_text(&objects_dir, &tip_object)?;
    let total_lines = tip_text.lines().count();
    // Half-open and clamped, so a viewport hanging off the end of a file that
    // shrank asks for nothing rather than failing.
    let window = match lines {
        Some(r) => {
            r.start.saturating_sub(1).min(total_lines)..r.end.saturating_sub(1).min(total_lines)
        }
        None => 0..total_lines,
    };

    // Indexed by line number in `tip_text`; only the requested window is
    // tracked, which is what lets a windowed blame stop early.
    let mut origins: HashMap<usize, LineOrigin> = HashMap::new();
    let mut remaining = window.len();

    let progress = PhaseGuard::cancellable(
        observer.unwrap_or_else(|| repo.observer()),
        Phase::Tracing,
        None,
        cancel,
    );

    // Newest first, so the nearest snapshot that explains a line is the one that
    // gets it. `Visit` orders by timestamp; the id breaks ties so the walk is
    // deterministic when two snapshots share a millisecond.
    let mut queue: BinaryHeap<Visit> = BinaryHeap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    if let Some(v) = visit_of(conn, &start_hash, &rel) {
        seen.insert((start_hash.clone(), rel.clone()));
        queue.push(v);
    }

    while remaining > 0 {
        progress.check()?;
        let Some(visit) = queue.pop() else { break };
        progress.tick();

        let our_text = read_tracked(conn, &objects_dir, &visit.hash, &visit.path)?;

        // The path each parent knew the file by. A rename recorded on this
        // snapshot means the parents held the old name; without this the parent
        // text comes back empty, every line looks new, and the walk stops here
        // crediting the whole file to whoever moved it.
        let parent_path = crate::commands::paths::renamed_from(conn, &visit.hash, &visit.path)
            .unwrap_or_else(|| visit.path.clone());
        let parents: Vec<(String, String)> = visit
            .parents
            .iter()
            .map(|p| (p.clone(), parent_path.clone()))
            .collect();

        // A line is this snapshot's own only when no parent had it. Against one
        // parent that is the old rule; against two it is what separates a
        // conflict resolution, which the merge really did write, from the
        // absorbed branch's work, which it did not.
        let mut introduced: Option<HashSet<usize>> = None;
        for (parent, parent_path) in &parents {
            let parent_text = read_tracked(conn, &objects_dir, parent, parent_path)?;
            let against: HashSet<usize> = introduced_lines(&parent_text, &our_text)
                .into_iter()
                .collect();
            introduced = Some(match introduced {
                Some(so_far) => &so_far & &against,
                None => against,
            });
        }
        // No parents: the file starts here, so everything in it is its own.
        let introduced =
            introduced.unwrap_or_else(|| (0..our_text.lines().count()).collect::<HashSet<_>>());

        if !introduced.is_empty() {
            let origin = LineOrigin {
                hash: SnapshotId::from_stored(visit.hash.clone()),
                created_at: crate::commands::timestamp_from_ms(visit.created_at_ms),
                message: visit.message.clone(),
                branch: BranchName::from_stored(&visit.branch),
                // A missing or unreadable author is absence, not failure:
                // authorship is optional, and a blame that refused to render
                // because one old snapshot had no metadata would be worse than
                // one that says nothing about that snapshot.
                author: crate::commands::load_snapshot_meta(conn, &visit.hash)
                    .ok()
                    .and_then(|m| m.author()),
                path: PathBuf::from(visit.path.clone()),
            };
            // Translated into tip line numbers: the map must run our→tip, and
            // querying it the other way attributes lines to the wrong snapshot
            // as soon as an ancestor's offsets differ from the tip's.
            let our_to_tip = equal_line_map(&tip_text, &our_text);
            for our_idx in introduced {
                let Some(&tip_idx) = our_to_tip.get(&our_idx) else {
                    continue;
                };
                if window.contains(&tip_idx) && !origins.contains_key(&tip_idx) {
                    origins.insert(tip_idx, origin.clone());
                    remaining -= 1;
                }
            }
        }

        for (parent, parent_path) in parents {
            if seen.insert((parent.clone(), parent_path.clone())) {
                if let Some(v) = visit_of(conn, &parent, &parent_path) {
                    queue.push(v);
                }
            }
        }
    }
    drop(progress);

    let lines = tip_text
        .lines()
        .enumerate()
        .filter(|(i, _)| window.contains(i))
        .map(|(i, text)| BlameLine {
            line_no: i + 1,
            text: text.to_string(),
            origin: origins.remove(&i),
        })
        .collect();

    Ok(Blame {
        path: PathBuf::from(rel),
        snapshot: SnapshotId::from_stored(start_hash),
        lines,
    })
}

/// One snapshot waiting to be examined, newest first.
struct Visit {
    hash: String,
    path: String,
    message: String,
    branch: String,
    created_at_ms: i64,
    parents: Vec<String>,
}

impl Ord for Visit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // `BinaryHeap` is a max-heap, so this orders newest-first as written.
        // The id is the tiebreak, because two snapshots can share a millisecond
        // and an order that depends on which row SQLite handed back first is not
        // an order.
        self.created_at_ms
            .cmp(&other.created_at_ms)
            .then_with(|| self.hash.cmp(&other.hash))
    }
}
impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Visit {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl Eq for Visit {}

fn visit_of(conn: &rusqlite::Connection, hash: &str, path: &str) -> Option<Visit> {
    let (message, branch, created_at_ms, parent, merge_parent) = conn
        .query_row(
            "SELECT message, branch, created_at_ms, parent_hash, merge_parent
             FROM snapshots WHERE hash = ?",
            [hash],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .ok()?;
    Some(Visit {
        hash: hash.to_string(),
        path: path.to_string(),
        message,
        branch,
        created_at_ms,
        parents: [parent, merge_parent]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect(),
    })
}

/// Where a blame starts when the caller did not say.
fn default_start(repo: &Repo) -> Result<String> {
    let root = repo.root();
    let position = std::fs::read_to_string(root.join(".velo/PARENT"))
        .unwrap_or_default()
        .trim()
        .to_string();
    if !position.is_empty() {
        return Ok(position);
    }
    // No working-tree position — which is the normal state for a consumer built
    // on `save_tree`, not an error. The branch a tip is derived for is still
    // recorded, so there is a defensible answer.
    let branch = BranchName::from_stored(
        std::fs::read_to_string(root.join(".velo/HEAD"))
            .unwrap_or_else(|_| "main".into())
            .trim(),
    );
    match repo.branch_tip(&branch)? {
        Some(tip) => Ok(tip.into_string()),
        None => Err(VeloError::UnbornBranch {
            branch: branch.into_string(),
        }),
    }
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
