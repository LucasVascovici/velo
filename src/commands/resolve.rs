//! `velo resolve` — interactive hunk-by-hunk conflict resolver.
//!
//! Non-interactive (--take ours/theirs/--all) is kept for scripting.
//! Interactive mode navigates each conflict file hunk by hunk with a
//! single-keypress decision UI.

use std::fs;
use std::path::Path;

use console::{style, Key, Term};
use rusqlite::params;
use similar::{DiffOp, TextDiff};

use crate::db;
use crate::error::{Result, VeloError};
use crate::storage;

// ─── Public types ─────────────────────────────────────────────────────────────

/// Clap-compatible enum for the --take option.
#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
pub enum TakeOption {
    Ours,
    Theirs,
}

// ─── Internal types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Ours,
    Theirs,
    BothOursFirst,
    BothTheirsFirst,
    Manual(Vec<String>),
}

impl Decision {
    fn to_db(&self) -> (&str, Option<String>) {
        match self {
            Decision::Ours => ("ours", None),
            Decision::Theirs => ("theirs", None),
            Decision::BothOursFirst => ("both_ours", None),
            Decision::BothTheirsFirst => ("both_theirs", None),
            Decision::Manual(lines) => ("manual", Some(lines.join("\n"))),
        }
    }

    fn from_db(kind: &str, content: Option<&str>) -> Option<Self> {
        match kind {
            "ours" => Some(Decision::Ours),
            "theirs" => Some(Decision::Theirs),
            "both_ours" => Some(Decision::BothOursFirst),
            "both_theirs" => Some(Decision::BothTheirsFirst),
            "manual" => Some(Decision::Manual(
                content
                    .unwrap_or("")
                    .lines()
                    .map(|s| s.to_string())
                    .collect(),
            )),
            _ => None,
        }
    }
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

struct ConflictFile {
    path: String,
    ancestor_hash: String,
    our_hash: String,
    their_hash: String,
}

// ─── Entry point ──────────────────────────────────────────────────────────────

pub fn run(root: &Path, file: Option<&str>, take: Option<TakeOption>, all: bool) -> Result<()> {
    // ── Validate: must be in a merge (unless --all on a clean state) ─────────
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;
    let conflict_count: i64 = conn
        .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
        .unwrap_or(0);
    let merge_active = root.join(".velo/MERGE_HEAD").exists() || conflict_count > 0;

    if !merge_active {
        if all {
            // --all with no conflicts is a graceful no-op
            println!("{}", style("No conflicts to resolve.").dim());
            return Ok(());
        }
        return Err(VeloError::InvalidInput(
            "No merge in progress. Nothing to resolve.".into(),
        ));
    }

    if !all && file.is_none() {
        return Err(VeloError::InvalidInput(
            "Specify a file, or use --all.\n  \
             Example: velo resolve src/auth.py\n  \
             Example: velo resolve --all --take theirs"
                .into(),
        ));
    }

    // Collect the conflict files to work on
    let targets: Vec<ConflictFile> = if all {
        load_all_conflict_files(&conn)?
    } else {
        let path = file.unwrap();
        let normalised = db::normalise(path);
        let cf = load_conflict_file(&conn, &normalised)?;
        vec![cf]
    };

    if targets.is_empty() {
        println!("{}", style("No conflict files found.").dim());
        return Ok(());
    }

    // ── Non-interactive: --take applies same decision to all hunks ────────────
    if let Some(take_side) = take {
        for cf in &targets {
            apply_take(root, &conn, cf, &take_side)?;
            remove_conflict_file(&conn, &cf.path)?;
            println!(
                "{} Resolved '{}' (took {}).",
                style("✔").green(),
                cf.path,
                match take_side {
                    TakeOption::Ours => "ours",
                    TakeOption::Theirs => "theirs",
                }
            );
        }
        finish_if_all_resolved(root, &conn)?;
        return Ok(());
    }

    // ── Interactive mode ──────────────────────────────────────────────────────
    for cf in &targets {
        interactive_resolve(root, &conn, cf)?;
    }

    finish_if_all_resolved(root, &conn)?;
    Ok(())
}

// ─── Non-interactive apply ────────────────────────────────────────────────────

fn apply_take(
    root: &Path,
    conn: &rusqlite::Connection,
    cf: &ConflictFile,
    side: &TakeOption,
) -> Result<()> {
    let objects_dir = root.join(".velo/objects");
    let ancestor = read_text(&objects_dir, &cf.ancestor_hash)?;
    let ours = read_text(&objects_dir, &cf.our_hash)?;
    let theirs = read_text(&objects_dir, &cf.their_hash)?;

    let mut hunks = compute_conflict_hunks(&ancestor, &ours, &theirs);

    let decision = match side {
        TakeOption::Ours => Decision::Ours,
        TakeOption::Theirs => Decision::Theirs,
    };
    for h in &mut hunks {
        h.decision = Some(decision.clone());
    }

    let resolved = build_resolved_content(
        &ancestor.lines().collect::<Vec<_>>(),
        &ours.lines().collect::<Vec<_>>(),
        &theirs.lines().collect::<Vec<_>>(),
        &hunks,
        ours.ends_with('\n'),
    );

    let full_path = root.join(db::db_to_path(&cf.path));
    fs::write(&full_path, &resolved)?;

    // Persist decisions
    for h in &hunks {
        let (kind, manual) = h.decision.as_ref().unwrap().to_db();
        conn.execute(
            "INSERT OR REPLACE INTO hunk_decisions
             (file_path, hunk_id, decision, manual_content)
             VALUES (?, ?, ?, ?)",
            params![cf.path, h.id as i64, kind, manual],
        )?;
    }
    Ok(())
}

// ─── Interactive TUI ─────────────────────────────────────────────────────────

fn interactive_resolve(root: &Path, conn: &rusqlite::Connection, cf: &ConflictFile) -> Result<()> {
    let objects_dir = root.join(".velo/objects");
    let ancestor = read_text(&objects_dir, &cf.ancestor_hash)?;
    let ours = read_text(&objects_dir, &cf.our_hash)?;
    let theirs = read_text(&objects_dir, &cf.their_hash)?;

    let mut hunks = compute_conflict_hunks(&ancestor, &ours, &theirs);
    if hunks.is_empty() {
        // No actual conflicts — this file was erroneously in conflict_files
        remove_conflict_file(conn, &cf.path)?;
        return Ok(());
    }

    // Reload any decisions already saved from a previous partial session
    for h in &mut hunks {
        if let Ok((kind, manual)) = conn.query_row(
            "SELECT decision, manual_content FROM hunk_decisions
             WHERE file_path = ? AND hunk_id = ?",
            params![cf.path, h.id as i64],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        ) {
            h.decision = Decision::from_db(&kind, manual.as_deref());
        }
    }

    let term = Term::stdout();
    let mut cursor: usize = 0; // current hunk index

    // MERGE_HEAD stores "pre_merge_hash:source_branch" — extract the branch name
    let merge_info = fs::read_to_string(root.join(".velo/MERGE_HEAD")).unwrap_or_default();
    let source_branch: String = merge_info
        .trim()
        .split_once(':')
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| "(unknown)".into());

    loop {
        let decided = hunks.iter().filter(|h| h.decision.is_some()).count();
        let total = hunks.len();

        // ── Draw the screen ───────────────────────────────────────────────────
        if term.is_term() {
            let _ = term.clear_screen();
        } else {
            println!("{}", "─".repeat(72));
        }

        let hunk = &hunks[cursor];

        println!(
            "  {}  ·  Hunk {}/{}  ·  {} decided  ·  {} ← {}",
            style(&cf.path).cyan().bold(),
            cursor + 1,
            total,
            decided,
            style("main").dim(),
            style(source_branch.trim()).yellow()
        );
        println!("{}", style("─".repeat(72)).dim());

        // Context before
        for line in &hunk.context_before {
            println!("  {}", style(line).dim());
        }

        // Ours (red)
        if hunk.ours.is_empty() {
            println!(
                "  {} {}",
                style("OURS:").red().bold(),
                style("(deleted)").dim()
            );
        } else {
            println!("  {}", style("OURS:").red().bold());
            for line in &hunk.ours {
                println!("    {}", style(format!("- {}", line)).red());
            }
        }

        // Theirs (green)
        if hunk.theirs.is_empty() {
            println!(
                "  {} {}",
                style("THEIRS:").green().bold(),
                style("(deleted)").dim()
            );
        } else {
            println!("  {}", style("THEIRS:").green().bold());
            for line in &hunk.theirs {
                println!("    {}", style(format!("+ {}", line)).green());
            }
        }

        // Context after
        for line in &hunk.context_after {
            println!("  {}", style(line).dim());
        }

        // Current decision badge
        if let Some(ref d) = hunk.decision {
            let badge = match d {
                Decision::Ours => style("[✔ OURS]").red().bold().to_string(),
                Decision::Theirs => style("[✔ THEIRS]").green().bold().to_string(),
                Decision::BothOursFirst => {
                    style("[✔ BOTH (ours·theirs)]").yellow().bold().to_string()
                }
                Decision::BothTheirsFirst => {
                    style("[✔ BOTH (theirs·ours)]").yellow().bold().to_string()
                }
                Decision::Manual(_) => style("[✔ MANUAL]").cyan().bold().to_string(),
            };
            println!("\n  Decided: {}", badge);
        }

        println!("{}", style("─".repeat(72)).dim());
        println!(
            "  [1] Keep ours   [2] Take theirs   [3] Both (ours·theirs)   [4] Both (theirs·ours)"
        );
        println!(
            "  [e] Edit in $EDITOR   [n/→] Next   [p/←] Prev   [u] Undo   [q] Quit without saving"
        );

        // ── Wait for keypress ─────────────────────────────────────────────────
        let key = if term.is_term() {
            term.read_key().map_err(VeloError::Io)?
        } else {
            // Non-TTY fallback: read a char from stdin
            let mut buf = String::new();
            std::io::stdin().read_line(&mut buf).ok();
            match buf.trim() {
                "1" => Key::Char('1'),
                "2" => Key::Char('2'),
                "3" => Key::Char('3'),
                "4" => Key::Char('4'),
                "e" => Key::Char('e'),
                "n" => Key::Char('n'),
                "p" => Key::Char('p'),
                "u" => Key::Char('u'),
                "q" => Key::Char('q'),
                _ => Key::Char('n'),
            }
        };

        let decision = match key {
            Key::Char('1') => Some(Decision::Ours),
            Key::Char('2') => Some(Decision::Theirs),
            Key::Char('3') => Some(Decision::BothOursFirst),
            Key::Char('4') => Some(Decision::BothTheirsFirst),
            Key::Char('e') => {
                let manual = open_in_editor(root, cf, hunk)?;
                manual.map(Decision::Manual)
            }
            Key::Char('u') | Key::Backspace => {
                // Undo: clear current hunk's decision
                hunks[cursor].decision = None;
                conn.execute(
                    "DELETE FROM hunk_decisions WHERE file_path = ? AND hunk_id = ?",
                    params![cf.path, cursor as i64],
                )
                .ok();
                None
            }
            Key::Char('n') | Key::ArrowRight => {
                cursor = (cursor + 1).min(total - 1);
                None
            }
            Key::Char('p') | Key::ArrowLeft => {
                cursor = cursor.saturating_sub(1);
                None
            }
            Key::Char('q') | Key::Escape => {
                println!("\n{} Quit — no changes written.", style("!").yellow());
                return Ok(());
            }
            _ => None,
        };

        // Apply the decision to the current hunk
        if let Some(d) = decision {
            let (kind, manual_content) = d.to_db();
            conn.execute(
                "INSERT OR REPLACE INTO hunk_decisions
                 (file_path, hunk_id, decision, manual_content)
                 VALUES (?, ?, ?, ?)",
                params![cf.path, cursor as i64, kind, manual_content],
            )?;
            hunks[cursor].decision = Decision::from_db(kind, manual_content.as_deref());

            // Auto-advance to next undecided hunk
            let next_undecided = hunks.iter().position(|h| h.decision.is_none());
            cursor = next_undecided.unwrap_or(total.saturating_sub(1));
        }

        // Check if all hunks decided
        if hunks.iter().all(|h| h.decision.is_some()) {
            // Write the resolved file
            let anc_lines: Vec<&str> = ancestor.lines().collect();
            let our_lines: Vec<&str> = ours.lines().collect();
            let thr_lines: Vec<&str> = theirs.lines().collect();

            let resolved = build_resolved_content(
                &anc_lines,
                &our_lines,
                &thr_lines,
                &hunks,
                ours.ends_with('\n'),
            );
            let full_path = root.join(db::db_to_path(&cf.path));
            fs::write(&full_path, &resolved)?;

            remove_conflict_file(conn, &cf.path)?;

            if term.is_term() {
                let _ = term.clear_screen();
            }
            println!(
                "{} All {} hunk(s) resolved — '{}' written.",
                style("✔").green().bold(),
                total,
                cf.path
            );
            break;
        }
    }

    Ok(())
}

// ─── Editor integration ───────────────────────────────────────────────────────

fn open_in_editor(
    _root: &Path,
    cf: &ConflictFile,
    hunk: &ConflictHunk,
) -> Result<Option<Vec<String>>> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });

    // Write a temp file with both versions and a resolution zone
    let tmp_dir = std::env::temp_dir();
    let filename = cf.path.replace(['/', '\\'], "_");
    let tmp_path = tmp_dir.join(format!("velo_hunk_{}.txt", filename));

    let mut content = "# VELO conflict hunk — edit the RESOLUTION section below, then save and exit.\n\
         # Do NOT change the lines starting with '#'.\n\
         #\n\
         # ── OURS ─────────────────────────────────────────────────────────────\n".to_string();
    for line in &hunk.ours {
        content.push_str(&format!("# {}\n", line));
    }
    content.push_str("# ── THEIRS ──────────────────────────────────────────────────────────\n");
    for line in &hunk.theirs {
        content.push_str(&format!("# {}\n", line));
    }
    content.push_str("# ── RESOLUTION (edit below) ─────────────────────────────────────────\n");
    // Pre-fill with ours as a starting point
    for line in &hunk.ours {
        content.push_str(&format!("{}\n", line));
    }

    fs::write(&tmp_path, &content)?;

    // Open editor
    let status = std::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()
        .map_err(VeloError::Io)?;

    if !status.success() {
        println!(
            "{} Editor exited with non-zero status.",
            style("!").yellow()
        );
        return Ok(None);
    }

    // Read back — everything after the last "# ── RESOLUTION" line
    let edited = fs::read_to_string(&tmp_path).map_err(VeloError::Io)?;
    let _ = fs::remove_file(&tmp_path);

    let resolution_marker =
        "# ── RESOLUTION (edit below) ─────────────────────────────────────────";
    if let Some(pos) = edited.find(resolution_marker) {
        let after = &edited[pos + resolution_marker.len()..];
        let lines: Vec<String> = after
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect();
        Ok(Some(lines))
    } else {
        println!(
            "{} Could not find resolution marker in edited file.",
            style("!").yellow()
        );
        Ok(None)
    }
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
        let anc_slot: Vec<String> = anc[anc_start..anc_end].iter().map(|s| s.to_string()).collect();
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
pub fn try_auto_merge(ancestor: &str, ours: &str, theirs: &str) -> Option<String> {
    if !compute_conflict_hunks(ancestor, ours, theirs).is_empty() {
        return None;
    }
    let anc: Vec<&str> = ancestor.lines().collect();
    let our: Vec<&str> = ours.lines().collect();
    let thr: Vec<&str> = theirs.lines().collect();
    Some(build_resolved_content(
        &anc,
        &our,
        &thr,
        &[],
        ours.ends_with('\n') || theirs.ends_with('\n'),
    ))
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

fn load_all_conflict_files(conn: &rusqlite::Connection) -> Result<Vec<ConflictFile>> {
    let mut stmt = conn.prepare(
        "SELECT path, ancestor_hash, our_hash, their_hash FROM conflict_files ORDER BY path",
    )?;
    let rows: Vec<ConflictFile> = stmt
        .query_map([], |r| {
            Ok(ConflictFile {
                path: r.get(0)?,
                ancestor_hash: r.get(1)?,
                our_hash: r.get(2)?,
                their_hash: r.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn load_conflict_file(conn: &rusqlite::Connection, path: &str) -> Result<ConflictFile> {
    conn.query_row(
        "SELECT path, ancestor_hash, our_hash, their_hash
         FROM conflict_files WHERE path = ?",
        [path],
        |r| {
            Ok(ConflictFile {
                path: r.get(0)?,
                ancestor_hash: r.get(1)?,
                our_hash: r.get(2)?,
                their_hash: r.get(3)?,
            })
        },
    )
    .map_err(|_| VeloError::InvalidInput(format!("No conflict found for '{}'.", path)))
}

fn remove_conflict_file(conn: &rusqlite::Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM conflict_files  WHERE path      = ?", [path])?;
    conn.execute("DELETE FROM hunk_decisions  WHERE file_path = ?", [path])?;
    Ok(())
}

fn finish_if_all_resolved(_root: &Path, conn: &rusqlite::Connection) -> Result<()> {
    let remaining: i64 = conn
        .query_row("SELECT count(*) FROM conflict_files", [], |r| r.get(0))
        .unwrap_or(0);

    if remaining == 0 {
        // Do NOT delete MERGE_HEAD here — keep it alive until `velo save`.
        // This lets the user still run `velo merge --abort` to undo the whole
        // merge even after resolving all conflicts but before committing.
        println!(
            "\n{} All conflicts resolved! Run {} to finalise.",
            style("✔").green().bold(),
            style("velo save \"Merge <branch>\"").yellow().bold()
        );
        println!(
            "  {} to cancel the merge entirely.",
            style("velo merge --abort").dim()
        );
    } else {
        println!(
            "\n{} {} conflict file(s) still unresolved.",
            style("!").yellow().bold(),
            remaining
        );
        let mut stmt = conn.prepare("SELECT path FROM conflict_files ORDER BY path")?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        for p in &paths {
            println!("  {}", style(p).yellow());
        }
    }
    Ok(())
}

fn read_text(objects_dir: &Path, hash: &str) -> Result<String> {
    if hash.is_empty() {
        return Ok(String::new());
    }
    let bytes = storage::read_object(objects_dir, hash)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
