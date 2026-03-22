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
            "SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash, t.name
             FROM snapshots s
             LEFT JOIN tags t ON s.hash = t.snapshot_hash
             WHERE s.branch = ?1
               AND s.branch NOT LIKE '_deleted_%'
               AND s.branch NOT LIKE '_stash%'
             ORDER BY s.created_at DESC, s.rowid DESC LIMIT ?2"
        } else {
            "SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash, t.name
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
                tag: r.get(5)?,
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
            "WITH RECURSIVE cte(hash, message, created_at, branch, parent_hash) AS (
                SELECT hash, message, created_at, branch, parent_hash
                FROM snapshots WHERE hash = ?1
                UNION ALL
                SELECT s.hash, s.message, s.created_at, s.branch, s.parent_hash
                FROM snapshots s JOIN cte c ON s.hash = c.parent_hash
            )
            SELECT c.hash, c.message, c.created_at, c.branch, c.parent_hash, t.name
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
                tag: r.get(5)?,
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

fn print_graph(history: &[LogEntry], current_parent: &str) {
    if history.is_empty() {
        return;
    }

    let known: std::collections::HashSet<&str> = history.iter().map(|e| e.hash.as_str()).collect();

    // lanes[i] = Some(hash) means lane i is "live" waiting for that hash to appear
    let mut lanes: Vec<Option<&str>> = Vec::new();

    for entry in history {
        let hash = entry.hash.as_str();
        let parent = entry.parent_hash.as_str();
        let is_head = hash == current_parent;

        // ── Assign this commit to a lane ──────────────────────────────────
        let my_lane = match lanes.iter().position(|l| *l == Some(hash)) {
            Some(p) => p,
            None => match lanes.iter().position(|l| l.is_none()) {
                Some(p) => {
                    lanes[p] = Some(hash);
                    p
                }
                None => {
                    lanes.push(Some(hash));
                    lanes.len() - 1
                }
            },
        };

        // ── Build commit-row prefix ────────────────────────────────────────
        let n = lanes.len();
        let mut glyphs: Vec<&str> = vec!["  "; n];
        for (i, lane) in lanes.iter().enumerate() {
            glyphs[i] = if i == my_lane {
                if is_head {
                    "● "
                } else {
                    "* "
                }
            } else if lane.is_some() {
                "| "
            } else {
                "  "
            };
        }
        let prefix: String = glyphs.concat();
        let indent = " ".repeat(prefix.len());

        // ── Print commit and message rows ──────────────────────────────────
        let tag_str = entry
            .tag
            .as_ref()
            .map(|t| format!(" {}", style(format!("[{}]", t)).yellow()))
            .unwrap_or_default();
        let blabel = format!("({})", &entry.branch);
        if is_head {
            println!(
                "{}{} {}{}",
                prefix,
                style(hash).yellow().bold(),
                style(&blabel).cyan().bold(),
                tag_str
            );
        } else {
            println!(
                "{}{} {}{}",
                prefix,
                style(hash).yellow(),
                style(&blabel).dim(),
                tag_str
            );
        }
        println!(
            "{}  {} {}",
            indent,
            style(safe_date(&entry.date)).dim(),
            style(&entry.message).white()
        );

        // ── Determine where the parent lives (if it's in our history) ─────
        let parent_in_history = !parent.is_empty() && known.contains(parent);
        let parent_lane = lanes.iter().position(|l| *l == Some(parent));

        // ── Convergence connector: draw |/ when this lane merges left ──────
        if let Some(pil) = parent_lane {
            if my_lane > pil {
                // My lane converges into pil — draw the diagonal row
                let conn: String = lanes
                    .iter()
                    .enumerate()
                    .map(|(i, lane)| {
                        if i == my_lane {
                            "/ "
                        } else if lane.is_some() {
                            "| "
                        } else {
                            "  "
                        }
                    })
                    .collect();
                println!("{}", conn);
            }
        }

        // ── Advance lanes ──────────────────────────────────────────────────
        if let Some(pil) = parent_lane {
            // Parent already tracked in another lane — retire this one
            lanes[my_lane] = None;
            let _ = pil; // suppress unused warning
        } else if parent_in_history {
            // Parent not yet in any lane — keep tracking in this lane
            lanes[my_lane] = Some(parent);
        } else {
            // Root commit or parent outside the displayed window
            lanes[my_lane] = None;
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }

        // ── Regular vertical-connector row ────────────────────────────────
        if !lanes.is_empty() {
            let conn: String = lanes
                .iter()
                .map(|l| if l.is_some() { "| " } else { "  " })
                .collect();
            println!("{}", conn);
        }
    }
}
