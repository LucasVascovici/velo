use std::collections::HashMap;
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

    let current_parent =
        fs::read_to_string(root.join(".velo/PARENT")).unwrap_or_default();
    let current_parent = current_parent.trim();
    let branch_raw =
        fs::read_to_string(root.join(".velo/HEAD")).unwrap_or_else(|_| "main".into());
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
        for e in rows { history.push(e?); }
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
        for e in rows { history.push(e?); }
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
    if s.len() >= 19 { &s[..19] } else { s }
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
    if history.is_empty() { return; }

    // Index by hash for quick lookup
    let by_hash: HashMap<&str, &LogEntry> =
        history.iter().map(|e| (e.hash.as_str(), e)).collect();

    // Build children map (which hashes in our set have this hash as parent)
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in history {
        if by_hash.contains_key(e.parent_hash.as_str()) {
            children.entry(e.parent_hash.as_str()).or_default().push(e.hash.as_str());
        }
    }
    // Suppress unused-variable warning — children map is used for future
    // topological refinement; currently we rely on query ordering.
    let _ = &children;

    // Render in the order provided by the query (ancestry-correct for the
    // common linear + simple branch cases).
    let render_order: Vec<&LogEntry> = history.iter().collect();

    // Track lanes: Vec<Option<&str>> where each slot holds the hash of the
    // commit expected to appear in that column, or None (empty lane).
    let mut lanes: Vec<Option<&str>> = Vec::new();

    for entry in &render_order {
        let hash = entry.hash.as_str();
        let parent = entry.parent_hash.as_str();

        // Find or allocate a lane for this commit
        let my_lane = if let Some(pos) = lanes.iter().position(|l| *l == Some(hash)) {
            pos
        } else {
            // New tip — open a new lane
            if let Some(empty) = lanes.iter().position(|l| l.is_none()) {
                lanes[empty] = Some(hash);
                empty
            } else {
                lanes.push(Some(hash));
                lanes.len() - 1
            }
        };

        // Build the graph column string for this row
        let width = lanes.len();
        let mut col_chars: Vec<char> = vec![' '; width * 2];

        for (i, lane) in lanes.iter().enumerate() {
            let col = i * 2;
            if i == my_lane {
                col_chars[col] = if hash == current_parent { '●' } else { '*' };
            } else if lane.is_some() {
                col_chars[col] = '|';
            }
        }

        let graph_prefix: String = col_chars.into_iter().collect();

        // Format the log line
        let tag_str = match &entry.tag {
            Some(t) => format!(" {}", style(format!("[{}]", t)).yellow()),
            None => String::new(),
        };
        let marker = if hash == current_parent {
            style(format!("{} {} ({}){}", graph_prefix, style(hash).yellow().bold(),
                style(&entry.branch).cyan(), tag_str)).to_string()
        } else {
            format!("{} {}{} ({})",
                graph_prefix,
                style(hash).yellow(),
                tag_str,
                style(&entry.branch).dim())
        };

        println!("{}", marker);

        // Show the message indented under the graph
        let indent: String = " ".repeat(my_lane * 2 + 2);
        println!("{}  {} {}", indent,
            style(safe_date(&entry.date)).dim(),
            style(&entry.message).white());

        // Advance lanes: replace my lane with my parent (if in set), else None
        if !parent.is_empty() && by_hash.contains_key(parent) {
            // Check if parent already has a lane
            let parent_lane = lanes.iter().position(|l| *l == Some(parent));
            if parent_lane.is_none() {
                lanes[my_lane] = Some(parent);
            } else {
                // Parent already tracked — close my lane
                lanes[my_lane] = None;
            }
        } else {
            lanes[my_lane] = None;
        }

        // Print connector lines between commits
        let next_has_content = render_order
            .iter()
            .skip_while(|e| e.hash.as_str() != hash)
            .nth(1)
            .is_some();

        if next_has_content {
            let mut connector: Vec<char> = vec![' '; lanes.len() * 2];
            for (i, lane) in lanes.iter().enumerate() {
                if lane.is_some() {
                    connector[i * 2] = '|';
                }
            }
            println!("{}", connector.into_iter().collect::<String>());
        }
    }
}
