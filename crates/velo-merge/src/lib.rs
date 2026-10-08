//! Pure three-way (diff3) merge engine.
//!
//! No database, no filesystem, no I/O: every entry point takes text and returns
//! text or structured hunks. This is deliberately separable so tools that only
//! need merging can depend on it without pulling in a repository.
//!
//! ```
//! use velo_merge::{diff3, MergeResult};
//! // Two sides editing different regions merge without asking.
//! let r = diff3("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n");
//! assert!(matches!(r, MergeResult::Clean(ref s) if s == "A\nb\nC\n"));
//! ```

use similar::{DiffOp, TextDiff};
use std::ops::Range;

#[cfg(any(feature = "json", feature = "yaml", feature = "toml"))]
mod structured;
#[cfg(feature = "json")]
pub use structured::JsonDriver;
#[cfg(feature = "toml")]
pub use structured::TomlDriver;
#[cfg(feature = "yaml")]
pub use structured::YamlDriver;

// ─── Public types ─────────────────────────────────────────────────────────────

/// How a single conflicting region should be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Keep the current branch's version.
    Ours,
    /// Take the incoming branch's version.
    Theirs,
    /// Keep both, ours first.
    BothOursFirst,
    /// Keep both, theirs first.
    BothTheirsFirst,
    /// Replace with these exact lines.
    Manual(Vec<String>),
}

/// The outcome of a three-way merge.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MergeResult {
    /// The two sides' changes did not overlap; this is the merged content.
    Clean(String),
    /// At least one region needs a human decision.
    Conflicted(Vec<ConflictHunk>),
}

/// A single contiguous conflict region between two branches.
#[derive(Debug, Clone)]
pub struct ConflictHunk {
    pub id: usize,
    /// First line in ancestor covered by this conflict (0-indexed, exclusive end).
    pub ancestor_start: usize,
    pub ancestor_end: usize,
    pub context_before: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub context_after: Vec<String>,
    pub decision: Option<Decision>,
}

/// Merge `ours` and `theirs` against their common `ancestor`.
///
/// Returns [`MergeResult::Clean`] when the two sides touched disjoint regions,
/// otherwise the conflicting hunks for a caller to resolve.
pub fn diff3(ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
    let hunks = compute_conflict_hunks(ancestor, ours, theirs);
    if hunks.is_empty() {
        let anc: Vec<&str> = ancestor.lines().collect();
        let our: Vec<&str> = ours.lines().collect();
        let thr: Vec<&str> = theirs.lines().collect();
        MergeResult::Clean(build_resolved_content(
            &anc,
            &our,
            &thr,
            &[],
            ours.ends_with('\n') || theirs.ends_with('\n'),
        ))
    } else {
        MergeResult::Conflicted(hunks)
    }
}

// ─── Merge drivers ────────────────────────────────────────────────────────────

/// A three-way merge strategy for one kind of file.
///
/// Line-based diff3 is right for prose and source and wrong for structured
/// documents, where a line merge yields syntactically valid conflicts in the
/// wrong places and invalid documents in the right ones. A driver lets a
/// caller merge a kind of file by its own rules while everything else keeps
/// using [`LineDriver`].
pub trait MergeDriver: Send + Sync {
    /// Short stable name, e.g. `line`, `json`.
    fn name(&self) -> &str;
    /// Merge `ours` and `theirs` against `ancestor`.
    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult;
    /// The units diff and blame compare for this kind of file.
    ///
    /// Half-open, 0-based **line** ranges that cover every line of `text`
    /// exactly once, in order. The default is one unit per line; a driver
    /// overrides it so blame can answer per paragraph or per key rather than
    /// per line.
    fn units(&self, text: &str) -> Vec<Range<usize>> {
        (0..text.lines().count()).map(|i| i..i + 1).collect()
    }
}

/// The text of each unit: its lines joined with a line feed, with no trailing newline.
pub fn unit_texts(text: &str, units: &[Range<usize>]) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    units
        .iter()
        .map(|u| lines[u.start.min(lines.len())..u.end.min(lines.len())].join("\n"))
        .collect()
}

/// Turn the sorted line indices at which units begin into ranges over `len`
/// lines. Line 0 always begins a unit, so every line is covered once.
#[cfg(any(feature = "json", feature = "yaml", feature = "toml"))]
pub(crate) fn ranges_from_starts(len: usize, starts: &[usize]) -> Vec<Range<usize>> {
    let mut starts: Vec<usize> = starts.iter().copied().filter(|&s| s < len).collect();
    if len > 0 {
        starts.push(0);
    }
    starts.sort_unstable();
    starts.dedup();
    let mut out = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        out.push(s..starts.get(i + 1).copied().unwrap_or(len));
    }
    out
}

/// The default: line-based diff3, exactly [`diff3`].
#[derive(Clone, Copy, Debug, Default)]
pub struct LineDriver;

impl MergeDriver for LineDriver {
    fn name(&self) -> &str {
        "line"
    }
    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        diff3(ancestor, ours, theirs)
    }
}

/// Line-based diff3 for prose, with paragraphs as the unit of blame and diff.
///
/// Merges exactly like [`LineDriver`]. A unit is a maximal run of non-blank
/// lines, and each run of blank lines is a unit of its own, so a paragraph
/// edit never shifts the attribution of its neighbours.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParagraphDriver;

impl MergeDriver for ParagraphDriver {
    fn name(&self) -> &str {
        "paragraph"
    }
    fn merge(&self, ancestor: &str, ours: &str, theirs: &str) -> MergeResult {
        diff3(ancestor, ours, theirs)
    }
    fn units(&self, text: &str) -> Vec<Range<usize>> {
        let blank: Vec<bool> = text.lines().map(|l| l.trim().is_empty()).collect();
        let starts: Vec<usize> = (0..blank.len())
            .filter(|&i| i == 0 || blank[i] != blank[i - 1])
            .collect();
        let mut out = Vec::new();
        for (k, &s) in starts.iter().enumerate() {
            out.push(s..starts.get(k + 1).copied().unwrap_or(blank.len()));
        }
        out
    }
}

/// One hunk covering the whole file, for a driver that finds a conflict diff3
/// has no line region for.
///
/// A resolver session needs something to decide; this gives it "ours or theirs
/// for the whole file". [`build_resolved_content`] honours such a hunk even
/// though diff3 would have merged the file cleanly.
pub fn whole_file_conflict(ancestor: &str, ours: &str, theirs: &str) -> Vec<ConflictHunk> {
    vec![ConflictHunk {
        id: 0,
        ancestor_start: 0,
        ancestor_end: ancestor.lines().count(),
        context_before: Vec::new(),
        ours: ours.lines().map(str::to_string).collect(),
        theirs: theirs.lines().map(str::to_string).collect(),
        context_after: Vec::new(),
        decision: None,
    }]
}

// ─── 3-way merge (diff3) ───────────────────────────────────────────────────────

/// One segment of a 3-way merge, aligned against the common ancestor.
#[derive(Debug, Clone)]
enum Segment {
    /// Lines identical across all sides, or on which both sides made the same
    /// change. Emitted verbatim.
    Stable(Vec<String>),
    /// Only our side changed this region — take ours automatically.
    OursOnly(Vec<String>),
    /// Only their side changed this region — take theirs automatically.
    TheirsOnly(Vec<String>),
    /// A genuine conflict: both sides changed the same region differently.
    Conflict {
        anc_start: usize,
        anc_end: usize,
        ours: Vec<String>,
        theirs: Vec<String>,
    },
}

/// Map each ancestor line index to its corresponding index on `side`, but only
/// for lines left *unchanged* (part of an `Equal` run). Changed lines map to
/// `None`. These common lines are the anchors the diff3 walk synchronises on.
fn equal_line_map(anc: &[&str], side: &[&str]) -> Vec<Option<usize>> {
    let mut map = vec![None; anc.len()];
    let diff = TextDiff::from_slices(anc, side);
    for op in diff.ops() {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = *op
        {
            for j in 0..len {
                map[old_index + j] = Some(new_index + j);
            }
        }
    }
    map
}

/// Decompose ancestor/ours/theirs into ordered merge segments (classic diff3).
///
/// Lines both sides leave unchanged are *stable anchors*; the slot between two
/// consecutive anchors is resolved by comparing each side's version against the
/// ancestor:
///   * both sides equal            → take either (no real change)
///   * only ours differs           → take ours
///   * only theirs differs         → take theirs
///   * both differ, differently    → conflict
///
/// Crucially, a change made by *theirs* to a region ours did not touch is
/// applied automatically instead of being dropped — the bug the previous
/// ours-only reconstruction had.
fn diff3_segments(ancestor: &str, ours: &str, theirs: &str) -> Vec<Segment> {
    let anc: Vec<&str> = ancestor.lines().collect();
    let our: Vec<&str> = ours.lines().collect();
    let thr: Vec<&str> = theirs.lines().collect();

    let our_map = equal_line_map(&anc, &our);
    let thr_map = equal_line_map(&anc, &thr);

    // Stable anchors: ancestor lines unchanged on BOTH sides. Sentinels at each
    // end let the loop treat the head and tail slots uniformly.
    let mut anchors: Vec<(i64, i64, i64)> = vec![(-1, -1, -1)];
    for k in 0..anc.len() {
        if let (Some(o), Some(t)) = (our_map[k], thr_map[k]) {
            anchors.push((k as i64, o as i64, t as i64));
        }
    }
    anchors.push((anc.len() as i64, our.len() as i64, thr.len() as i64));

    let mut segments = Vec::new();
    for w in anchors.windows(2) {
        let (a_anc, a_our, a_thr) = w[0];
        let (b_anc, b_our, b_thr) = w[1];

        // The slot strictly between the two anchors.
        let anc_start = (a_anc + 1) as usize;
        let anc_end = b_anc as usize; // exclusive
        let anc_slot: Vec<String> = anc[anc_start..anc_end]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let our_slot: Vec<String> = our[(a_our + 1) as usize..b_our as usize]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let thr_slot: Vec<String> = thr[(a_thr + 1) as usize..b_thr as usize]
            .iter()
            .map(|s| s.to_string())
            .collect();

        if !(anc_slot.is_empty() && our_slot.is_empty() && thr_slot.is_empty()) {
            if our_slot == thr_slot {
                segments.push(Segment::Stable(our_slot));
            } else if our_slot == anc_slot {
                segments.push(Segment::TheirsOnly(thr_slot));
            } else if thr_slot == anc_slot {
                segments.push(Segment::OursOnly(our_slot));
            } else {
                segments.push(Segment::Conflict {
                    anc_start,
                    anc_end,
                    ours: our_slot,
                    theirs: thr_slot,
                });
            }
        }

        // Emit the anchor line itself (unless it is the trailing sentinel).
        if (b_anc as usize) < anc.len() {
            segments.push(Segment::Stable(vec![anc[b_anc as usize].to_string()]));
        }
    }

    segments
}

// ─── Hunk computation ─────────────────────────────────────────────────────────

/// Compute the conflicting hunks — the regions both sides changed differently.
/// Non-overlapping changes are auto-merged and never surface here.
pub fn compute_conflict_hunks(ancestor: &str, ours: &str, theirs: &str) -> Vec<ConflictHunk> {
    let anc: Vec<&str> = ancestor.lines().collect();
    let segments = diff3_segments(ancestor, ours, theirs);

    let mut hunks = Vec::new();
    for seg in &segments {
        if let Segment::Conflict {
            anc_start,
            anc_end,
            ours,
            theirs,
        } = seg
        {
            let id = hunks.len();
            let ctx_start = anc_start.saturating_sub(3);
            let ctx_end = (anc_end + 3).min(anc.len());
            hunks.push(ConflictHunk {
                id,
                ancestor_start: *anc_start,
                ancestor_end: *anc_end,
                context_before: anc[ctx_start..*anc_start]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ours: ours.clone(),
                theirs: theirs.clone(),
                context_after: anc[*anc_end..ctx_end]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                decision: None,
            });
        }
    }
    hunks
}

/// Attempt a clean, fully-automatic 3-way merge.
///
/// Returns `Some(merged_text)` when the two sides' changes do not overlap (so no
/// human decision is needed), or `None` when at least one region genuinely
/// conflicts and must be resolved interactively.
///
/// A convenience over [`diff3`] for callers that only act on the clean case and
/// re-derive the hunks later (as `velo-core`'s `reconcile` does). It is a wrapper
/// rather than a second implementation deliberately: the two spent a while as
/// parallel copies of the same walk, which meant either could drift.
pub fn try_auto_merge(ancestor: &str, ours: &str, theirs: &str) -> Option<String> {
    match diff3(ancestor, ours, theirs) {
        MergeResult::Clean(merged) => Some(merged),
        MergeResult::Conflicted(_) => None,
    }
}

/// Produce the final merged file content.
///
/// Non-conflicting changes from *both* sides are applied automatically; each
/// conflicting region uses the decision recorded on its matching hunk. Because
/// the segmentation here is identical to the one `compute_conflict_hunks` used,
/// conflict regions line up with hunks by their ancestor range.
pub fn build_resolved_content(
    anc: &[&str],
    our: &[&str],
    thr: &[&str],
    hunks: &[ConflictHunk],
    trailing_newline: bool,
) -> String {
    use std::collections::HashMap;

    let ancestor = anc.join("\n");
    let ours = our.join("\n");
    let theirs = thr.join("\n");
    let segments = diff3_segments(&ancestor, &ours, &theirs);

    // Decisions keyed by the conflict region they resolve.
    let decisions: HashMap<(usize, usize), &ConflictHunk> = hunks
        .iter()
        .map(|h| ((h.ancestor_start, h.ancestor_end), h))
        .collect();

    // A driver can raise a conflict diff3 has no region for, as one hunk over
    // the whole file. If diff3 produced no conflict with that range, the hunk
    // stands for the entire file and its decision is the result.
    let whole = anc.len();
    if let Some(h) = decisions.get(&(0, whole)) {
        let matched = segments.iter().any(
            |s| matches!(s, Segment::Conflict { anc_start: 0, anc_end, .. } if *anc_end == whole),
        );
        if !matched {
            let joined = hunk_lines(h).join("\n");
            return if trailing_newline && !joined.ends_with('\n') {
                format!("{}\n", joined)
            } else {
                joined
            };
        }
    }

    let mut output: Vec<String> = Vec::new();
    for seg in segments {
        match seg {
            Segment::Stable(lines) | Segment::OursOnly(lines) | Segment::TheirsOnly(lines) => {
                output.extend(lines)
            }
            Segment::Conflict {
                anc_start,
                anc_end,
                ours,
                theirs,
            } => {
                if let Some(h) = decisions.get(&(anc_start, anc_end)) {
                    output.extend(hunk_lines(h));
                } else {
                    // No decision recorded (not reached in normal flow). Emit
                    // both sides rather than silently dropping either.
                    output.extend(ours);
                    output.extend(theirs);
                }
            }
        }
    }

    let joined = output.join("\n");
    if trailing_newline && !joined.ends_with('\n') {
        format!("{}\n", joined)
    } else {
        joined
    }
}

fn hunk_lines(h: &ConflictHunk) -> Vec<String> {
    match h.decision.as_ref().unwrap_or(&Decision::Ours) {
        Decision::Ours => h.ours.clone(),
        Decision::Theirs => h.theirs.clone(),
        Decision::BothOursFirst => h.ours.iter().chain(h.theirs.iter()).cloned().collect(),
        Decision::BothTheirsFirst => h.theirs.iter().chain(h.ours.iter()).cloned().collect(),
        Decision::Manual(ls) => ls.clone(),
    }
}

// ─── DB helpers ───────────────────────────────────────────────────────────────
