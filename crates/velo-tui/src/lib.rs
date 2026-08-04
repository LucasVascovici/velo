//! Interactive hunk-by-hunk conflict resolver.
//!
//! Terminal interaction lives here, not in `velo-core`: core exposes conflicts as
//! data and applies decisions, this crate walks a human through them. A consumer
//! with its own UI (an editor, a GUI) can ignore this crate entirely and call the
//! core API directly.

use console::{style, Key, Term};
use velo_core::commands::resolve::{self, ConflictFile, ConflictSession};
use velo_core::error::Result;
use velo_core::merge::{ConflictHunk, Decision};
use velo_core::WriteGuard;

const RESOLUTION_MARKER: &str = "# >>> VELO RESOLUTION — EDIT BELOW THIS LINE <<<";

/// Outcome of an interactive session over one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// All hunks decided and the file written.
    Resolved,
    /// The user quit; nothing was written.
    Aborted,
    /// The file had no real conflicts, so its stale conflict state was cleared.
    NothingToDo,
}

/// Walk the user through every hunk of `file`, writing the resolved file once all
/// hunks have a decision.
///
/// Decisions are persisted as they are made, so quitting and re-running resumes
/// where the user left off.
pub fn resolve_interactive(guard: &WriteGuard, file: ConflictFile) -> Result<Outcome> {
    let root = guard.root();
    let mut session = resolve::open_session(guard.repo(), file)?;
    if session.hunks.is_empty() {
        // No genuine conflict — this file was recorded in error.
        resolve::clear_conflict(guard, &session.file.path)?;
        return Ok(Outcome::NothingToDo);
    }

    let term = Term::stdout();
    let mut cursor: usize = 0;

    // MERGE_HEAD stores "pre_merge_hash:source_branch".
    let merge_info = std::fs::read_to_string(root.join(".velo/MERGE_HEAD")).unwrap_or_default();
    let source_branch: String = merge_info
        .trim()
        .split_once(':')
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| "(unknown)".into());

    loop {
        draw(&term, &session, cursor, &source_branch);

        let decision = match read_key(&term) {
            Key::Char('1') => Some(Decision::Ours),
            Key::Char('2') => Some(Decision::Theirs),
            Key::Char('3') => Some(Decision::BothOursFirst),
            Key::Char('4') => Some(Decision::BothTheirsFirst),
            Key::Char('e') => {
                open_in_editor(&session.file.path, &session.hunks[cursor])?.map(Decision::Manual)
            }
            Key::Char('u') | Key::Backspace => {
                session.hunks[cursor].decision = None;
                resolve::clear_decision(guard, &session.file.path, cursor).ok();
                None
            }
            Key::Char('n') | Key::ArrowRight => {
                cursor = (cursor + 1).min(session.hunks.len() - 1);
                None
            }
            Key::Char('p') | Key::ArrowLeft => {
                cursor = cursor.saturating_sub(1);
                None
            }
            Key::Char('q') | Key::Escape => {
                println!("\n{} Quit — no changes written.", style("!").yellow());
                return Ok(Outcome::Aborted);
            }
            _ => None,
        };

        if let Some(d) = decision {
            resolve::record_decision(guard, &session.file.path, cursor, &d)?;
            session.hunks[cursor].decision = Some(d);
            // Jump to the next undecided hunk.
            cursor = session
                .hunks
                .iter()
                .position(|h| h.decision.is_none())
                .unwrap_or(session.hunks.len().saturating_sub(1));
        }

        if session.all_decided() {
            let total = session.hunks.len();
            resolve::finalise(guard, &session)?;
            if term.is_term() {
                let _ = term.clear_screen();
            }
            println!(
                "{} All {} hunk(s) resolved — '{}' written.",
                style("✔").green().bold(),
                total,
                session.file.path
            );
            return Ok(Outcome::Resolved);
        }
    }
}

// ─── Rendering ────────────────────────────────────────────────────────────────

fn draw(term: &Term, session: &ConflictSession, cursor: usize, source_branch: &str) {
    if term.is_term() {
        let _ = term.clear_screen();
    } else {
        println!("{}", "─".repeat(72));
    }

    let hunk = &session.hunks[cursor];
    println!(
        "  {}  ·  Hunk {}/{}  ·  {} decided  ·  {} ← {}",
        style(&session.file.path).cyan().bold(),
        cursor + 1,
        session.hunks.len(),
        session.decided_count(),
        style("main").dim(),
        style(source_branch.trim()).yellow()
    );
    println!("{}", style("─".repeat(72)).dim());

    for line in &hunk.context_before {
        println!("  {}", style(line).dim());
    }

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

    for line in &hunk.context_after {
        println!("  {}", style(line).dim());
    }

    if let Some(ref d) = hunk.decision {
        let badge = match d {
            Decision::Ours => style("[✔ OURS]").red().bold().to_string(),
            Decision::Theirs => style("[✔ THEIRS]").green().bold().to_string(),
            Decision::BothOursFirst => style("[✔ BOTH (ours·theirs)]").yellow().bold().to_string(),
            Decision::BothTheirsFirst => {
                style("[✔ BOTH (theirs·ours)]").yellow().bold().to_string()
            }
            Decision::Manual(_) => style("[✔ MANUAL]").cyan().bold().to_string(),
        };
        println!("\n  Decided: {}", badge);
    }

    println!("{}", style("─".repeat(72)).dim());
    println!("  [1] Keep ours   [2] Take theirs   [3] Both (ours·theirs)   [4] Both (theirs·ours)");
    println!(
        "  [e] Edit in $EDITOR   [n/→] Next   [p/←] Prev   [u] Undo   [q] Quit without saving"
    );
}

fn read_key(term: &Term) -> Key {
    if term.is_term() {
        term.read_key().unwrap_or(Key::Char('n'))
    } else {
        // Non-TTY fallback: read a line so the resolver stays scriptable.
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok();
        match buf.trim() {
            "1" => Key::Char('1'),
            "2" => Key::Char('2'),
            "3" => Key::Char('3'),
            "4" => Key::Char('4'),
            "e" => Key::Char('e'),
            "p" => Key::Char('p'),
            "u" => Key::Char('u'),
            "q" => Key::Char('q'),
            _ => Key::Char('n'),
        }
    }
}

// ─── Editor integration ───────────────────────────────────────────────────────

/// Hand one hunk to `$VISUAL`/`$EDITOR` and read the resolution back.
///
/// The resolution zone is delimited by a sentinel rather than by stripping
/// `#`-prefixed lines, so comments, preprocessor directives and Markdown headings
/// survive a round-trip. (The previous implementation deleted them.)
fn open_in_editor(path: &str, hunk: &ConflictHunk) -> Result<Option<Vec<String>>> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });

    let tmp_path =
        std::env::temp_dir().join(format!("velo_hunk_{}.txt", path.replace(['/', '\\'], "_")));

    let mut content = String::from(
        "# VELO conflict hunk — edit below the RESOLUTION marker, then save and exit.\n\
         # Everything above the marker is ignored.\n#\n# ── OURS ──\n",
    );
    for line in &hunk.ours {
        content.push_str(&format!("# {}\n", line));
    }
    content.push_str("# ── THEIRS ──\n");
    for line in &hunk.theirs {
        content.push_str(&format!("# {}\n", line));
    }
    content.push_str(RESOLUTION_MARKER);
    content.push('\n');
    for line in &hunk.ours {
        content.push_str(line);
        content.push('\n');
    }

    std::fs::write(&tmp_path, &content)?;
    let status = std::process::Command::new(&editor)
        .arg(&tmp_path)
        .status()?;
    if !status.success() {
        println!(
            "{} Editor exited with a non-zero status.",
            style("!").yellow()
        );
        return Ok(None);
    }

    let edited = std::fs::read_to_string(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    match edited.find(RESOLUTION_MARKER) {
        Some(pos) => {
            let after = &edited[pos + RESOLUTION_MARKER.len()..];
            Ok(Some(
                after
                    .lines()
                    .skip_while(|l| l.trim().is_empty())
                    .map(str::to_string)
                    .collect(),
            ))
        }
        None => {
            println!(
                "{} Resolution marker missing from the edited file; hunk left undecided.",
                style("!").yellow()
            );
            Ok(None)
        }
    }
}
