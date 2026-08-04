//! Render [`History`] for the terminal: full table, oneline, or commit graph.

use console::style;
use velo_core::commands::history::{BranchRef, EmptyReason, Entry, History, Scope};

/// Which presentation to use. `velo history` picks one from its flags.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Aligned table with headers.
    Full,
    /// One line per snapshot.
    Oneline,
    /// ASCII commit graph with lanes.
    Graph,
}

pub fn print(history: &History, view: View) {
    print_header(&history.scope);

    if let Some(reason) = &history.empty {
        print_empty(reason);
        return;
    }

    match view {
        View::Graph => print_graph(history),
        View::Oneline => print_oneline(history),
        View::Full => print_full(history),
    }
    println!();
}

fn print_header(scope: &Scope) {
    match scope {
        Scope::All => println!(
            "\n{}",
            style("Global history (all branches)").bold().underlined()
        ),
        Scope::NamedBranch { name } => println!(
            "\n{}",
            style(format!(
                "History for branch '{}'",
                style(name).cyan().bold()
            ))
            .bold()
            .underlined()
        ),
        Scope::CurrentBranch { name } => {
            println!("\nHistory for branch: {}", style(name).cyan().bold())
        }
    }
}

fn print_empty(reason: &EmptyReason) {
    match reason {
        EmptyReason::UnbornBranch { branch } => {
            println!("No snapshots yet on branch '{}'.", style(branch).cyan())
        }
        EmptyReason::NoSnapshots => println!("  {}", style("No snapshots found.").dim()),
        EmptyReason::NoSnapshotsTouching { file } => println!(
            "  {} No snapshots found that touched '{}'.",
            style("!").yellow(),
            file
        ),
    }
}

// ─── Decorations ──────────────────────────────────────────────────────────────

/// `(HEAD → main, init)`, or empty when no branch points here.
fn decorate(history: &History, hash: &str) -> String {
    let refs = history.refs_at(hash);
    if refs.is_empty() {
        return String::new();
    }
    format!(" ({})", join_refs(refs))
}

/// Branches pointing at this snapshot, or — when none do — the branch it was
/// recorded on, so mid-history snapshots still carry context without repeating a
/// name twice.
fn refs_or_origin(history: &History, entry: &Entry) -> String {
    let refs = history.refs_at(&entry.hash);
    if refs.is_empty() {
        style(format!("({})", entry.branch)).dim().to_string()
    } else {
        format!("({})", join_refs(refs))
    }
}

fn join_refs(refs: &[BranchRef]) -> String {
    refs.iter()
        .map(|r| {
            if r.is_head {
                style(format!("HEAD → {}", r.name))
                    .green()
                    .bold()
                    .to_string()
            } else {
                style(&r.name).cyan().bold().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn tag_suffix(entry: &Entry, bold: bool) -> String {
    match &entry.tag {
        Some(t) => {
            let s = style(format!("[{}]", t)).yellow();
            let s = if bold { s.bold() } else { s };
            format!(" {}", s)
        }
        None => String::new(),
    }
}

/// Trim a timestamp to `YYYY-MM-DDTHH:MM:SS`.
fn short_date(s: &str) -> &str {
    if s.len() >= 19 {
        &s[..19]
    } else {
        s
    }
}

// ─── Full table ───────────────────────────────────────────────────────────────

fn print_full(history: &History) {
    // Column widths. The hash column derives from the real hash length so the
    // header and rows can't drift apart when that length changes.
    const PREFIX_W: usize = 2; // "→ " or "  "
    const GAP: usize = 3;
    const BRANCH_W: usize = 18;
    const DATE_W: usize = 19;
    let hash_w = velo_core::commands::SNAP_HASH_LEN.max(
        history
            .entries
            .iter()
            .map(|e| e.hash.len())
            .max()
            .unwrap_or(0),
    );

    // The rule spans the fixed columns plus the widest message actually shown.
    let fixed_w = PREFIX_W + hash_w + GAP + BRANCH_W + GAP + DATE_W + GAP;
    let msg_w = history
        .entries
        .iter()
        .map(|e| e.message.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(7, 40); // at least as wide as "Message"
    let sep = "─".repeat(fixed_w + msg_w);

    println!(
        "  {:<hash_w$}{:GAP$}{:<BRANCH_W$}{:GAP$}{:<DATE_W$}{:GAP$}{}",
        style("Hash").dim().bold(),
        "",
        style("Branch").dim().bold(),
        "",
        style("Date").dim().bold(),
        "",
        style("Message").dim().bold(),
    );
    println!("{}", style(&sep).dim());

    for e in &history.entries {
        // Truncate on char boundaries — branch names may be non-ASCII, and
        // slicing bytes would panic mid-character.
        let branch_disp = if e.branch.chars().count() > BRANCH_W {
            let keep: String = e.branch.chars().take(BRANCH_W - 2).collect();
            format!("{}..", keep)
        } else {
            e.branch.clone()
        };

        let is_current = history.current.as_deref() == Some(e.hash.as_str());
        let (arrow, hash_styled) = if is_current {
            (
                style("→").green().bold().to_string(),
                style(&e.hash).green().bold().to_string(),
            )
        } else {
            (" ".to_string(), style(&e.hash).yellow().to_string())
        };

        println!(
            "{} {:<hash_w$}{:GAP$}{:<BRANCH_W$}{:GAP$}{:<DATE_W$}{:GAP$}{}{}{}",
            arrow,
            hash_styled,
            "",
            style(&branch_disp).dim(),
            "",
            style(short_date(&e.created_at)).dim(),
            "",
            style(&e.message).white(),
            decorate(history, &e.hash),
            tag_suffix(e, true),
        );
    }
    println!("{}", style(&sep).dim());
}

// ─── Oneline ──────────────────────────────────────────────────────────────────

fn print_oneline(history: &History) {
    for e in &history.entries {
        let marker = if history.current.as_deref() == Some(e.hash.as_str()) {
            "* "
        } else {
            "  "
        };
        println!(
            "{}{} {}{}  {}",
            marker,
            style(&e.hash).yellow(),
            refs_or_origin(history, e),
            tag_suffix(e, false),
            e.message
        );
    }
}

// ─── Graph ────────────────────────────────────────────────────────────────────
//
// Each active line of history occupies a "lane", rendered two columns wide. For
// each snapshot we draw its lane glyph, then advance the lanes: the snapshot's
// lane starts tracking its first parent, and a merge opens a new lane to the
// right for the second parent.

/// Per-lane colour cycle: cyan → green → yellow → magenta → blue.
fn lane_colour(lane: usize, s: &str) -> String {
    match lane % 5 {
        0 => style(s).cyan().to_string(),
        1 => style(s).green().to_string(),
        2 => style(s).yellow().to_string(),
        3 => style(s).magenta().to_string(),
        _ => style(s).blue().to_string(),
    }
}

/// A connector row: `│` for every live lane, spaces elsewhere.
fn v_row(lanes: &[Option<String>]) -> String {
    let mut s = String::new();
    for (i, l) in lanes.iter().enumerate() {
        if l.is_some() {
            s.push_str(&lane_colour(i, "│"));
        } else {
            s.push(' ');
        }
        if i + 1 < lanes.len() {
            s.push(' ');
        }
    }
    s.trim_end().to_string()
}

fn print_graph(history: &History) {
    use std::collections::HashSet;

    let known: HashSet<&str> = history.entries.iter().map(|e| e.hash.as_str()).collect();
    // lanes[i] = Some(hash): lane i is live, waiting for that snapshot.
    let mut lanes: Vec<Option<String>> = Vec::new();

    for entry in &history.entries {
        let hash = entry.hash.as_str();
        let parent = entry.parent.as_deref().unwrap_or("");
        let mp = entry.merge_parent.as_deref().unwrap_or("");
        let is_head = history.current.as_deref() == Some(hash);
        let is_merge = !mp.is_empty() && known.contains(mp);

        // ── Find or claim this snapshot's lane, before any lane mutation ──────
        let my_lane = match lanes.iter().position(|l| l.as_deref() == Some(hash)) {
            Some(p) => p,
            None => match lanes.iter().position(|l| l.is_none()) {
                Some(p) => {
                    lanes[p] = Some(hash.to_string());
                    p
                }
                None => {
                    lanes.push(Some(hash.to_string()));
                    lanes.len() - 1
                }
            },
        };

        // ── Commit row, with lanes still in their pre-update state ────────────
        let w = lanes.len();
        let mut row = String::new();
        // `i` is a lane number compared against my_lane and used for colouring,
        // not just for indexing — an index loop is clearer than enumerate().
        #[allow(clippy::needless_range_loop)]
        for i in 0..w {
            if i == my_lane {
                row.push_str(&lane_colour(my_lane, if is_head { "●" } else { "○" }));
            } else if lanes[i].is_some() {
                row.push_str(&lane_colour(i, "│"));
            } else {
                row.push(' ');
            }
            if i + 1 < w {
                row.push(' ');
            }
        }

        let hash_s = if is_head {
            style(hash).white().bold().to_string()
        } else {
            style(hash).white().to_string()
        };
        println!(
            "{}  {}  {}  {}  {}{}",
            row,
            hash_s,
            refs_or_origin(history, entry),
            style(short_date(&entry.created_at)).dim(),
            entry.message,
            entry
                .tag
                .as_ref()
                .map(|t| format!("  {}", style(format!("[{}]", t)).bold().yellow()))
                .unwrap_or_default(),
        );

        // ── Advance the lanes ─────────────────────────────────────────────────
        // Does the first parent already occupy a lane other than ours?
        let pp_lane: Option<usize> = if parent.is_empty() {
            None
        } else {
            lanes
                .iter()
                .position(|l| l.as_deref() == Some(parent))
                .filter(|&p| p != my_lane)
        };

        let pre_width = lanes.len();
        let converging = pp_lane.is_some_and(|p| my_lane > p);

        // Our lane tracks the first parent, or retires when it converges or roots.
        lanes[my_lane] = if pp_lane.is_some() {
            None
        } else if !parent.is_empty() && known.contains(parent) {
            Some(parent.to_string())
        } else {
            None
        };

        // Open a lane to the right for the merge parent. Done after the update
        // above so it can't contaminate the commit glyph or the pp_lane search.
        let mp_new_lane: Option<usize> = if is_merge {
            if lanes.iter().any(|l| l.as_deref() == Some(mp)) {
                None
            } else {
                lanes.push(Some(mp.to_string()));
                Some(lanes.len() - 1)
            }
        } else {
            None
        };

        while lanes.last() == Some(&None) {
            lanes.pop();
        }

        // ── Connector rows ────────────────────────────────────────────────────

        // A merge forks a new lane off to the right.
        if let Some(mpl) = mp_new_lane {
            let fw = pre_width.max(mpl + 1);
            let mut fork = String::new();
            for i in 0..fw {
                if i == mpl {
                    fork.push_str(&lane_colour(mpl, "╲"));
                } else if i < lanes.len() && lanes[i].is_some() {
                    fork.push_str(&lane_colour(i, "│"));
                } else if i == my_lane {
                    fork.push_str(&lane_colour(my_lane, "│"));
                } else {
                    fork.push(' ');
                }
                if i + 1 < fw {
                    fork.push(' ');
                }
            }
            println!("{}", fork.trim_end());
        }

        // Our lane rejoining an existing parent lane to its left.
        if converging {
            if let Some(pil) = pp_lane {
                let mut conv = String::new();
                for i in 0..pre_width {
                    if i == my_lane {
                        conv.push_str(&lane_colour(my_lane, "╱"));
                    } else if i == pil || (i < lanes.len() && lanes[i].is_some()) {
                        conv.push_str(&lane_colour(i, "│"));
                    } else {
                        conv.push(' ');
                    }
                    if i + 1 < pre_width {
                        conv.push(' ');
                    }
                }
                println!("{}", conv.trim_end());
            }
        }

        // Verticals for whatever remains live.
        if !lanes.is_empty() {
            let v = v_row(&lanes);
            if !v.is_empty() {
                println!("{}", v);
            }
        }
    }
}
