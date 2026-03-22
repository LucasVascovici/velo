use std::fs;
use std::path::Path;

use console::style;
use rusqlite::params;

use crate::db;
use crate::error::Result;

struct LogEntry {
    hash: String,
    message: String,
    date: String,
    branch: String,
    parent_hash: String,
    merge_parent: String,
    tag: Option<String>,
}

pub fn run(
    root: &Path,
    all: bool,
    limit: usize,
    filter_branch: Option<&str>,
    oneline: bool,
    graph: bool,
    file_filter: Option<&str>,
) -> Result<()> {
    let conn = db::get_conn_at_path(&root.join(".velo/velo.db"))?;

    let current_parent = fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let current_parent = current_parent.trim();
    let branch_raw = fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
    let branch = branch_raw.trim();
    let sql_limit = limit as i64;

    let mut history: Vec<LogEntry> = Vec::new();

    if all || filter_branch.is_some() {
        let target_branch = filter_branch.unwrap_or("");
        let header = if let Some(b) = filter_branch {
            format!("History for branch '{}'", style(b).cyan().bold())
        } else {
            "Global history (all branches)".to_string()
        };
        println!("\n{}", style(header).bold().underlined());

        let sql = if filter_branch.is_some() {
            "SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash, s.merge_parent, t.name
             FROM snapshots s
             LEFT JOIN tags t ON s.hash = t.snapshot_hash
             WHERE s.branch = ?1
               AND s.branch NOT LIKE '_deleted_%'
               AND s.branch NOT LIKE '_stash%'
             ORDER BY s.created_at DESC, s.rowid DESC LIMIT ?2"
        } else {
            "SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash, s.merge_parent, t.name
             FROM snapshots s
             LEFT JOIN tags t ON s.hash = t.snapshot_hash
             WHERE s.branch NOT LIKE '_deleted_%'
               AND s.branch NOT LIKE '_stash%'
             ORDER BY s.created_at DESC, s.rowid DESC LIMIT ?2"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![target_branch, sql_limit], |r| {
            Ok(LogEntry {
                hash: r.get(0)?,
                message: r.get(1)?,
                date: r.get(2)?,
                branch: r.get(3)?,
                parent_hash: r.get(4)?,
                merge_parent: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                tag: r.get(6)?,
            })
        })?;
        for e in rows {
            history.push(e?);
        }
    } else {
        if current_parent.is_empty() {
            println!("No snapshots yet on branch '{}'.", style(branch).cyan());
            return Ok(());
        }
        println!("\nHistory for branch: {}", style(branch).cyan().bold());
        let mut stmt = conn.prepare(
            "WITH RECURSIVE cte(hash, message, created_at, branch, parent_hash, merge_parent) AS (
                SELECT hash, message, created_at, branch, parent_hash, merge_parent
                FROM snapshots WHERE hash = ?1
                UNION ALL
                SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash, s.merge_parent
                FROM snapshots s JOIN cte c ON s.hash = c.parent_hash
            )
            SELECT c.hash, c.message, c.created_at, c.branch, c.parent_hash, c.merge_parent, t.name
            FROM cte c
            LEFT JOIN tags t ON c.hash = t.snapshot_hash
            LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![current_parent, sql_limit], |r| {
            Ok(LogEntry {
                hash: r.get(0)?,
                message: r.get(1)?,
                date: r.get(2)?,
                branch: r.get(3)?,
                parent_hash: r.get(4)?,
                merge_parent: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                tag: r.get(6)?,
            })
        })?;
        for e in rows {
            history.push(e?);
        }
    }

    // ── File filter: only show snapshots that touched the given file ──────────
    if let Some(file) = file_filter {
        let normalised = db::normalise(file);
        history.retain(|e| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ? AND path = ?)",
                params![e.hash, normalised],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false)
        });
        if history.is_empty() {
            println!(
                "  {} No snapshots found that touched '{}'.",
                style("!").yellow(),
                file
            );
            return Ok(());
        }
    }

    if history.is_empty() {
        println!("  {}", style("No snapshots found.").dim());
        return Ok(());
    }

    if graph {
        print_graph(&history, current_parent);
    } else if oneline {
        print_oneline(&history, current_parent);
    } else {
        print_full(&history, current_parent);
    }

    println!();
    Ok(())
}

// ─── Formatters ───────────────────────────────────────────────────────────────

fn safe_date(s: &str) -> &str {
    if s.len() >= 19 {
        &s[..19]
    } else {
        s
    }
}

fn print_full(history: &[LogEntry], current_parent: &str) {
    // Column widths (fixed so header and data stay in lock-step)
    // Prefix: 4 chars ("-->" + space, or 4 spaces)
    // Hash:   12 chars
    // Branch: 18 chars
    // Date:   19 chars
    // Message: remainder
    let sep = "─".repeat(78);
    println!(
        "  {:<12}   {:<18}   {:<19}   {}",
        console::style("Hash").dim().bold(),
        console::style("Branch").dim().bold(),
        console::style("Date").dim().bold(),
        console::style("Message").dim().bold(),
    );
    println!("{}", console::style(&sep).dim());

    for e in history {
        let branch_disp = if e.branch.len() > 18 {
            format!("{}..", &e.branch[..16])
        } else {
            e.branch.clone()
        };

        let tag_str = match &e.tag {
            Some(t) => format!(" {}", console::style(format!("[{}]", t)).yellow().bold()),
            None => String::new(),
        };

        let is_current = e.hash == current_parent;
        let (arrow, hash_styled) = if is_current {
            (
                console::style("→").green().bold().to_string(),
                console::style(&e.hash).green().bold().to_string(),
            )
        } else {
            (
                " ".to_string(),
                console::style(&e.hash).yellow().to_string(),
            )
        };

        println!(
            "{} {:<12}   {:<18}   {:<19}   {}{}",
            arrow,
            hash_styled,
            console::style(&branch_disp).dim(),
            console::style(safe_date(&e.date)).dim(),
            console::style(&e.message).white(),
            tag_str,
        );
    }
    println!("{}", console::style(&sep).dim());
}

fn print_oneline(history: &[LogEntry], current_parent: &str) {
    for e in history {
        let marker = if e.hash == current_parent { "* " } else { "  " };
        let tag_str = match &e.tag {
            Some(t) => format!(" {}", style(format!("[{}]", t)).yellow()),
            None => String::new(),
        };
        println!(
            "{}{} {}{}  {}",
            marker,
            style(&e.hash).yellow(),
            style(&e.branch).dim(),
            tag_str,
            e.message
        );
    }
}

// ─── Graph renderer ───────────────────────────────────────────────────────────
//
// Strategy:
//   1. Build parent→children map from the history set.
//   2. Topological sort (Kahn's algorithm) to establish render order.
//   3. Maintain a list of "lanes" — each active branch tip occupies one lane.
//   4. For each commit, render its lane column (*), draw merges (joining lines),
//      and advance lanes (removing the commit's lane, adding its parents').

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

/// Render 2-chars-per-lane for a connector row. Active lanes get │, others get spaces.
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

fn print_graph(history: &[LogEntry], current_parent: &str) {
    use std::collections::HashSet;
    if history.is_empty() {
        return;
    }

    let known: HashSet<&str> = history.iter().map(|e| e.hash.as_str()).collect();

    // lanes[i] = Some(hash): lane i is live, waiting for that commit.
    let mut lanes: Vec<Option<String>> = Vec::new();

    for entry in history {
        let hash = entry.hash.as_str();
        let parent = entry.parent_hash.as_str();
        let mp = entry.merge_parent.as_str();
        let is_head = hash == current_parent;
        let is_merge = !mp.is_empty() && known.contains(mp);

        // ── Step 1: find/assign lane (BEFORE any lane mutation) ──────────────
        let my_lane = match lanes.iter().position(|l| l.as_deref() == Some(hash)) {
            Some(p) => p,
            None => {
                // Take leftmost free slot, or append
                if let Some(p) = lanes.iter().position(|l| l.is_none()) {
                    lanes[p] = Some(hash.to_string());
                    p
                } else {
                    lanes.push(Some(hash.to_string()));
                    lanes.len() - 1
                }
            }
        };

        // ── Step 2: print commit row (lanes are still pre-update here) ────────
        // Each lane = 2 chars: char + space (trailing space trimmed at end).
        let w = lanes.len();
        let mut row = String::new();
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
        let branch_s = style(format!("({})", entry.branch)).cyan().to_string();
        let date_s = style(safe_date(&entry.date)).dim().to_string();
        let tag_s = entry
            .tag
            .as_ref()
            .map(|t| format!("  {}", style(format!("[{}]", t)).bold().yellow()))
            .unwrap_or_default();

        // Two spaces between graph prefix and commit info.
        println!(
            "{}  {}  {}  {}  {}{}",
            row, hash_s, branch_s, date_s, entry.message, tag_s
        );

        // ── Step 3: update lanes ──────────────────────────────────────────────
        // Does primary parent already have a lane (other than ours)?
        let pp_lane: Option<usize> = if !parent.is_empty() {
            lanes
                .iter()
                .position(|l| l.as_deref() == Some(parent))
                .filter(|&p| p != my_lane)
        } else {
            None
        };

        // Save state needed for connector rows.
        let pre_width = lanes.len();
        let converging = pp_lane.map_or(false, |p| my_lane > p);
        let pp_lane_idx = pp_lane;

        // Update my lane → track primary parent (or clear if converging/root).
        if pp_lane.is_some() {
            lanes[my_lane] = None;
        } else if !parent.is_empty() && known.contains(parent) {
            lanes[my_lane] = Some(parent.to_string());
        } else {
            lanes[my_lane] = None;
        }

        // Open merge-parent lane to the right (AFTER updating, so it doesn't
        // contaminate the commit row glyph or the pp_lane search above).
        let mp_new_lane: Option<usize> = if is_merge {
            let already = lanes.iter().position(|l| l.as_deref() == Some(mp));
            if already.is_none() {
                let nl = lanes.len();
                lanes.push(Some(mp.to_string()));
                Some(nl)
            } else {
                None
            }
        } else {
            None
        };

        // Trim trailing empty lanes.
        while lanes.last() == Some(&None) {
            lanes.pop();
        }

        // ── Step 4: connector rows ────────────────────────────────────────────

        // A) Merge fork row: after a merge commit, show the new branch lane
        //    opening to the right with a ╲ diagonal.
        //    Format (2 chars per lane): │ for active lanes, ╲ for new mp lane.
        if let Some(mpl) = mp_new_lane {
            let fw = pre_width.max(mpl + 1);
            let mut fork = String::new();
            for i in 0..fw {
                if i == mpl {
                    fork.push_str(&lane_colour(mpl, "╲"));
                } else if i < lanes.len() && lanes[i].is_some() {
                    fork.push_str(&lane_colour(i, "│"));
                } else if i == my_lane {
                    // my lane continues down (it was updated above)
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

        // B) Convergence row: when my lane merges into an existing parent lane.
        //    Show ╱ at my_lane (retiring), │ everywhere else.
        if converging {
            if let Some(pil) = pp_lane_idx {
                let cw = pre_width;
                let mut conv = String::new();
                for i in 0..cw {
                    if i == my_lane {
                        conv.push_str(&lane_colour(my_lane, "╱"));
                    } else if i == pil || (i < lanes.len() && lanes[i].is_some()) {
                        conv.push_str(&lane_colour(i, "│"));
                    } else {
                        conv.push(' ');
                    }
                    if i + 1 < cw {
                        conv.push(' ');
                    }
                }
                println!("{}", conv.trim_end());
            }
        }

        // C) Standard vertical connector for all remaining active lanes.
        if !lanes.is_empty() {
            let v = v_row(&lanes);
            if !v.is_empty() {
                println!("{}", v);
            }
        }
    }
}
